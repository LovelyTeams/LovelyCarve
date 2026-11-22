//! The workspace: the evidence a process holds, each under a short alias that
//! addresses start with (`/<alias>/…`). Case evidence keeps its alias in the
//! case settings and its ID in the case ledger, so every process that opens
//! the case resolves the same address to the same evidence.
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{
    browsing::{self, LastLink},
    exec::Exec,
    types::{Address, Parts},
    CliError,
};
use crate::{
    app::workflows::casework::CaseOption,
    core::{casework::case::evidence_path_key, common::ids::ImageId},
    host::State,
};

/// Case settings that hold the aliases, keyed by the evidence path.
const ALIAS_KEY: &str = "cli.alias.";
const MAX_ALIAS: usize = 64;

/// An alias this process holds for an image.
#[derive(Clone, Debug)]
struct Named {
    alias: String,
    /// Chosen with `--as` or for evidence a command derived; a suggested
    /// alias may still give way to one of those.
    explicit: bool,
}

/// Aliases this process assigned, by image ID: derived and caseless
/// evidence live only here; case evidence is cached here too. Once given, an
/// alias stays, so adding evidence never renames other evidence.
#[derive(Clone, Default)]
pub struct Workspace(Arc<Mutex<BTreeMap<ImageId, Named>>>);

impl Workspace {
    fn with<T>(&self, change: impl FnOnce(&mut BTreeMap<ImageId, Named>) -> T) -> T {
        match self.0.lock() {
            Ok(mut aliases) => change(&mut aliases),
            Err(poisoned) => change(&mut poisoned.into_inner()),
        }
    }

    fn aliases(&self) -> BTreeMap<ImageId, Named> {
        self.with(|aliases| aliases.clone())
    }

    fn set(&self, image_id: ImageId, alias: String, explicit: bool) {
        self.with(|aliases| {
            aliases.insert(image_id, Named { alias, explicit });
        });
    }

    pub fn forget(&self, image_id: &ImageId) {
        self.with(|aliases| {
            aliases.remove(image_id);
        });
    }
}

/// One evidence item of the workspace.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub alias: String,
    pub image_id: ImageId,
    pub path: String,
    pub source_format: Option<String>,
    pub size_bytes: Option<u64>,
    /// Open in this process.
    pub loaded: bool,
    /// Registered in the active case.
    pub in_case: bool,
    /// Browsing from case indexes only; the source bytes are missing.
    pub index_only: bool,
}

/// A node an address resolved to: its evidence and the names and IDs from
/// the evidence root down to it (empty for the root itself).
#[derive(Clone, Debug)]
pub struct Target {
    pub evidence: Evidence,
    pub location: Vec<(String, String)>,
}

impl Target {
    pub fn image_id(&self) -> &ImageId {
        &self.evidence.image_id
    }

    /// The node's ID; `""` is the evidence root.
    pub fn node_id(&self) -> &str {
        self.location.last().map_or("", |(_, id)| id.as_str())
    }

    /// The parent directory's node ID; `None` at the evidence root.
    pub fn parent_id(&self) -> Option<&str> {
        self.location
            .len()
            .checked_sub(2)
            .map(|index| self.location[index].1.as_str())
            .or_else(|| (!self.location.is_empty()).then_some(""))
    }

    pub fn address(&self) -> String {
        cli_path(&self.evidence.alias, &self.location)
    }

    pub fn name(&self) -> &str {
        self.location
            .last()
            .map_or(self.evidence.alias.as_str(), |(name, _)| name.as_str())
    }
}

/// The address of a node: the alias, then the evidence root's child by its
/// ID (stable and ASCII, unlike the localized volume names) and the names
/// below it.
pub fn cli_path(alias: &str, location: &[(String, String)]) -> String {
    let mut path = format!("/{alias}");
    for (index, (name, id)) in location.iter().enumerate() {
        path.push('/');
        if index == 0 && !id.is_empty() && !id.contains('/') {
            path.push_str(id);
        } else {
            path.push_str(name);
        }
    }
    path
}

/// An alias a person can type: letters, digits, `_`, `-` and `.`, starting
/// with a letter or digit.
pub fn validate_alias(alias: &str) -> Result<(), CliError> {
    let valid = !alias.is_empty()
        && alias.chars().count() <= MAX_ALIAS
        && alias.chars().next().is_some_and(char::is_alphanumeric)
        && alias
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-' | '.'));
    if valid {
        Ok(())
    } else {
        Err(CliError::new(
            "usage",
            format!("别名 {alias} 无效：1–64 个字母、数字、_、- 或 .，以字母或数字开头"),
        ))
    }
}

/// The alias an evidence path suggests: its file name up to the first dot,
/// with separators folded to `-`.
pub fn default_alias(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    let stem = name
        .split('.')
        .find(|part| !part.is_empty())
        .unwrap_or(name);
    let mut slug = String::new();
    for character in stem.chars() {
        if character.is_alphanumeric() || matches!(character, '_' | '-') {
            slug.push(character);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches(['-', '_']).chars().take(40).collect();
    let slug = slug.trim_end_matches(['-', '_']).to_owned();
    if slug.is_empty() {
        "evidence".to_owned()
    } else {
        slug
    }
}

/// `preferred`, or `preferred-2`, `preferred-3`… when another evidence item
/// already has it (aliases compare case-insensitively).
fn unique(preferred: &str, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(preferred) {
        return preferred.to_owned();
    }
    (2..)
        .map(|number| format!("{preferred}-{number}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| preferred.to_owned())
}

fn alias_key(path: &str) -> String {
    let identity = evidence_path_key(path);
    format!(
        "{ALIAS_KEY}{}",
        hex::encode(Sha256::digest(identity.as_bytes()))
    )
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| super::command_error(error).in_operation(name)
}

impl Exec {
    pub fn state(&self) -> State<'_, crate::state::AppState> {
        State::new(&self.state)
    }

    pub async fn case_dir(&self) -> Option<String> {
        self.state
            .active_case
            .lock()
            .await
            .as_ref()
            .map(|case| case.dir.to_string_lossy().into_owned())
    }

    async fn stored_aliases(&self) -> Result<HashMap<String, String>, CliError> {
        if self.state.active_case.lock().await.is_none() {
            return Ok(HashMap::new());
        }
        let entries = crate::commands::casework::case::case_option_query(
            self.state(),
            Some(ALIAS_KEY.to_owned()),
        )
        .await
        .map_err(operation("case_option_query"))?;
        Ok(entries
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect())
    }

    async fn store_aliases(&self, entries: Vec<CaseOption>) {
        if entries.is_empty() {
            return;
        }
        if let Err(error) =
            crate::commands::casework::case::case_option_save(self.state(), entries).await
        {
            tracing::warn!("save evidence aliases: {}", error.message);
        }
    }

    /// Every evidence item: open in this process or registered in the active
    /// case, each with its alias, sorted by alias. Aliases missing so far are
    /// assigned and, for case evidence, saved.
    pub async fn evidence(&self) -> Result<Vec<Evidence>, CliError> {
        let loaded = crate::commands::evidence::image::image_list(self.state())
            .await
            .map_err(operation("image_list"))?;
        let registered = crate::commands::casework::case::case_evidence_list(self.state())
            .await
            .map_err(operation("case_evidence_list"))?;
        let stored = self.stored_aliases().await?;
        let mut items: Vec<Evidence> = Vec::new();
        for evidence in &registered {
            items.push(Evidence {
                alias: String::new(),
                image_id: ImageId::from(evidence.image_id.clone()),
                path: evidence.path.clone(),
                source_format: serde_json::to_value(evidence.source_format)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned)),
                size_bytes: Some(evidence.size_bytes),
                loaded: false,
                in_case: true,
                index_only: false,
            });
        }
        for info in &loaded {
            let image_id = ImageId::from(info.id.clone());
            match items.iter_mut().find(|item| item.image_id == image_id) {
                Some(item) => {
                    item.loaded = true;
                    item.index_only = info.index_only;
                }
                None => items.push(Evidence {
                    alias: String::new(),
                    image_id,
                    path: info.path.clone(),
                    source_format: serde_json::to_value(info.source_format)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned)),
                    size_bytes: Some(info.size_bytes),
                    loaded: true,
                    in_case: false,
                    index_only: info.index_only,
                }),
            }
        }
        let known = self.workspace.aliases();
        // Keep aliases already given, then name the rest in a stable order.
        for item in &mut items {
            if let Some(named) = known.get(&item.image_id) {
                item.alias = named.alias.clone();
            } else if item.in_case {
                if let Some(alias) = stored.get(&alias_key(&item.path)) {
                    item.alias = alias.clone();
                }
            }
        }
        let mut unnamed: Vec<usize> = (0..items.len())
            .filter(|index| items[*index].alias.is_empty())
            .collect();
        unnamed.sort_by(|left, right| items[*left].path.cmp(&items[*right].path));
        let mut save = Vec::new();
        for index in unnamed {
            let preferred = default_alias(&items[index].path);
            let taken = |candidate: &str| {
                items
                    .iter()
                    .any(|item| item.alias.eq_ignore_ascii_case(candidate))
            };
            let alias = unique(&preferred, &taken);
            if items[index].in_case {
                save.push(CaseOption {
                    key: alias_key(&items[index].path),
                    value: alias.clone(),
                });
            }
            items[index].alias = alias;
        }
        self.store_aliases(save).await;
        for item in &items {
            if !known.contains_key(&item.image_id) {
                self.workspace
                    .set(item.image_id.clone(), item.alias.clone(), item.in_case);
            }
        }
        items.sort_by_cached_key(|item| item.alias.to_lowercase());
        Ok(items)
    }

    /// The evidence an alias names, opened when it is case evidence not yet
    /// open in this process.
    pub async fn resolve(&self, alias: &str) -> Result<Evidence, CliError> {
        let items = self.evidence().await?;
        let Some(mut evidence) = items
            .iter()
            .find(|item| item.alias.eq_ignore_ascii_case(alias))
            .cloned()
        else {
            let known: Vec<String> = items
                .iter()
                .map(|item| format!("/{}", item.alias))
                .collect();
            return Err(CliError::new(
                "notFound",
                if known.is_empty() {
                    format!(
                        "没有名为 {alias} 的检材：当前没有打开任何检材；用 evidence add <检材路径> 或 -i <检材路径> 打开"
                    )
                } else {
                    format!("没有名为 {alias} 的检材；可用：{}", known.join("、"))
                },
            ));
        };
        if !evidence.loaded {
            let info = self.load(&evidence.path, None).await?;
            evidence.loaded = true;
            evidence.index_only = info.index_only;
        }
        Ok(evidence)
    }

    /// `alias` when given, else the only evidence item.
    pub async fn resolve_default(&self, alias: Option<&str>) -> Result<Evidence, CliError> {
        if let Some(alias) = alias {
            return self.resolve(alias.trim_start_matches('/')).await;
        }
        let items = self.evidence().await?;
        match items.as_slice() {
            [only] => self.resolve(&only.alias.clone()).await,
            [] => Err(CliError::new(
                "noImage",
                "没有打开任何检材：用 evidence add <检材路径> 或 -i <检材路径> 打开",
            )),
            many => Err(CliError::new(
                "usage",
                format!(
                    "有多个检材，请指定别名：{}",
                    many.iter()
                        .map(|item| item.alias.as_str())
                        .collect::<Vec<_>>()
                        .join("、")
                ),
            )),
        }
    }

    /// Open `path` (registered in the active case when there is one).
    pub async fn load(
        &self,
        path: &str,
        options: Option<LoadOptions>,
    ) -> Result<crate::core::evidence::image::ImageInfo, CliError> {
        let options = options.unwrap_or_default();
        let case_dir = self.case_dir().await;
        let foreground = self.background.foreground();
        let loaded = crate::commands::evidence::image::image_load(
            self.host.clone(),
            self.state(),
            path.to_owned(),
            options.qcow_password,
            options.qcow_key_hex,
            None,
            case_dir,
            options.raw_read_mode,
        )
        .await
        .map_err(operation("image_load"));
        foreground.finish(&loaded);
        loaded
    }

    /// Open evidence and name it: `alias` when given (it must be free),
    /// otherwise the alias it already has or one its path suggests.
    pub async fn add(
        &self,
        path: &str,
        alias: Option<&str>,
        options: LoadOptions,
    ) -> Result<(Evidence, crate::core::evidence::image::ImageInfo), CliError> {
        if let Some(alias) = alias {
            validate_alias(alias)?;
        }
        let info = self.load(path, Some(options)).await?;
        let image_id = ImageId::from(info.id.clone());
        if let Some(alias) = alias {
            let items = self.evidence().await?;
            if let Some(other) = items
                .iter()
                .find(|item| item.alias.eq_ignore_ascii_case(alias) && item.image_id != image_id)
            {
                return Err(CliError::new(
                    "alreadyExists",
                    format!("别名 {alias} 已用于 {}", other.path),
                ));
            }
            self.workspace.set(image_id.clone(), alias.to_owned(), true);
            let registered = items
                .iter()
                .any(|item| item.image_id == image_id && item.in_case);
            if registered {
                self.store_aliases(vec![CaseOption {
                    key: alias_key(&info.path),
                    value: alias.to_owned(),
                }])
                .await;
            }
        }
        let evidence = self
            .evidence()
            .await?
            .into_iter()
            .find(|item| item.image_id == image_id)
            .ok_or_else(|| super::internal_error("刚打开的检材不在检材列表中"))?;
        Ok((evidence, info))
    }

    /// Name evidence this process derived (an assembled array, a decrypted
    /// container, a shadow copy) after `preferred`, unless it has a name.
    pub async fn adopt(&self, image_id: &ImageId, preferred: &str) -> Result<String, CliError> {
        if let Some(named) = self.workspace.aliases().get(image_id) {
            if named.explicit {
                return Ok(named.alias.clone());
            }
        }
        let items = self.evidence().await?;
        let preferred = default_alias(preferred);
        let taken = |candidate: &str| {
            items.iter().any(|item| {
                item.image_id != *image_id && item.alias.eq_ignore_ascii_case(candidate)
            })
        };
        let alias = unique(&preferred, &taken);
        self.workspace.set(image_id.clone(), alias.clone(), true);
        Ok(alias)
    }

    /// Resolve an address to its node. `/` is not a node; callers that list
    /// the evidence handle it first.
    pub async fn target(&self, address: &Address, last_link: LastLink) -> Result<Target, CliError> {
        match address.parts() {
            Parts::Root => Err(CliError::new(
                "usage",
                "这里需要检材内的路径，例如 /<别名>/part:0/…；evidence ls 列出别名",
            )),
            Parts::Path { alias, path } => {
                let evidence = self.resolve(alias).await?;
                let location = self
                    .locate(&evidence.image_id, path, last_link)
                    .await
                    .map_err(|error| in_address(error, &evidence.alias))?;
                Ok(Target { evidence, location })
            }
            Parts::Node { alias, node_id } => {
                let evidence = self.resolve(alias).await?;
                let location =
                    super::navigation::locate_node(self, &evidence.image_id, node_id).await?;
                Ok(Target { evidence, location })
            }
        }
    }

    /// Resolve `path` from the evidence root.
    pub async fn locate(
        &self,
        image_id: &ImageId,
        path: &str,
        last_link: LastLink,
    ) -> Result<Vec<(String, String)>, CliError> {
        let image = self.image(image_id).await?;
        let mut page = |parent_id: String, cursor, page_size| {
            self.page(image_id, parent_id, cursor, page_size)
        };
        browsing::resolve(&[], path, last_link, &mut page, |parent| {
            super::navigation::case_insensitive(Arc::clone(&image), parent)
        })
        .await
        .map_err(|error| super::msys::explain(error, path))
    }
}

/// A name a directory does not hold, said with the directory's address.
fn in_address(mut error: CliError, alias: &str) -> CliError {
    let Some(details) = error.details.as_ref() else {
        return error;
    };
    let (Some(missing), Some(parent)) = (details["missing"].as_str(), details["parent"].as_array())
    else {
        return error;
    };
    let parent: Vec<(String, String)> = parent
        .iter()
        .filter_map(|pair| Some((pair[0].as_str()?.to_owned(), pair[1].as_str()?.to_owned())))
        .collect();
    let address = cli_path(alias, &parent);
    error.message = format!("{address} 下找不到「{missing}」；用 ls {address} 查看名称");
    error.details = None;
    error
}

/// How evidence is opened.
#[derive(Default)]
pub struct LoadOptions {
    pub qcow_password: Option<String>,
    pub qcow_key_hex: Option<String>,
    pub raw_read_mode: Option<crate::core::evidence::image::RawReadMode>,
}

/// Every evidence item, for `ls /` and `evidence ls`.
pub fn evidence_rows(items: &[Evidence]) -> serde_json::Value {
    json!({
        "path": "/",
        "items": items.iter().map(|item| json!({
            "name": item.alias,
            "path": format!("/{}", item.alias),
            "kind": "evidence",
            "evidencePath": item.path,
            "sourceFormat": item.source_format,
            "size": item.size_bytes,
            "loaded": item.loaded,
            "inCase": item.in_case,
            "indexOnly": item.index_only,
            "imageId": item.image_id,
        })).collect::<Vec<_>>(),
        "nextCursor": null,
        "total": items.len(),
        "truncated": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_come_from_file_names_and_stay_unique() {
        assert_eq!(
            default_alias(r"D:\evidence\nps-2010-emails.E01"),
            "nps-2010-emails"
        );
        assert_eq!(
            default_alias("/data/split evidence.raw.001"),
            "split-evidence"
        );
        assert_eq!(default_alias(r"\\.\PhysicalDrive0"), "PhysicalDrive0");
        assert_eq!(default_alias("assembled://md0"), "md0");
        assert_eq!(default_alias(r"D:\cases\服务器镜像.E01"), "服务器镜像");
        assert_eq!(default_alias("D:/evidence/(1).img"), "1");
        assert_eq!(default_alias("D:/evidence/...."), "evidence");
        let taken = |candidate: &str| {
            ["disk", "disk-2"]
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(candidate))
        };
        assert_eq!(unique("DISK", &taken), "DISK-3");
        assert_eq!(unique("nas", &taken), "nas");
        assert!(validate_alias("nas-01.a").is_ok());
        for invalid in ["", "-nas", "a/b", "a:b", "@x", &"a".repeat(65)] {
            assert!(validate_alias(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn printed_paths_use_the_root_id_and_names_below_it() {
        let location = vec![
            ("分区0".to_owned(), "part:0".to_owned()),
            ("Windows".to_owned(), "ntfs:part:0:42".to_owned()),
        ];
        assert_eq!(cli_path("nas", &location), "/nas/part:0/Windows");
        assert_eq!(cli_path("nas", &[]), "/nas");
        assert_eq!(
            cli_path("dir", &[("root".into(), "dir:a/b".into())]),
            "/dir/root"
        );
    }
}
