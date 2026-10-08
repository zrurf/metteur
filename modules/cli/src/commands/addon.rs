//! Addon install/uninstall/list/enable command handlers.

use metteur_proto::proto::daemon_client::DaemonClient;
use metteur_proto::proto::{
    InstallAddonRequest, ListAddonsRequest, SetAddonEnabledRequest, UninstallAddonRequest,
};
use tonic::transport::Channel;

use super::*;
use crate::print;

/// Handles `addons`: lists installed addons.
pub(crate) async fn handle_addons(client: &mut DaemonClient<Channel>) -> anyhow::Result<Outcome> {
    let list = client.list_addons(ListAddonsRequest::default()).await.map_err(status)?.into_inner();
    Ok(Outcome::Printed(print::addons(&list)))
}

/// Installs with explicit command-line grants; package declarations confer none.
pub(crate) async fn handle_install_addon(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    path: String,
    workspace: Option<String>,
    granted: Vec<String>,
) -> anyhow::Result<Outcome> {
    let request = InstallAddonRequest {
        package_path: path,
        workspace_path: resolve_scope(workspace, state)?,
        granted_permissions: granted,
    };
    let info = client.install_addon(request).await.map_err(status)?.into_inner();
    Ok(Outcome::Printed(format!(
        "installed {} v{} ({} tool(s), {} fragment(s)); status={}{}",
        info.id, info.version, info.tool_count, info.fragment_count,info.status,if info.error.is_empty(){String::new()}else{format!("; {}",info.error)}
    )))
}

/// Handles `uninstall <id> [ws|global]`.
pub(crate) async fn handle_uninstall_addon(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    id: String,
    workspace: Option<String>,
) -> anyhow::Result<Outcome> {
    let request = UninstallAddonRequest {
        id: id.clone(),
        workspace_path: resolve_scope(workspace, state)?,
    };
    client.uninstall_addon(request).await.map_err(status)?;
    Ok(Outcome::Printed(format!("uninstalled {id}")))
}

/// Handles `addon <id> on|off`.
pub(crate) async fn handle_set_addon_enabled(
    client: &mut DaemonClient<Channel>,
    state: &SessionState,
    id: String,
    on: bool,
    workspace: Option<String>,
) -> anyhow::Result<Outcome> {
    let request = SetAddonEnabledRequest {
        id: id.clone(),
        workspace_path: resolve_scope(workspace, state)?,
        enabled: on,
    };
    client.set_addon_enabled(request).await.map_err(status)?;
    Ok(Outcome::Printed(format!(
        "{id} is now {}",
        if on {
            "on"
        } else {
            "off"
        }
    )))
}

/// Resolves an addon scope token to a `workspace_path` field value.
fn resolve_scope(workspace: Option<String>, state: &SessionState) -> anyhow::Result<String> {
    match workspace.as_deref() {
        None => Ok(String::new()),
        Some("ws" | "workspace") => require_ws(state),
        Some(other) => Ok(other.to_string()),
    }
}

#[cfg(test)]
mod addon_tests {
    use super::*;
    #[test]
    fn install_defaults_to_no_grants_and_preserves_explicit_selection() {
        assert_eq!(parse("install package.zip").unwrap(),Command::InstallAddon {
            path:"package.zip".into(),workspace:None,granted:vec![],
        });
        assert_eq!(parse("install package.zip ws --grant fs:read --grant tools --grant tools").unwrap(),Command::InstallAddon {
            path:"package.zip".into(),workspace:Some("ws".into()),granted:vec!["fs:read".into(),"tools".into()],
        });
    }
    #[test]
    fn install_rejects_missing_or_implicit_authorization_options() {
        for command in ["install", "install package.zip --grant", "install package.zip --all", "install package.zip --grant --all", "install package.zip global extra"] {
            assert!(parse(command).is_err(),"{command}");
        }
    }
}
