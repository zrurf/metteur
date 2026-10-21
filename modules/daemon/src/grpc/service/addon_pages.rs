//! Narrow host-owned page bridge endpoint. The iframe never receives an RPC client.
use super::super::proto::{ResourceRequest, ResourceResponse};
use super::{DaemonService, to_status};
use tonic::{Request, Response, Status};
impl DaemonService {
    pub(crate) async fn addon_pages(
        &self,
        request: Request<ResourceRequest>,
    ) -> Result<Response<ResourceResponse>, Status> {
        let subject =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        if req.schema_version != 1
            || req.request_id.len() > 64
            || uuid::Uuid::parse_str(&req.request_id).is_err()
            || req.payload_json.len() > 16384
        {
            return Err(Status::invalid_argument("Invalid page envelope"));
        }
        let ws = self
            .state
            .workspaces
            .get(std::path::Path::new(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("Workspace closed"))?;

        let host = self
            .state
            .addon_host
            .as_ref()
            .ok_or_else(|| Status::unimplemented("Addon host unavailable"))?;
        let input:crate::addon::ui::PageRequest = serde_json::from_str(&req.payload_json)
            .map_err(|_| Status::invalid_argument("Invalid page operation"))?;
        // Refresh only discovery/admission. Existing instances use immutable
        // package snapshots; lifecycle changes revoke leases in AddonHost.
        if matches!(&input,crate::addon::ui::PageRequest::List|crate::addon::ui::PageRequest::Open{..}) {
            self.state.registry_for(Some(ws.root()), false).await?;
        }
        let value = host.page_request(ws.root(), &subject, input).await.map_err(to_status)?;
        Ok(Response::new(ResourceResponse {
            schema_version: 1,
            payload_json: value.to_string(),
            error_json: String::new(),
        }))
    }
}
