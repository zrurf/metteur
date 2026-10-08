//! Adapt immutable package declarations into the existing LSP manager.
use super::{package::Package, runtime::Permission, services::ServiceContext};
use crate::{
    DaemonError, DaemonResult,
    integration::lsp::{LspManager, OwnedOptions},
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

pub(super) fn languages(
    package: &Package,
    context: &ServiceContext,
) -> Vec<metteur_shared::config::LspLanguageConfig> {
    if context.lsp_disabled {
        return vec![];
    }
    package
        .manifest
        .lsp
        .iter()
        .filter_map(|entry| {
            let declaration = &entry.language;
            // Explicit definitions reserve their language even when user LSP is disabled.
            if context.lsp_config.languages.iter().any(|l| l.id == declaration.id) {
                return None;
            }
            let extensions: Vec<_> = declaration
                .extensions
                .iter()
                .filter(|ext| {
                    !context
                        .lsp_config
                        .languages
                        .iter()
                        .any(|l| l.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
                })
                .cloned()
                .collect();
            if extensions.is_empty() {
                return None;
            }
            Some(metteur_shared::config::LspLanguageConfig {
                id: declaration.id.clone(),
                command: declaration.command.clone(),
                extensions,
            })
        })
        .collect()
}

pub(super) async fn authorize(
    package: &Package,
    permissions: &HashSet<Permission>,
    root: &Path,
    context: &ServiceContext,
) -> DaemonResult<()> {
    for language in languages(package, context) {
        super::services::authorize(&language.command, permissions, root, context).await?;
    }
    Ok(())
}

pub(super) async fn start(
    package: &Package,
    permissions: &HashSet<Permission>,
    root: &Path,
    context: &ServiceContext,
    runtime: &Path,
) -> DaemonResult<Option<Arc<LspManager>>> {
    let mut languages = languages(package, context);
    let mut env = HashMap::new();
    let mut secrets = vec![];
    for language in &mut languages {
        let declaration = &package
            .manifest
            .lsp
            .iter()
            .find(|e| e.language.id == language.id)
            .expect("validated language")
            .language;
        language.command = declaration
            .command
            .iter()
            .map(|arg| {
                arg.strip_prefix("${package}/")
                    .map(|p| runtime.join(p).to_string_lossy().into_owned())
                    .unwrap_or_else(|| arg.clone())
            })
            .collect();
        #[cfg(unix)]
        if declaration.command[0].starts_with("${package}/") {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&language.command[0], std::fs::Permissions::from_mode(0o700))?;
        }
        let mut values = HashMap::new();
        for (name, reference) in &declaration.env_refs {
            if !permissions.contains(&Permission::Environment) {
                return Err(DaemonError::Addon("Addon LSP environment grant required".into()));
            }
            let value = std::env::var(reference).map_err(|_| {
                DaemonError::Addon("Addon LSP environment reference unavailable".into())
            })?;
            if value.len() > 16384 {
                return Err(DaemonError::Addon(
                    "Addon LSP environment reference exceeds limit".into(),
                ));
            }
            secrets.push(value.clone());
            values.insert(name.clone(), value);
        }
        env.insert(language.id.clone(), values);
    }
    let config = metteur_shared::config::LspConfig {
        enabled: true,
        languages,
        debounce_ms: context.lsp_config.debounce_ms,
        check_on_node_end: context.lsp_config.check_on_node_end,
    };
    let manager = LspManager::owned(
        &config,
        root,
        OwnedOptions {
            env,
            secrets,
            timeout_ms: package.manifest.call_timeout_ms(5000).clamp(1, 30_000),
        },
    );
    if let Some(manager) = &manager {
        let timeout = std::time::Duration::from_millis(
            package.manifest.call_timeout_ms(5000).clamp(1, 30_000),
        );
        let result =
            tokio::time::timeout(timeout, manager.initialize_all()).await.unwrap_or_else(|_| {
                Err(DaemonError::Addon("Addon LSP initialization timed out".into()))
            });
        if let Err(error) = result {
            manager.shutdown().await;
            return Err(error);
        }
    }
    Ok(manager)
}
