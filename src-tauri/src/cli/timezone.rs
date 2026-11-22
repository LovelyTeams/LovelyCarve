//! Display-only timestamp conversion. Never infer an instant for a naive time,
//! change stored metadata, or apply the examiner's host timezone.
mod evidence;

use std::{collections::BTreeMap, sync::Arc};

use chrono::{DateTime, FixedOffset, Timelike};
use jiff::tz::{Offset, TimeZone, TimeZoneDatabase};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{exec::Exec, CliError};
use crate::core::{
    casework::case::evidence_path_key, common::ids::ImageId,
    filesystem::evidence_session::EvidenceSession,
};

/// What holds the timestamps being converted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// One node.
    Node,
    /// A page whose `items` are nodes.
    Page,
}

const CASE_KEY: &str = "cli.timezone";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Selection {
    pub zone: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_key: Option<String>,
    pub warnings: Vec<String>,
    #[serde(skip)]
    pub timezone: TimeZone,
}

impl Selection {
    fn configured(zone: &str, source: &str) -> Result<Self, CliError> {
        Ok(Self {
            zone: zone.to_owned(),
            source: source.to_owned(),
            configuration: None,
            volume_key: None,
            warnings: vec![],
            timezone: parse(zone)?,
        })
    }
}

fn parse(zone: &str) -> Result<TimeZone, CliError> {
    if zone == "UTC" {
        return Ok(TimeZone::UTC);
    }
    let bytes = zone.as_bytes();
    if bytes.len() == 6
        && matches!(bytes[0], b'+' | b'-')
        && bytes[3] == b':'
        && [bytes[1], bytes[2], bytes[4], bytes[5]]
            .iter()
            .all(u8::is_ascii_digit)
    {
        let hours = i32::from(bytes[1] - b'0') * 10 + i32::from(bytes[2] - b'0');
        let minutes = i32::from(bytes[4] - b'0') * 10 + i32::from(bytes[5] - b'0');
        if hours <= 23 && minutes < 60 && zone != "-00:00" {
            let seconds = (hours * 3600 + minutes * 60) * if bytes[0] == b'-' { -1 } else { 1 };
            return Offset::from_seconds(seconds)
                .map(TimeZone::fixed)
                .map_err(super::internal_error);
        }
    } else if !zone.is_empty() && !zone.starts_with(['+', '-']) {
        // A bundled database is deterministic on Windows as well as Unix, and
        // never reads the examiner's TZ environment or localtime configuration.
        if let Ok(timezone) = TimeZoneDatabase::bundled().get(zone) {
            return Ok(timezone);
        }
    }
    Err(CliError::invalid(format!(
        "无效时区：{zone}；请用 Asia/Shanghai 等 IANA 名称、UTC、±HH:MM、auto、evidence 或 case"
    )))
}

pub(super) fn validate(zone: &str) -> Result<(), CliError> {
    match zone {
        "auto" | "evidence" | "case" => Ok(()),
        _ => parse(zone).map(|_| ()),
    }
}

fn image_key(image: &EvidenceSession) -> String {
    let identity = evidence_path_key(&image.path.to_string_lossy());
    format!(
        "{CASE_KEY}.image.{}",
        hex::encode(Sha256::digest(identity.as_bytes()))
    )
}

async fn settings(exec: &Exec) -> Result<BTreeMap<String, String>, CliError> {
    if exec.state.active_case.lock().await.is_none() {
        return Ok(BTreeMap::new());
    }
    let entries =
        crate::commands::casework::case::case_option_query(exec.state(), Some(CASE_KEY.to_owned()))
            .await
            .map_err(|error| super::command_error(error).in_operation("case_option_query"))?;
    Ok(entries
        .into_iter()
        .map(|entry| (entry.key, entry.value))
        .collect())
}

async fn loaded(
    exec: &Exec,
    image_id: Option<&ImageId>,
) -> Result<Option<Arc<EvidenceSession>>, CliError> {
    match image_id {
        Some(image_id) => exec.image(image_id).await.map(Some),
        None => Ok(None),
    }
}

struct Display {
    configured: Option<Selection>,
    detected: evidence::Detection,
    strict_evidence: bool,
}

impl Display {
    fn select(&self, node_id: Option<&str>) -> Result<Selection, CliError> {
        if let Some(selection) = &self.configured {
            return Ok(selection.clone());
        }
        self.detected.select(node_id, self.strict_evidence)
    }
}

async fn display(
    exec: &Exec,
    image: Option<Arc<EvidenceSession>>,
    override_zone: Option<&str>,
) -> Result<Display, CliError> {
    let policy = override_zone.unwrap_or("auto");
    validate(policy)?;
    let mut configured = None;
    if !matches!(policy, "auto" | "evidence" | "case") {
        configured = Some(Selection::configured(policy, "command")?);
    } else if policy != "evidence" {
        let entries = settings(exec).await?;
        let preference = image
            .as_ref()
            .and_then(|image| entries.get(&image_key(image)))
            .filter(|zone| zone.as_str() != "auto")
            .filter(|_| policy == "auto");
        let (zone, source) = match preference {
            Some(zone) => (Some(zone), "imageSetting"),
            None => (
                entries.get(CASE_KEY).filter(|zone| zone.as_str() != "auto"),
                "caseSetting",
            ),
        };
        if let Some(zone) = zone {
            configured = Some(Selection::configured(zone, source)?);
        } else if policy == "case" {
            return Err(CliError::invalid(
                "案件未设置显示时区；先运行 tz set Asia/Shanghai",
            ));
        }
    }
    let detected = if configured.is_none() {
        tokio::task::spawn_blocking(move || evidence::detect(image.as_ref()))
            .await
            .map_err(super::internal_error)?
    } else {
        evidence::Detection::default()
    };
    Ok(Display {
        configured,
        detected,
        strict_evidence: policy == "evidence",
    })
}

/// `tz`: the effective display zone for `image_id`'s evidence, with the
/// candidates detection found.
pub(super) async fn get(
    exec: &Exec,
    image_id: Option<&ImageId>,
    override_zone: Option<&str>,
) -> Result<Value, CliError> {
    let image = loaded(exec, image_id).await?;
    let display = display(exec, image, override_zone).await?;
    Ok(
        json!({"effective": display.select(None)?, "candidates": display.detected.candidates,
        "unresolvedVolumes": display.detected.unresolved_volumes, "policy": override_zone.unwrap_or("auto")}),
    )
}

/// `tz set`: save `zone` for the case, or for one evidence item.
pub(super) async fn set(
    exec: &Exec,
    image_id: Option<&ImageId>,
    zone: &str,
) -> Result<Value, CliError> {
    validate(zone)?;
    if matches!(zone, "case" | "evidence") {
        return Err(CliError::invalid(
            "保存的设置需要具体时区或 auto；evidence、case 只用于 --tz",
        ));
    }
    if exec.state.active_case.lock().await.is_none() {
        return Err(CliError::new(
            "usage",
            "时区设置保存在案件中：用 -C <案件目录> 指定案件；只影响一条命令时用 --tz",
        ));
    }
    let (key, scope) = match loaded(exec, image_id).await? {
        Some(image) => (image_key(&image), "evidence"),
        None => (CASE_KEY.to_owned(), "case"),
    };
    crate::commands::casework::case::case_option_save(
        exec.state(),
        vec![crate::app::workflows::casework::CaseOption {
            key,
            value: zone.to_owned(),
        }],
    )
    .await
    .map_err(|error| super::command_error(error).in_operation("case_option_save"))?;
    Ok(json!({"zone": zone, "scope": scope, "saved": true}))
}

/// Convert the node timestamps of `value` for display.
pub(super) async fn apply(
    exec: &Exec,
    image_id: &ImageId,
    override_zone: Option<&str>,
    value: &mut Value,
    shape: Shape,
) -> Result<(), CliError> {
    let image = loaded(exec, Some(image_id)).await?;
    let display = display(exec, image, override_zone).await?;
    render_response(value, &display, shape)
}

fn render_response(value: &mut Value, display: &Display, shape: Shape) -> Result<(), CliError> {
    if shape == Shape::Node {
        return render_node(value, display);
    }
    let mut zones = Vec::new();
    let mut indexes = Vec::new();
    if let Some(items) = value.get_mut("items").and_then(Value::as_array_mut) {
        for node in items.iter_mut() {
            render_node(node, display)?;
            let metadata = node
                .as_object_mut()
                .and_then(|node| node.remove("timestampDisplay"));
            let Some(metadata) = metadata else {
                indexes.push(None);
                continue;
            };
            let zone = metadata["timeZone"].clone();
            let index = match zones.iter().position(|existing| *existing == zone) {
                Some(index) => index,
                None => {
                    zones.push(zone);
                    zones.len() - 1
                }
            };
            indexes.push(Some(index));
            if metadata["unconvertedFields"]
                .as_array()
                .is_some_and(|fields| !fields.is_empty())
            {
                node["timestampUnconvertedFields"] = metadata["unconvertedFields"].clone();
            }
        }
        if zones.len() > 1 {
            for (item, index) in items.iter_mut().zip(indexes) {
                if let Some(index) = index {
                    item["timestampTimeZone"] = json!(index);
                }
            }
        }
    }
    if zones.is_empty() {
        zones.push(json!(display.select(None)?));
    }
    value["timestampDisplay"] = json!({
        "timeZones": zones,
        "unconvertedReason": "无已知 UTC 偏移、非法时间或无法无损转换；保留原值"
    });
    Ok(())
}

fn render_node(node: &mut Value, display: &Display) -> Result<(), CliError> {
    let Some(object) = node.as_object_mut() else {
        return Ok(());
    };
    let Some(original) = object.get("timestamps").cloned() else {
        return Ok(());
    };
    let selection = display.select(object.get("id").and_then(Value::as_str))?;
    let mut converted = original.clone();
    let mut unchanged = Vec::new();
    if let Some(timestamps) = converted.as_object_mut() {
        for field in ["modified", "accessed", "created", "mftChanged", "birth"] {
            if let Some(raw) = timestamps.get(field).and_then(Value::as_str) {
                match convert(raw, &selection.timezone) {
                    Some(rendered) => {
                        timestamps.insert(field.into(), json!(rendered));
                    }
                    None => unchanged.push(field),
                }
            }
        }
    }
    if converted != original {
        object.insert("timestampsOriginal".into(), original);
    }
    object.insert("timestamps".into(), converted);
    object.insert("timestampDisplay".into(), json!({"timeZone": selection, "unconvertedFields": unchanged,
        "unconvertedReason": if unchanged.is_empty() { Value::Null } else { json!("无已知 UTC 偏移、非法时间或无法无损转换；保留原值") }}));
    Ok(())
}

fn convert(raw: &str, zone: &TimeZone) -> Option<String> {
    // RFC 3339 -00:00 means the offset is unknown. Parsing it as UTC would
    // fabricate evidence, just as appending Z to a FAT local time would.
    if raw.ends_with("-00:00") {
        return None;
    }
    let date = DateTime::parse_from_rfc3339(raw).ok()?;
    if date.nanosecond() >= 1_000_000_000 {
        return None;
    }
    let instant = jiff::Timestamp::new(date.timestamp(), date.nanosecond() as i32).ok()?;
    let seconds = zone.to_offset(instant).seconds();
    let rendered = date.with_timezone(&FixedOffset::east_opt(seconds)?);
    let mut text = rendered.format("%Y-%m-%dT%H:%M:%S").to_string();
    if let Some((_, fraction)) = raw.split_once('.') {
        let fraction: String = fraction.chars().take_while(char::is_ascii_digit).collect();
        text.push('.');
        text.push_str(&fraction);
    }
    text.push_str(
        &rendered
            .format(if seconds % 60 == 0 { "%:z" } else { "%::z" })
            .to_string(),
    );
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_keep_instant_precision_and_dst() {
        let zone = parse("America/New_York").unwrap();
        assert_eq!(
            convert("2024-01-01T00:00:00.123456789+00:00", &zone).unwrap(),
            "2023-12-31T19:00:00.123456789-05:00"
        );
        assert_eq!(
            convert("2024-07-01T00:00:00.000+00:00", &zone).unwrap(),
            "2024-06-30T20:00:00.000-04:00"
        );
        assert_eq!(
            convert("2024-01-01T00:00:00Z", &parse("+05:45").unwrap()).unwrap(),
            "2024-01-01T05:45:00+05:45"
        );
        assert_eq!(
            convert("2024-01-01T08:00:00+08:00", &parse("UTC").unwrap()).unwrap(),
            "2024-01-01T00:00:00+00:00"
        );
    }

    #[test]
    fn unknown_offsets_and_naive_times_are_never_assigned_a_zone() {
        for raw in [
            "2024-01-01T00:00:00",
            "2024-01-01T00:00:00-00:00",
            "invalid",
        ] {
            assert!(convert(raw, &TimeZone::UTC).is_none());
        }
        for zone in ["+24:00", "+08:99", "-00:00", "No/SuchZone", ""] {
            assert!(validate(zone).is_err());
        }
    }

    #[test]
    fn node_rendering_keeps_roles_and_original_unknown_local_times() {
        let raw = json!({"modified": "2024-01-01T00:00:00.120Z", "accessed": "2024-01-01T01:00:00",
            "created": null, "mftChanged": "2024-01-02T00:00:00Z", "birth": "2024-01-01T00:00:00-00:00"});
        let mut node = json!({"id": "test", "timestamps": raw, "name": "2024-01-01T00:00:00Z"});
        let display = Display {
            configured: Some(Selection::configured("Asia/Shanghai", "test").unwrap()),
            detected: evidence::Detection::default(),
            strict_evidence: false,
        };
        render_node(&mut node, &display).unwrap();
        assert_eq!(node["timestampsOriginal"], raw);
        assert_eq!(
            node["timestamps"]["modified"],
            "2024-01-01T08:00:00.120+08:00"
        );
        assert_eq!(
            node["timestamps"]["mftChanged"],
            "2024-01-02T08:00:00+08:00"
        );
        assert_eq!(node["timestamps"]["accessed"], raw["accessed"]);
        assert_eq!(node["timestamps"]["birth"], raw["birth"]);
        assert_eq!(
            node["timestampDisplay"]["unconvertedFields"],
            json!(["accessed", "birth"])
        );
        assert!(node["timestamps"]["created"].is_null());
        assert_eq!(node["name"], "2024-01-01T00:00:00Z");
    }
    #[test]
    fn list_provenance_is_shared_and_identical_originals_are_omitted() {
        let display = Display {
            configured: Some(Selection::configured("UTC", "test").unwrap()),
            detected: evidence::Detection::default(),
            strict_evidence: false,
        };
        let original =
            json!({"modified":"2024-01-01T00:00:00+00:00", "created":"2024-01-01T01:00:00"});
        let node = json!({"id":"test", "timestamps": original});
        let mut response = json!({"items":[node.clone(), node], "nextCursor":null});
        render_response(&mut response, &display, Shape::Page).unwrap();
        assert_eq!(
            response["timestampDisplay"]["timeZones"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        for node in response["items"].as_array().unwrap() {
            assert_eq!(node["timestamps"], original);
            assert!(node.get("timestampsOriginal").is_none());
            assert!(node.get("timestampDisplay").is_none());
            assert!(node.get("timestampTimeZone").is_none());
            assert_eq!(node["timestampUnconvertedFields"], json!(["created"]));
        }
        let mut empty = json!({"items":[]});
        render_response(&mut empty, &display, Shape::Page).unwrap();
        assert_eq!(empty["timestampDisplay"]["timeZones"][0]["source"], "test");
    }

    #[test]
    fn mixed_volumes_keep_per_node_timezone_indexes_in_tree_pages() {
        let mut first = Selection::configured("+08:00", "test").unwrap();
        first.volume_key = Some("part:0".into());
        let mut second = Selection::configured("-04:00", "test").unwrap();
        second.volume_key = Some("part:1".into());
        let display = Display {
            configured: None,
            detected: evidence::Detection {
                candidates: vec![first, second],
                ..Default::default()
            },
            strict_evidence: true,
        };
        let mut response = json!({"items":[
            {"id":"ntfs:part:0:5", "timestamps":{"modified":"2024-01-01T00:00:00Z"}},
            {"id":"ntfs:part:1:5", "timestamps":{"modified":"2024-01-01T00:00:00Z"}}
        ]});
        render_response(&mut response, &display, Shape::Page).unwrap();
        assert_eq!(
            response["timestampDisplay"]["timeZones"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(response["items"][0]["timestampTimeZone"], 0);
        assert_eq!(response["items"][1]["timestampTimeZone"], 1);
        assert_eq!(
            response["items"][0]["timestamps"]["modified"],
            "2024-01-01T08:00:00+08:00"
        );
        assert_eq!(
            response["items"][1]["timestamps"]["modified"],
            "2023-12-31T20:00:00-04:00"
        );
    }
}
