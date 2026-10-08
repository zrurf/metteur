//! Verified immutable package contents and host-owned contribution identity.
use super::{
    manifest::{Manifest, package_file},
    runtime::Permission,
    signature::{Policy, Snapshot},
};
use crate::{DaemonError, DaemonResult};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    pub version: String,
    pub fingerprint: String,
    /// "global" or a canonical workspace root; never selected by model input.
    pub scope: String,
    #[serde(default)]
    pub registered_names: Vec<String>,
}
impl Identity {
    pub fn owner(&self) -> String {
        format!("{}::{}", self.scope, self.id)
    }
}

pub(crate) struct Package {
    pub identity: Identity,
    pub manifest: Manifest,
    pub path: PathBuf,
    pub wasm: Arc<Vec<u8>>,
    pub fragments: Vec<metteur_shared::SystemFragment>,
    pub files: Arc<std::collections::BTreeMap<String, Vec<u8>>>,
}
impl Package {
    pub fn load(path: &Path, scope: &str, policy: &Policy) -> DaemonResult<Self> {
        let snapshot = Snapshot::read(path)?;
        snapshot.verify(&policy.trusted_keys, policy.require_signature)?;
        let manifest = Manifest::from_files(&snapshot.files)?;
        let wasm = Arc::new(if manifest.addon.entry.is_empty() {
            vec![]
        } else {
            package_file(&snapshot.files, &manifest.addon.entry)?.to_vec()
        });
        let fragments = manifest
            .fragments
            .iter()
            .map(|fragment| {
                let bytes = package_file(&snapshot.files, &fragment.file)?;
                Ok(metteur_shared::SystemFragment {
                    priority: fragment.priority,
                    scope: fragment.scope.clone(),
                    content: String::from_utf8(bytes.to_vec())
                        .map_err(|_| DaemonError::Addon("fragment is not UTF-8".into()))?,
                })
            })
            .collect::<DaemonResult<Vec<_>>>()?;
        let registered_names = manifest
            .tools
            .iter()
            .map(|tool| format!("tool:{}{}", super::pascal(&manifest.id), tool.name))
            .chain(manifest.functions.iter().map(|function|format!("function:{}{}",super::pascal(&manifest.id),function.name)))
            .chain(
                manifest
                    .nodes
                    .iter()
                    .map(|node| format!("node:{}{}", super::pascal(&manifest.id), node.name)),
            )
            .chain(manifest.hooks.iter().map(|hook| {
                format!("hook:{}{}:{}", super::pascal(&manifest.id), hook.name, hook.event)
            }))
            .chain(manifest.fragments.iter().map(|fragment| format!("fragment:{}", fragment.name)))
            .chain(
                manifest
                    .lsp
                    .iter()
                    .map(|entry| format!("lsp:{}{}", super::pascal(&manifest.id), entry.name)),
            )
            .chain(
                manifest
                    .mcp
                    .iter()
                    .map(|entry| format!("mcp:{}{}", super::pascal(&manifest.id), entry.name)),
            )
            .collect();
        let identity = Identity {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            fingerprint: snapshot.fingerprint(),
            scope: scope.into(),
            registered_names,
        };
        Ok(Self {
            identity,
            manifest,
            path: path.into(),
            wasm,
            fragments,
            files: Arc::new(snapshot.files),
        })
    }
    pub fn permissions(&self, granted: &[String]) -> DaemonResult<HashSet<Permission>> {
        let missing = self.manifest.missing_permissions(granted);
        if !missing.is_empty() {
            return Err(DaemonError::PermissionDenied(format!(
                "addon requires explicit grants for this package fingerprint: {}; use --grant for each selected capability",
                missing.join(", ")
            )));
        }
        granted
            .iter()
            .map(|raw| {
                if !self.manifest.permissions.required.contains(raw) {
                    return Err(DaemonError::PermissionDenied(
                        "grant is not declared by this addon".into(),
                    ));
                }
                Permission::parse(raw)
                    .ok_or_else(|| DaemonError::PermissionDenied("unknown addon grant".into()))
            })
            .collect()
    }
}
