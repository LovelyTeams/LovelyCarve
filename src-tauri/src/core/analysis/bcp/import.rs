//! Stage BCP rows as regular analysis records. Publication belongs to the app.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use lovelycarve_plugin::{
    protocol::WireRecord,
    record::SourceRef,
    schema::{self, FieldKind, FieldSchema, RecordSchema},
    Value,
};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::core::common::error::Failure;
use crate::core::{
    analysis::results::{
        record_query,
        record_store::{self, NewRun, RecordWriter, RunCompletion, RunInput, RunTarget},
    },
    casework::case::evidence_path_key,
    evidence::image::ReadFaultReport,
};

use super::{BcpDataset, BcpPackage};

pub(crate) const BCP_PLUGIN_ID: &str = "org.lovelycarve.bcp";
const JOB: &str = "org.lovelycarve.bcp/import";
const BATCH_ROWS: usize = 2048;
const BATCH_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpImportResult {
    pub run_id: String,
    pub record_count: u64,
    pub reused: bool,
}

pub(crate) fn published(case_dir: &Path, image_id: &str) -> Result<Option<BcpImportResult>> {
    let Some(connection) = record_query::open(case_dir, image_id)? else {
        return Ok(None);
    };
    Ok(record_query::published_runs(&connection)?
        .into_iter()
        .find(|run| run.job_key == JOB && run.plugin_id == BCP_PLUGIN_ID)
        .map(|run| BcpImportResult {
            run_id: run.run_id,
            record_count: run.record_count,
            reused: true,
        }))
}

/// Owns an unpublished run until publication or cleanup.
pub(crate) struct StagedImport {
    case_dir: PathBuf,
    image_id: String,
    run_id: String,
    record_count: u64,
    published: bool,
}

impl Drop for StagedImport {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        if let Err(error) = record_store::discard(&self.case_dir, &self.image_id, &self.run_id) {
            tracing::warn!(run_id = %self.run_id, "discard BCP staging run: {error:#}");
        }
    }
}

impl StagedImport {
    pub(crate) fn publish(mut self) -> Result<BcpImportResult> {
        record_store::publish(
            &self.case_dir,
            &self.image_id,
            &self.run_id,
            &RunCompletion {
                summary: None,
                finished_at: &chrono::Utc::now().to_rfc3339(),
                read_faults: &ReadFaultReport::default(),
            },
        )?
        .context(Failure::not_found("BCP 的暂存运行已不存在"))?;
        self.published = true;
        Ok(BcpImportResult {
            run_id: self.run_id.clone(),
            record_count: self.record_count,
            reused: false,
        })
    }
}

pub(crate) fn stage(
    case_dir: &Path,
    image_id: &str,
    image_path: &Path,
    package: &BcpPackage,
    token: &CancellationToken,
    progress: impl Fn(u64, u64),
) -> Result<StagedImport> {
    let catalog = package.catalog();
    let mut datasets = catalog.datasets.iter().collect::<Vec<_>>();
    datasets.sort_unstable_by_key(|table| &table.code);
    let schemes = datasets
        .iter()
        .map(|table| table_schema(table))
        .collect::<Result<Vec<_>>>()?;
    let schemas: Vec<_> = schemes.iter().map(|(schema, _)| schema.clone()).collect();
    let catalog_path = &catalog.path;
    let outer_zip = enclosing_zip(image_path, Path::new(catalog_path));
    let mut inputs = Vec::new();
    let mut names = HashSet::new();
    for file in datasets.iter().flat_map(|table| &table.files) {
        anyhow::ensure!(!token.is_cancelled(), Failure::cancelled("BCP 导入已取消"));
        if names.insert(&file.name) {
            inputs.push(RunInput {
                path: outer_zip
                    .as_ref()
                    .map_or_else(|| file.name.clone(), |zip| format!("{zip}:{}", file.name)),
                root: Some("root".to_owned()),
                root_path: Some(outer_zip.clone().unwrap_or_else(|| file.name.clone())),
                node_id: None,
                size: package.member_size(&file.name)?,
            });
        }
    }
    anyhow::ensure!(!token.is_cancelled(), Failure::cancelled("BCP 导入已取消"));
    let run_id = format!("run_{}", uuid::Uuid::new_v4().simple());
    let metadata = serde_json::json!({ "format": "bcp" });
    record_store::begin_run(
        case_dir,
        image_id,
        &NewRun {
            run_id: &run_id,
            job_key: JOB,
            plugin_id: BCP_PLUGIN_ID,
            plugin_version: env!("CARGO_PKG_VERSION"),
            analyzer: "import",
            analyzer_name: "BCP 取证结果",
            category: "bcp",
            target: &RunTarget {
                kind: "evidence".to_owned(),
                id: String::new(),
                label: String::new(),
            },
            options: &metadata,
            profile: &serde_json::json!({}),
            inputs: &inputs,
            schema_hash: &schema::schema_hash(&schemas),
            // Built into LovelyCarve: its version names the code.
            library_sha256: None,
            started_at: &chrono::Utc::now().to_rfc3339(),
        },
    )?;
    let mut staging = StagedImport {
        case_dir: case_dir.to_owned(),
        image_id: image_id.to_owned(),
        run_id,
        record_count: 0,
        published: false,
    };
    let mut writer = RecordWriter::open(case_dir, image_id, &staging.run_id, &schemas)?;
    let mut batch = Vec::with_capacity(BATCH_ROWS);
    let mut bytes = 0;
    progress(0, catalog.total_rows);
    for (dataset, (schema, columns)) in datasets.into_iter().zip(schemes) {
        let mut number = 0;
        package.visit_rows(dataset, token, |file, offset, length, values| {
            number += 1;
            anyhow::ensure!(
                values.len() == dataset.fields.len(),
                "{} 第 {number} 条记录有 {} 个字段，BCP 索引声明 {} 个",
                file.name,
                values.len(),
                dataset.fields.len()
            );
            let source = match &outer_zip {
                Some(zip) => {
                    SourceRef::path("root", zip).note(format!("{} @ {offset}+{length}", file.name))
                }
                None => SourceRef::path("root", &file.name).at(offset, length),
            };
            batch.push(WireRecord {
                record_type: schema.record_type.clone(),
                values: columns
                    .iter()
                    .map(|column| Value::from(values[*column].as_str()))
                    .collect(),
                title: Some(format!("{} #{number}", schema.label)),
                severity: None,
                sources: vec![source],
            });
            bytes += length;
            if batch.len() == BATCH_ROWS || bytes >= BATCH_BYTES {
                writer
                    .insert_batch(&batch)
                    .with_context(|| format!("导入 {} 第 {number} 条记录所在批次", dataset.code))?;
                progress(writer.records(), catalog.total_rows);
                batch.clear();
                bytes = 0;
            }
            Ok(())
        })?;
    }
    anyhow::ensure!(!token.is_cancelled(), Failure::cancelled("BCP 导入已取消"));
    if !batch.is_empty() {
        writer.insert_batch(&batch)?;
    }
    if writer.records() == 0 {
        writer.warn("BCP_EMPTY", "BCP 数据包没有记录", None, None)?;
    }
    staging.record_count = writer.records();
    progress(staging.record_count, catalog.total_rows);
    Ok(staging)
}

fn enclosing_zip(evidence: &Path, source: &Path) -> Option<String> {
    (evidence_path_key(&evidence.to_string_lossy()) != evidence_path_key(&source.to_string_lossy()))
        .then(|| {
            source
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .flatten()
}

fn table_schema(table: &BcpDataset) -> Result<(RecordSchema, Vec<usize>)> {
    let fallback = table.code.to_ascii_lowercase();
    let (name, label, primary): (&str, &str, &[&str]) = match schema_hints(&table.code) {
        Some(hints) => hints,
        None => (fallback.as_str(), table.code.as_str(), &[]),
    };
    let mut schema = RecordSchema::new(format!("bcp.{name}"), label);
    let mut columns = primary
        .iter()
        .filter_map(|name| {
            table
                .fields
                .iter()
                .position(|field| field.name.eq_ignore_ascii_case(name))
        })
        .collect::<Vec<_>>();
    let visible = columns.len();
    let remaining = (0..table.fields.len())
        .filter(|index| !columns.contains(index))
        .collect::<Vec<_>>();
    columns.extend(remaining);
    let mut used = HashSet::new();
    for (position, &index) in columns.iter().enumerate() {
        let source = &table.fields[index];
        let normalized = source.name.to_ascii_lowercase().replace(
            |character: char| !character.is_ascii_alphanumeric() && character != '_',
            "_",
        );
        let mut id = normalized;
        if schema::validate_identifier("field id", &id).is_err() || used.contains(&id) {
            id = format!("field_{}", index + 1);
            while used.contains(&id) {
                id.push('_');
            }
        }
        used.insert(id.clone());
        let label = if source.label.trim().is_empty() {
            &source.name
        } else {
            &source.label
        };
        let label = if label.trim().is_empty() {
            id.as_str()
        } else {
            label.as_str()
        };
        let field = FieldSchema::new(&id, label, FieldKind::Text);
        schema.fields.push(
            if (visible > 0 && position >= visible) || source.name == "COLLECT_TARGET_ID" {
                field.hidden()
            } else {
                field
            },
        );
    }
    if schema.fields.is_empty() {
        schema
            .fields
            .push(FieldSchema::new("value", "记录内容", FieldKind::Text));
    }
    schema.validate().map_err(anyhow::Error::msg)?;
    Ok((schema, columns))
}

mod presentation;
use presentation::schema_hints;

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::core::analysis::results::record_query::RecordsQuery;

    #[test]
    fn only_complete_bcp_runs_replace_published_results() {
        let source = tempfile::tempdir().unwrap();
        let case = tempfile::tempdir().unwrap();
        let path = source.path().join("messages.bcp");
        fs::write(source.path().join("GAB_ZIP_INDEX.xml"), r#"<MESSAGE><DATASET name="WA_COMMON_010013"><DATA><ITEM key="A010004" val="WA_MFORENSICS_020600"/><DATASET name="WA_COMMON_010014"><DATA><ITEM key="H010020" val="messages.bcp"/><ITEM key="I010034" val="2"/></DATA></DATASET><DATASET name="WA_COMMON_010015"><DATA><ITEM key="A" eng="GROUP_NAME" chn="群名称"/><ITEM key="B" eng="CONTENT" chn="消息内容"/></DATA></DATASET></DATA></DATASET></MESSAGE>"#).unwrap();
        let token = CancellationToken::new();
        let stage_source = || {
            stage(
                case.path(),
                "evidence",
                source.path(),
                &BcpPackage::open(source.path(), None).unwrap(),
                &token,
                |_, _| {},
            )
        };
        fs::write(&path, "alpha\tfirst\nbeta\tsecond\n").unwrap();
        let staged = stage_source().unwrap();
        assert!(published(case.path(), "evidence").unwrap().is_none());
        let imported = staged.publish().unwrap();
        fs::write(&path, "beta\ttruncated\n").unwrap();
        assert!(stage_source().is_err());
        let current = published(case.path(), "evidence").unwrap().unwrap();
        assert_eq!(
            (current.run_id, current.record_count),
            (imported.run_id.clone(), 2)
        );
        let connection = record_query::open(case.path(), "evidence")
            .unwrap()
            .unwrap();
        let page = record_query::records_page(
            &connection,
            &RecordsQuery {
                run_id: &imported.run_id,
                record_type: "bcp.group_messages",
                keyword: Some("second"),
                filters: &[],
                bcp_scope: None,
                sort: None,
                descending: false,
                skip: 0,
                limit: 100,
            },
        )
        .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(
            page.items[0].sources[0].offset,
            Some("alpha\tfirst\n".len() as u64)
        );
        assert_eq!(
            page.items[0].values.get("content"),
            Some(&serde_json::json!("second"))
        );

        fs::write(&path, "alpha\tnew\nbeta\tversion\n").unwrap();
        let replacement = stage_source().unwrap().publish().unwrap();
        assert_ne!(replacement.run_id, imported.run_id);
        assert!(record_query::published_run(&connection, &imported.run_id)
            .unwrap()
            .is_none());
    }
}
