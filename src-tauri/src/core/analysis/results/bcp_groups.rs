//! BCP navigation projected from the original columns of published records.

use std::collections::HashSet;

use lovelycarve_plugin::schema::RecordSchema;
use rusqlite::{Connection, ToSql};

use super::{
    models::{AnalysisTreeNode, BcpRecordScope, BcpRowGroup},
    record_query::RunRecordType,
    record_store::{table_name, value_column},
};

use crate::core::common::error::Failure;

const MAX_GROUPS_PER_TYPE: usize = 4096;
const MAX_APPLICATIONS_PER_SCOPE: usize = 128;
const GENERIC_MESSAGING: &str = "1039999";
const MAIMAI: &str = "1070109";

struct GroupColumns {
    application: String,
    account: String,
    nickname: String,
    deleted: String,
}

impl GroupColumns {
    fn from_schema(schema: &RecordSchema) -> Self {
        let type_name = schema.record_type.as_str();
        let app_fields: &[&str] = match type_name {
            "bcp.transactions" | "bcp.payment_accounts" | "bcp.shops" | "bcp.bank_cards" => {
                &["reg_account_type", "network_app", "contact_account_type"]
            }
            _ => &[
                "contact_account_type",
                "mail_tool_type",
                "network_app",
                "browse_type",
                "reg_account_type",
            ],
        };
        let application = coalesce(schema, app_fields);
        let account = coalesce(schema, &["account_id", "account"]);
        let account = if application == "''" || account == "''" {
            "''".to_owned()
        } else {
            format!("CASE WHEN {application} <> '' THEN {account} ELSE '' END")
        };
        Self {
            application,
            account,
            nickname: nullable_column(schema, "regis_nickname").unwrap_or_else(|| "NULL".into()),
            deleted: nullable_column(schema, "delete_status").unwrap_or_else(|| "NULL".into()),
        }
    }
}

fn nullable_column(schema: &RecordSchema, name: &str) -> Option<String> {
    schema
        .field_index(name)
        .map(|index| format!("NULLIF(TRIM(t.{}), '')", value_column(index)))
}

fn coalesce(schema: &RecordSchema, fields: &[&str]) -> String {
    let columns: Vec<_> = fields
        .iter()
        .filter_map(|name| nullable_column(schema, name))
        .collect();
    if columns.is_empty() {
        "''".to_owned()
    } else {
        format!("COALESCE({}, '')", columns.join(", "))
    }
}

fn photo_application(schema: &RecordSchema, application: &str) -> String {
    if !matches!(
        schema.record_type.as_str(),
        "bcp.messaging_accounts" | "bcp.friends"
    ) {
        return "NULL".to_owned();
    }
    let Some(photo) = nullable_column(schema, "user_photo") else {
        return "NULL".to_owned();
    };
    // Require the provider host and avatar namespace for the whole group.
    format!(
        "NULLIF(MIN(CASE WHEN {application} = '{GENERIC_MESSAGING}'
             AND (LOWER({photo}) GLOB 'https://i[0-9].taou.com/maimai/*'
               OR LOWER({photo}) GLOB 'http://i[0-9].taou.com/maimai/*')
             THEN '{MAIMAI}' ELSE '' END), '')"
    )
}

fn identify_code(record_type: &str, application: &str, account: &str) -> Option<&'static str> {
    match (record_type, application) {
        // GoldenEyes travel-account records carry both the location code
        // and the corresponding full application code.
        ("bcp.locations" | "bcp.vehicles", "04002") => Some("1440004"),
        ("bcp.locations" | "bcp.vehicles", "04003") => Some("1440003"),
        (record_type, "01007")
            if record_type.starts_with("bcp.email")
                && account.split_once('@').is_some_and(|(name, domain)| {
                    !name.is_empty()
                        && !name.contains(char::is_whitespace)
                        && domain.eq_ignore_ascii_case("qq.com")
                }) =>
        {
            Some("1010008")
        }
        _ => None,
    }
}

/// `None` leaves a type available as a single table when its grouping would exceed the directory budget.
pub(crate) fn groups(
    connection: &Connection,
    run_id: &str,
    record_type: &RunRecordType,
) -> anyhow::Result<Option<Vec<BcpRowGroup>>> {
    let columns = GroupColumns::from_schema(&record_type.schema);
    let identified = photo_application(&record_type.schema, &columns.application);
    let table = table_name(record_type.type_id);
    let limit = MAX_GROUPS_PER_TYPE + 1;
    let mut statement = connection.prepare(&format!(
        "SELECT {application}, {account}, MAX({nickname}), COUNT(*),
                SUM(CASE WHEN {deleted} = '1' THEN 1 ELSE 0 END), {identified}
         FROM record r JOIN {table} t ON t.record_id = r.record_id
         WHERE r.run_id = ?1 AND r.type_id = ?2
         GROUP BY 1, 2 ORDER BY 1, 2 LIMIT {limit}",
        application = columns.application,
        account = columns.account,
        nickname = columns.nickname,
        deleted = columns.deleted,
    ))?;
    let grouped = statement
        .query_map(rusqlite::params![run_id, record_type.type_id], |row| {
            let application: String = row.get(0)?;
            let account: String = row.get(1)?;
            let identified_application = row.get::<_, Option<String>>(5)?.or_else(|| {
                identify_code(&record_type.schema.record_type, &application, &account)
                    .map(str::to_owned)
            });
            Ok(BcpRowGroup {
                application,
                identified_application,
                account,
                account_label: row.get(2)?,
                total: row.get::<_, i64>(3)? as u64,
                deleted: row.get::<_, i64>(4)? as u64,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok((grouped.len() <= MAX_GROUPS_PER_TYPE).then_some(grouped))
}

/// Direct messages can inherit the provider of their exact exported owner
/// account within the same run and generic application code.
pub(crate) fn identify_messaging_groups(nodes: &mut [AnalysisTreeNode]) {
    let accounts = nodes
        .iter()
        .filter(|node| node.artifact_type == "bcp.messaging_accounts")
        .flat_map(|node| node.bcp_groups.iter().flatten())
        .filter(|group| {
            group.identified_application.as_deref() == Some(MAIMAI) && !group.account.is_empty()
        })
        .map(|group| group.account.clone())
        .collect::<HashSet<_>>();
    for group in nodes
        .iter_mut()
        .filter(|node| node.artifact_type == "bcp.direct_messages")
        .flat_map(|node| node.bcp_groups.iter_mut().flatten())
    {
        if group.application == GENERIC_MESSAGING && accounts.contains(&group.account) {
            group.identified_application = Some(MAIMAI.to_owned());
        }
    }
}

pub(crate) fn append_scope(
    schema: &RecordSchema,
    scope: &BcpRecordScope,
    filter: &mut String,
    parameters: &mut Vec<Box<dyn ToSql>>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        schema.record_type.starts_with("bcp.")
            && (1..=MAX_APPLICATIONS_PER_SCOPE).contains(&scope.applications.len()),
        Failure::invalid_input("无效的 BCP 应用分组筛选")
    );
    let columns = GroupColumns::from_schema(schema);
    let placeholders = vec!["?"; scope.applications.len()].join(", ");
    filter.push_str(&format!(" AND {} IN ({placeholders})", columns.application));
    parameters.extend(
        scope
            .applications
            .iter()
            .cloned()
            .map(|value| Box::new(value) as Box<dyn ToSql>),
    );
    if let Some(account) = &scope.account {
        filter.push_str(&format!(" AND {} = ?", columns.account));
        parameters.push(Box::new(account.clone()));
    }
    Ok(())
}
