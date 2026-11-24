//! The `lovelycarve://` link of the active case or of a place in it
//! (`core::casework::evidence_link`).
use serde::Deserialize;

use crate::{
    app::error::AppError,
    core::{
        casework::evidence_link::{ByteRange, EvidenceLink, LinkTarget},
        common::ids::{ImageId, NodeId},
    },
    host::State,
    state::AppState,
};

/// A place in one evidence item of the active case, as the desktop locates a
/// file: a node by ID (with an optional byte range of its content), a path
/// below one of the evidence's root nodes, or neither for the evidence root.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkPlace {
    pub image_id: ImageId,
    #[serde(default)]
    pub node_id: Option<NodeId>,
    #[serde(default)]
    pub root_id: Option<NodeId>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub length: Option<u64>,
}

impl LinkPlace {
    fn into_target(self) -> Result<LinkTarget, AppError> {
        let range = match (self.offset, self.length) {
            (None, None) => None,
            (Some(offset), length) => Some(ByteRange {
                offset,
                length: length.unwrap_or(1),
            }),
            (None, Some(_)) => return Err(AppError::invalid_input("length needs an offset")),
        };
        match (self.node_id, self.root_id, self.path, range) {
            (Some(node_id), None, None, range) => Ok(LinkTarget::Node {
                image_id: self.image_id,
                node_id,
                range,
            }),
            (None, Some(root_id), Some(path), None) => Ok(LinkTarget::Source {
                image_id: self.image_id,
                root_id,
                path,
            }),
            (None, None, None, None) => Ok(LinkTarget::Evidence {
                image_id: self.image_id,
            }),
            _ => Err(AppError::invalid_input(
                "a place is a node ID with an optional byte range, a root ID with a path, or neither",
            )),
        }
    }
}

/// The link of the active case, or of `place` in one of its evidence items.
pub async fn case_link(
    state: State<'_, AppState>,
    place: Option<LinkPlace>,
) -> Result<String, AppError> {
    let case_id = {
        let guard = state.active_case.lock().await;
        let case = guard
            .as_ref()
            .ok_or_else(|| AppError::invalid_input("no active case"))?;
        if let Some(place) = &place {
            if !case.has_evidence(&place.image_id) {
                return Err(AppError::not_found(format!(
                    "evidence {} is not in the active case",
                    place.image_id
                )));
            }
        }
        case.profile().case_uid.clone()
    };
    let target = match place {
        Some(place) => place.into_target()?,
        None => LinkTarget::Case,
    };
    EvidenceLink::new(case_id, target)
        .map(|link| link.to_string())
        .map_err(|error| AppError::invalid_input(error.to_string()))
}
