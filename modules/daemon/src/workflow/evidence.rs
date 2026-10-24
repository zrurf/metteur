use crate::storage::resources::digest;
use metteur_shared::workflow::{WorkflowContract, validate_path};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io::Read, path::Path};

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileEvidence {
    pub path: String,
    pub status: String,
    pub digest: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceSnapshot {
    pub contract_digest: String,
    pub files: Vec<FileEvidence>,
}

fn file(root: &Path, path: &str, remaining: &mut u64) -> FileEvidence {
    let mut result = FileEvidence {
        path: path.into(),
        status: "unavailable".into(),
        digest: None,
    };
    if validate_path(path).is_err() {
        result.status = "invalid_path".into();
        return result;
    }
    let mut cursor = root.to_path_buf();
    for component in path.split('/') {
        cursor.push(component);
        match std::fs::symlink_metadata(&cursor) {
            Ok(metadata) if crate::workspace::fs::is_link(&metadata) => {
                result.status = "link_rejected".into();
                return result;
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                result.status = "missing".into();
                return result;
            }
            Err(_) => return result,
        }
    }
    let fs = crate::workspace::fs::WorkspaceFs::new(root.to_path_buf());
    let Ok(resolved) = fs.resolve_existing(path) else {
        return result;
    };
    let Ok(mut handle) = std::fs::File::open(&resolved) else {
        return result;
    };
    let Ok(before) = handle.metadata() else {
        return result;
    };
    if !before.is_file() {
        result.status = "not_a_file".into();
        return result;
    }
    if before.len() > MAX_FILE_BYTES || before.len() > *remaining {
        result.status = "size_limit".into();
        return result;
    }
    let mut bytes = Vec::new();
    if (&mut handle).take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).is_err() {
        return result;
    }
    let Ok(after) = handle.metadata() else {
        return result;
    };
    if bytes.len() as u64 != before.len()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || fs.resolve_existing(path).ok().as_ref() != Some(&resolved)
    {
        result.status = "changed_while_reading".into();
        return result;
    }
    *remaining -= bytes.len() as u64;
    result.digest = Some(digest(&bytes));
    result.status = "available".into();
    result
}

/// Capture only declared evidence paths after a real command returns.
/// Missing or inaccessible bytes remain explicit; they never become an empty-file hash.
pub fn capture(root: &Path, contract: &WorkflowContract) -> EvidenceSnapshot {
    let paths: BTreeSet<_> = contract.scope_paths.iter().chain(&contract.evidence_paths).collect();
    let mut remaining = MAX_TOTAL_BYTES;
    EvidenceSnapshot {
        contract_digest: digest(&serde_json::to_vec(contract).expect("serializable contract")),
        files: paths.into_iter().map(|p| file(root, p, &mut remaining)).collect(),
    }
}
