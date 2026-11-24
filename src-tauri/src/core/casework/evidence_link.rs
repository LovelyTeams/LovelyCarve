//! `lovelycarve://` links: a place in a case that the desktop app opens and
//! highlights.
//!
//! ```text
//! lovelycarve://open?case=<case uid>
//! lovelycarve://open?case=<case uid>&evidence=<image id>
//! lovelycarve://open?case=<case uid>&evidence=<image id>&node=<node id>[&offset=<n>&length=<n>]
//! lovelycarve://open?case=<case uid>&evidence=<image id>&root=<root node id>&path=<path>
//! ```
//!
//! A link names its case by the case uid, which survives moving the case
//! directory, and otherwise only places inside that case: it never carries a
//! host path, so opening one reaches nothing but cases registered on the
//! machine that opens it. `offset` and `length` select bytes of the node's
//! content to highlight. Values are percent-encoded UTF-8 with `:`, `/` and
//! `@` left readable; parsing also takes `+` for a space, as form-encoded
//! query strings write it, and skips parameters it does not know.
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

use crate::core::common::ids::{ImageId, NodeId};

/// The URL scheme the desktop app registers.
pub const SCHEME: &str = "lovelycarve";

/// The one action links take: open a place.
const OPEN: &str = "open";

/// The largest offset or length a link carries: the desktop frontend holds
/// both as JavaScript numbers.
pub const MAX_INTEGER: u64 = (1 << 53) - 1;

const MAX_LINK_BYTES: usize = 256 * 1024;
const MAX_ID_BYTES: usize = 1024;
/// Node IDs of folder evidence embed relative paths.
const MAX_NODE_BYTES: usize = 32 * 1024;
/// The longest path a source lookup resolves.
const MAX_PATH_BYTES: usize = 32 * 1024;

/// Bytes a value keeps as they are: the RFC 3986 unreserved characters and
/// the `:`, `/` and `@` of node IDs and paths. Brackets, quotes and the rest
/// are encoded, so links survive Markdown and terminal link detection.
const VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b':')
    .remove(b'/')
    .remove(b'@');

/// A place in a case. [`EvidenceLink::new`] and [`EvidenceLink::parse`]
/// return only links whose values are non-empty, bounded and in range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceLink {
    case_id: String,
    target: LinkTarget,
}

/// What a link opens inside its case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// The case itself.
    Case,
    /// The root of one evidence item.
    Evidence { image_id: ImageId },
    /// A node by its ID, with an optional part of its content to highlight.
    Node {
        image_id: ImageId,
        node_id: NodeId,
        range: Option<ByteRange>,
    },
    /// A node by its path below one of the evidence's root nodes, as analysis
    /// records name their sources.
    Source {
        image_id: ImageId,
        root_id: NodeId,
        path: String,
    },
}

/// Bytes of a node's content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub offset: u64,
    pub length: u64,
}

/// Why a value is not a link this version opens.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("不是 lovelycarve:// 链接")]
    NotALink,
    #[error("链接过长")]
    TooLong,
    #[error("不支持的链接操作：{0}")]
    UnsupportedAction(String),
    #[error("链接缺少参数 {0}")]
    Missing(&'static str),
    #[error("链接参数 {0} 重复")]
    Repeated(&'static str),
    #[error("链接参数 {0} 无效：{1}")]
    Invalid(&'static str, &'static str),
    #[error("链接参数 {0} 需要同时给出 {1}")]
    Requires(&'static str, &'static str),
    #[error("链接参数 {0} 与 {1} 不能同时使用")]
    Conflicts(&'static str, &'static str),
}

impl EvidenceLink {
    /// A link to `target` in the case `case_id`.
    pub fn new(case_id: impl Into<String>, target: LinkTarget) -> Result<Self, LinkError> {
        let link = Self {
            case_id: case_id.into(),
            target,
        };
        link.validate()?;
        Ok(link)
    }

    pub fn case_id(&self) -> &str {
        &self.case_id
    }

    pub fn target(&self) -> &LinkTarget {
        &self.target
    }

    /// Whether `text` starts with the link scheme; [`EvidenceLink::parse`]
    /// still decides whether it is a valid link.
    pub fn has_scheme(text: &str) -> bool {
        text.trim_start()
            .split_once(':')
            .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case(SCHEME))
    }

    /// Read a link, as a person may have pasted it.
    pub fn parse(text: &str) -> Result<Self, LinkError> {
        let text = text.trim();
        if text.len() > MAX_LINK_BYTES {
            return Err(LinkError::TooLong);
        }
        let (scheme, rest) = text.split_once(':').ok_or(LinkError::NotALink)?;
        if !scheme.eq_ignore_ascii_case(SCHEME) {
            return Err(LinkError::NotALink);
        }
        let rest = rest.strip_prefix("//").ok_or(LinkError::NotALink)?;
        let rest = rest.split_once('#').map_or(rest, |(link, _fragment)| link);
        let (action, query) = rest.split_once('?').unwrap_or((rest, ""));
        // Some launchers add a slash after the host.
        let action = action.strip_suffix('/').unwrap_or(action);
        if !action.eq_ignore_ascii_case(OPEN) {
            return Err(LinkError::UnsupportedAction(
                action.chars().take(64).collect(),
            ));
        }
        let mut fields = Fields::default();
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let Ok(name) = decode(name) else {
                continue;
            };
            let Some((name, slot)) = fields.slot(&name) else {
                continue;
            };
            if slot.is_some() {
                return Err(LinkError::Repeated(name));
            }
            *slot = Some(decode(value).map_err(|_| LinkError::Invalid(name, "不是 UTF-8 文本"))?);
        }
        fields.into_link()
    }

    fn validate(&self) -> Result<(), LinkError> {
        check_value("case", &self.case_id, MAX_ID_BYTES)?;
        match &self.target {
            LinkTarget::Case => Ok(()),
            LinkTarget::Evidence { image_id } => check_value("evidence", image_id, MAX_ID_BYTES),
            LinkTarget::Node {
                image_id,
                node_id,
                range,
            } => {
                check_value("evidence", image_id, MAX_ID_BYTES)?;
                check_value("node", node_id, MAX_NODE_BYTES)?;
                match range {
                    None => Ok(()),
                    Some(range) if range.offset > MAX_INTEGER => {
                        Err(LinkError::Invalid("offset", "超出范围"))
                    }
                    Some(range) if range.length == 0 => {
                        Err(LinkError::Invalid("length", "须大于 0"))
                    }
                    Some(range) if range.length > MAX_INTEGER => {
                        Err(LinkError::Invalid("length", "超出范围"))
                    }
                    Some(_) => Ok(()),
                }
            }
            LinkTarget::Source {
                image_id,
                root_id,
                path,
            } => {
                check_value("evidence", image_id, MAX_ID_BYTES)?;
                check_value("root", root_id, MAX_ID_BYTES)?;
                check_value("path", path, MAX_PATH_BYTES)
            }
        }
    }
}

impl std::fmt::Display for EvidenceLink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut url = format!("{SCHEME}://{OPEN}?");
        push(&mut url, "case", &self.case_id);
        match &self.target {
            LinkTarget::Case => {}
            LinkTarget::Evidence { image_id } => push(&mut url, "evidence", image_id),
            LinkTarget::Node {
                image_id,
                node_id,
                range,
            } => {
                push(&mut url, "evidence", image_id);
                push(&mut url, "node", node_id);
                if let Some(range) = range {
                    push(&mut url, "offset", &range.offset.to_string());
                    push(&mut url, "length", &range.length.to_string());
                }
            }
            LinkTarget::Source {
                image_id,
                root_id,
                path,
            } => {
                push(&mut url, "evidence", image_id);
                push(&mut url, "root", root_id);
                push(&mut url, "path", path);
            }
        }
        formatter.write_str(&url)
    }
}

fn push(url: &mut String, name: &str, value: &str) {
    if !url.ends_with('?') {
        url.push('&');
    }
    url.push_str(name);
    url.push('=');
    url.extend(utf8_percent_encode(value, VALUE));
}

fn decode(text: &str) -> Result<String, std::str::Utf8Error> {
    let spaced = text.replace('+', " ");
    percent_decode_str(&spaced)
        .decode_utf8()
        .map(std::borrow::Cow::into_owned)
}

fn check_value(name: &'static str, value: &str, limit: usize) -> Result<(), LinkError> {
    if value.is_empty() {
        Err(LinkError::Missing(name))
    } else if value.len() > limit {
        Err(LinkError::Invalid(name, "过长"))
    } else if value.contains('\0') {
        Err(LinkError::Invalid(name, "含有 NUL 字符"))
    } else {
        Ok(())
    }
}

/// A decimal offset or length, as JavaScript can hold it exactly.
fn integer(name: &'static str, text: &str) -> Result<u64, LinkError> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(LinkError::Invalid(name, "须为十进制整数"));
    }
    text.parse::<u64>()
        .ok()
        .filter(|value| *value <= MAX_INTEGER)
        .ok_or(LinkError::Invalid(name, "超出范围"))
}

/// The parameters a link may carry, as given.
#[derive(Default)]
struct Fields {
    case: Option<String>,
    evidence: Option<String>,
    node: Option<String>,
    root: Option<String>,
    path: Option<String>,
    offset: Option<String>,
    length: Option<String>,
}

impl Fields {
    fn slot(&mut self, name: &str) -> Option<(&'static str, &mut Option<String>)> {
        Some(match name {
            "case" => ("case", &mut self.case),
            "evidence" => ("evidence", &mut self.evidence),
            "node" => ("node", &mut self.node),
            "root" => ("root", &mut self.root),
            "path" => ("path", &mut self.path),
            "offset" => ("offset", &mut self.offset),
            "length" => ("length", &mut self.length),
            _ => return None,
        })
    }

    fn into_link(self) -> Result<EvidenceLink, LinkError> {
        let Self {
            case,
            evidence,
            node,
            root,
            path,
            offset,
            length,
        } = self;
        let case_id = case.ok_or(LinkError::Missing("case"))?;
        let target = match (evidence, node, root, path) {
            (None, None, None, None) => LinkTarget::Case,
            (None, Some(_), _, _) => return Err(LinkError::Requires("node", "evidence")),
            (None, None, Some(_), _) => return Err(LinkError::Requires("root", "evidence")),
            (None, None, None, Some(_)) => return Err(LinkError::Requires("path", "evidence")),
            (Some(image_id), None, None, None) => LinkTarget::Evidence {
                image_id: image_id.into(),
            },
            (Some(_), Some(_), Some(_), _) => return Err(LinkError::Conflicts("node", "root")),
            (Some(_), Some(_), None, Some(_)) => return Err(LinkError::Conflicts("node", "path")),
            (Some(image_id), Some(node_id), None, None) => LinkTarget::Node {
                image_id: image_id.into(),
                node_id: node_id.into(),
                range: None,
            },
            (Some(_), None, Some(_), None) => return Err(LinkError::Requires("root", "path")),
            (Some(_), None, None, Some(_)) => return Err(LinkError::Requires("path", "root")),
            (Some(image_id), None, Some(root_id), Some(path)) => LinkTarget::Source {
                image_id: image_id.into(),
                root_id: root_id.into(),
                path,
            },
        };
        let range = match (offset, length) {
            (None, None) => None,
            (None, Some(_)) => return Err(LinkError::Requires("length", "offset")),
            (Some(offset), length) => Some(ByteRange {
                offset: integer("offset", &offset)?,
                length: length.map_or(Ok(1), |length| integer("length", &length))?,
            }),
        };
        let target = match (target, range) {
            (target, None) => target,
            (
                LinkTarget::Node {
                    image_id, node_id, ..
                },
                Some(range),
            ) => LinkTarget::Node {
                image_id,
                node_id,
                range: Some(range),
            },
            (
                LinkTarget::Case | LinkTarget::Evidence { .. } | LinkTarget::Source { .. },
                Some(_),
            ) => return Err(LinkError::Requires("offset", "node")),
        };
        EvidenceLink::new(case_id, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(range: Option<ByteRange>) -> EvidenceLink {
        EvidenceLink::new(
            "3f2a",
            LinkTarget::Node {
                image_id: "9c1b".into(),
                node_id: "ntfs:part:0:42".into(),
                range,
            },
        )
        .unwrap()
    }

    #[test]
    fn every_target_writes_one_url_and_reads_back() {
        let source = EvidenceLink::new(
            "3f2a",
            LinkTarget::Source {
                image_id: "9c1b".into(),
                root_id: "host-docker:container:ab".into(),
                path: "Users/张三/a b+c&d=(1)'#~.txt".into(),
            },
        )
        .unwrap();
        let cases = [
            (
                EvidenceLink::new("3f2a", LinkTarget::Case).unwrap(),
                "lovelycarve://open?case=3f2a",
            ),
            (
                EvidenceLink::new(
                    "3f2a",
                    LinkTarget::Evidence {
                        image_id: "9c1b".into(),
                    },
                )
                .unwrap(),
                "lovelycarve://open?case=3f2a&evidence=9c1b",
            ),
            (
                node(None),
                "lovelycarve://open?case=3f2a&evidence=9c1b&node=ntfs:part:0:42",
            ),
            (
                node(Some(ByteRange {
                    offset: 256,
                    length: 12,
                })),
                "lovelycarve://open?case=3f2a&evidence=9c1b&node=ntfs:part:0:42&offset=256&length=12",
            ),
            (
                source,
                "lovelycarve://open?case=3f2a&evidence=9c1b&root=host-docker:container:ab\
                 &path=Users/%E5%BC%A0%E4%B8%89/a%20b%2Bc%26d%3D%281%29%27%23~.txt",
            ),
        ];
        for (link, url) in cases {
            assert_eq!(link.to_string(), url);
            assert_eq!(EvidenceLink::parse(url), Ok(link));
        }
    }

    #[test]
    fn links_read_as_people_paste_them() {
        let expected = EvidenceLink::new(
            "3f2a",
            LinkTarget::Source {
                image_id: "9c1b".into(),
                root_id: "part:0".into(),
                path: "a b+c".into(),
            },
        )
        .unwrap();
        for url in [
            "  LovelyCarve://OPEN/?case=3f2a&&evidence=9c1b&root=part:0&path=a+b%2Bc&later=1#top\n",
            "lovelycarve://open?%63ase=3f2a&evidence=9c1b&root=part%3A0&path=a%20b%2bc&x=%FF",
        ] {
            assert_eq!(EvidenceLink::parse(url), Ok(expected.clone()), "{url}");
        }
        assert_eq!(
            EvidenceLink::parse("lovelycarve://open?case=3f2a&evidence=9c1b&node=n&offset=7"),
            Ok(EvidenceLink::new(
                "3f2a",
                LinkTarget::Node {
                    image_id: "9c1b".into(),
                    node_id: "n".into(),
                    range: Some(ByteRange {
                        offset: 7,
                        length: 1
                    }),
                },
            )
            .unwrap())
        );
        assert!(EvidenceLink::has_scheme(" LOVELYCARVE://open"));
        assert!(!EvidenceLink::has_scheme("D:\\evidence\\lovelycarve.E01"));
    }

    #[test]
    fn malformed_links_say_what_is_wrong() {
        let base = "lovelycarve://open?case=c&evidence=e";
        let too_long = format!("{base}&path={}", "a".repeat(MAX_LINK_BYTES));
        let cases: Vec<(String, LinkError)> = vec![
            ("https://example.com/?case=c".into(), LinkError::NotALink),
            ("lovelycarve:open?case=c".into(), LinkError::NotALink),
            (
                "lovelycarve://search?case=c".into(),
                LinkError::UnsupportedAction("search".into()),
            ),
            ("lovelycarve://open".into(), LinkError::Missing("case")),
            (
                "lovelycarve://open?case=".into(),
                LinkError::Missing("case"),
            ),
            (
                "lovelycarve://open?case=c&case=d".into(),
                LinkError::Repeated("case"),
            ),
            (
                "lovelycarve://open?case=c&node=n".into(),
                LinkError::Requires("node", "evidence"),
            ),
            (
                "lovelycarve://open?case=c&path=p".into(),
                LinkError::Requires("path", "evidence"),
            ),
            (
                format!("{base}&node=n&root=r"),
                LinkError::Conflicts("node", "root"),
            ),
            (
                format!("{base}&node=n&path=p"),
                LinkError::Conflicts("node", "path"),
            ),
            (
                format!("{base}&root=r"),
                LinkError::Requires("root", "path"),
            ),
            (
                format!("{base}&path=p"),
                LinkError::Requires("path", "root"),
            ),
            (
                format!("{base}&offset=1"),
                LinkError::Requires("offset", "node"),
            ),
            (
                format!("{base}&root=r&path=p&offset=1"),
                LinkError::Requires("offset", "node"),
            ),
            (
                "lovelycarve://open?case=c&node=n&offset=1".into(),
                LinkError::Requires("node", "evidence"),
            ),
            (
                format!("{base}&node=n&length=1"),
                LinkError::Requires("length", "offset"),
            ),
            (
                format!("{base}&node=n&offset=-1"),
                LinkError::Invalid("offset", "须为十进制整数"),
            ),
            (
                format!("{base}&node=n&offset=0x10"),
                LinkError::Invalid("offset", "须为十进制整数"),
            ),
            (
                format!("{base}&node=n&offset={}", MAX_INTEGER + 1),
                LinkError::Invalid("offset", "超出范围"),
            ),
            (
                format!("{base}&node=n&offset=0&length=0"),
                LinkError::Invalid("length", "须大于 0"),
            ),
            (
                format!("{base}&node=%FF"),
                LinkError::Invalid("node", "不是 UTF-8 文本"),
            ),
            (
                format!("{base}&node=a%00b"),
                LinkError::Invalid("node", "含有 NUL 字符"),
            ),
            (too_long, LinkError::TooLong),
        ];
        for (url, error) in cases {
            assert_eq!(EvidenceLink::parse(&url), Err(error), "{url}");
        }
        assert_eq!(
            EvidenceLink::new(
                "c",
                LinkTarget::Node {
                    image_id: "e".into(),
                    node_id: "".into(),
                    range: None,
                },
            ),
            Err(LinkError::Missing("node"))
        );
    }
}
