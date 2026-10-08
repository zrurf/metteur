//! Registry RPCs: tools, node kinds, MCP servers and addons.

use std::path::PathBuf;

use tonic::{Request, Response, Status};

use super::super::proto::{
    AddonInfo, AddonList, Empty, InstallAddonRequest, ListAddonsRequest, McpServerInfo,
    McpServerList, NodeKindInfo, NodeKindList, SetAddonEnabledRequest, ToolInfo, ToolList,
    UninstallAddonRequest,
};
use super::*;

impl DaemonService {
    pub(crate) async fn list_tools(
        &self,
        request: Request<super::super::proto::RegistryRequest>,
    ) -> Result<Response<ToolList>, Status> {
        let req=request.into_inner();
        let workspace=optional_workspace(&req.workspace_path)?;
        let registry=self.state.registry_for(workspace.as_deref(),false).await?;
        let tools=registry.tools()
            .into_iter()
            .map(|t| ToolInfo {
                name: t.name().to_string(),
                description: t.description().to_string(),
            })
            .collect();
        Ok(Response::new(ToolList {
            tools,
        }))
    }

    pub(crate) async fn list_node_kinds(
        &self,
        request: Request<super::super::proto::RegistryRequest>,
    ) -> Result<Response<NodeKindList>, Status> {
        let workspace=optional_workspace(&request.into_inner().workspace_path)?;
        let registry=self.state.registry_for(workspace.as_deref(),false).await?;
        let catalog = registry.node_signatures();
        let kinds = catalog.keys().cloned().collect();
        let infos = catalog
            .values()
            .map(|signature| NodeKindInfo {
                kind: signature.kind.clone(),
                addon_binding_json: catalog.addon_bindings.get(&signature.kind).map(|v|v.to_string()).unwrap_or_default(),
                node_type: format!("{:?}", signature.node_type),
                pins: signature
                    .pins
                    .iter()
                    .map(|pin| super::super::proto::Pin {
                        id: String::new(),
                        key: pin.key.clone(),
                        name: pin.name.clone(),
                        pin_type: format!("{:?}", pin.pin_type),
                        data_type: pin.data_type.to_string(),
                        default_json: pin
                            .default
                            .as_ref()
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                        optional: pin.optional,
                        choices: pin.choices.clone(),
                        description: pin.description.clone().unwrap_or_default(),
                    })
                    .collect(),
                description: signature.description.clone(),
                dynamic_pins: signature.dynamic_pins,
            })
            .collect();
        Ok(Response::new(NodeKindList {
            kinds,
            infos,
            signature_version: 1,
        }))
    }

    pub(crate) async fn list_mcp_servers(
        &self,
        request: Request<super::super::proto::RegistryRequest>,
    ) -> Result<Response<McpServerList>, Status> {
        let workspace=optional_workspace(&request.into_inner().workspace_path)?;
        self.state.registry_for(workspace.as_deref(),false).await?;
        let mut servers:Vec<_> = match &self.state.mcp_host {
            Some(host) => host
                .statuses()
                .into_iter()
                .map(|status| McpServerInfo {
                    owner:String::new(),
                    scope_root:String::new(),
                    name: status.alias,
                    status: match status.state {
                        crate::integration::mcp::StatusKind::Connected => "Connected".to_string(),
                        crate::integration::mcp::StatusKind::Disabled => "Disabled".to_string(),
                        crate::integration::mcp::StatusKind::Failed => "Failed".to_string(),
                    },
                    tool_count: status.tool_count,
                    error: status.error,
                })
                .collect(),
            None => Vec::new(),
        };
        if let Some(host)=&self.state.addon_host {servers.extend(host.mcp_statuses(workspace.as_deref()).await);}
        Ok(Response::new(McpServerList {
            servers,
        }))
    }

    pub(crate) async fn install_addon(
        &self,
        request: Request<InstallAddonRequest>,
    ) -> Result<Response<AddonInfo>, Status> {
        let Some(host) = &self.state.addon_host else {
            return Err(Status::unimplemented("addon host is not attached"));
        };
        let req = request.into_inner();
        let workspace = optional_workspace(&req.workspace_path)?;
        if let Some(root)=&workspace && self.state.workspaces.get(root).await.is_none() {return Err(Status::not_found("workspace not open"));}
        let info = host
            .install(
                std::path::Path::new(&req.package_path),
                workspace.as_deref(),
                &req.granted_permissions,
            )
            .await
            .map_err(to_status)?;
        self.state.registry_for(workspace.as_deref(),false).await?;
        let roots:Vec<_>=workspace.into_iter().collect();
        let info=host.list(&roots).await.into_iter().find(|item|item.id==info.id && item.scope_root==info.scope_root).unwrap_or(info);
        Ok(Response::new(addon_info_to_proto(info)))
    }

    pub(crate) async fn list_addons(
        &self,
        request: Request<ListAddonsRequest>,
    ) -> Result<Response<AddonList>, Status> {
        let Some(host) = &self.state.addon_host else {
            return Err(Status::unimplemented("addon host is not attached"));
        };
        let requested=request.into_inner().workspace_path;
        let roots: Vec<PathBuf> = if requested.is_empty() {
            self.state.workspaces.list().await.iter().map(|ws| ws.root().to_path_buf()).collect()
        } else {
            let ws=self.state.workspaces.get(&PathBuf::from(requested)).await
                .ok_or_else(||Status::not_found("workspace not open"))?;
            vec![ws.root().to_path_buf()]
        };
        self.state.registry_for(None,false).await?;
        for root in &roots {self.state.registry_for(Some(root),false).await?;}
        let addons = host.list(&roots).await.into_iter().map(addon_info_to_proto).collect();
        Ok(Response::new(AddonList {
            addons,
        }))
    }

    pub(crate) async fn uninstall_addon(
        &self,
        request: Request<UninstallAddonRequest>,
    ) -> Result<Response<Empty>, Status> {
        let Some(host) = &self.state.addon_host else {
            return Err(Status::unimplemented("addon host is not attached"));
        };
        let req = request.into_inner();
        let workspace = optional_workspace(&req.workspace_path)?;
        host.uninstall(&req.id, workspace.as_deref()).await.map_err(to_status)?;
        Ok(Response::new(Empty {}))
    }

    pub(crate) async fn set_addon_enabled(
        &self,
        request: Request<SetAddonEnabledRequest>,
    ) -> Result<Response<Empty>, Status> {
        let Some(host) = &self.state.addon_host else {
            return Err(Status::unimplemented("addon host is not attached"));
        };
        let req = request.into_inner();
        let workspace = optional_workspace(&req.workspace_path)?;
        if req.enabled && let Some(root)=&workspace && self.state.workspaces.get(root).await.is_none() {return Err(Status::not_found("workspace not open"));}
        host.set_enabled(&req.id, req.enabled, workspace.as_deref()).await.map_err(to_status)?;
        if req.enabled {self.state.registry_for(workspace.as_deref(),false).await?;}
        Ok(Response::new(Empty {}))
    }
}
