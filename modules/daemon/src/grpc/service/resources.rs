//! Authenticated resource adapters. User controls are never exposed as tools.
use super::{DaemonService, super::proto::{ResourceRequest, ResourceResponse}};
use tonic::{Request, Response, Status};
use metteur_shared::resources::{ResourceRole, SCHEMA_VERSION, Validity};
use crate::{project_memory::{ProjectMemory, MemoryQuery, NoteInput}, storage::resources::{Result, fault}};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(tag="operation", rename_all="snake_case", deny_unknown_fields)]
enum ReadMemory { Query { #[serde(default)] query: MemoryQuery }, Read { id: Uuid }, Evidence { run_id: Uuid, entry_id: Option<String> } }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Promote { id: Uuid, content_digest: String }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusInput { id: Uuid, validity: Validity }

pub(super) fn response(value: Result<serde_json::Value>) -> Response<ResourceResponse> {
    let (payload_json, error_json) = match value { Ok(value) => (value.to_string(), String::new()), Err(error) => (String::new(), serde_json::to_string(&error).unwrap()) };
    Response::new(ResourceResponse { schema_version: SCHEMA_VERSION, payload_json, error_json })
}
pub(super) fn decode<T: serde::de::DeserializeOwned>(req: &ResourceRequest) -> Result<T> {
    if req.schema_version != SCHEMA_VERSION { return Err(fault("unsupported_schema", "Unsupported resource request schema")); }
    if req.payload_json.len() > 64 * 1024 { return Err(fault("resource_limit", "Resource request is too large")); }
    serde_json::from_str(&req.payload_json).map_err(|_| fault("invalid_request", "Invalid resource request fields"))
}
impl DaemonService {
    pub(super) async fn memory_service(&self, req: &ResourceRequest) -> std::result::Result<ProjectMemory, Status> {
        let ws = self.state.workspaces.get(std::path::Path::new(&req.workspace_path)).await.ok_or_else(|| Status::not_found("workspace not open"))?;
        ProjectMemory::new(ws.db.clone(), ws.root()).map_err(|_| Status::internal("memory unavailable"))
    }
    pub(crate) async fn query_project_memory(&self, request: Request<ResourceRequest>) -> std::result::Result<Response<ResourceResponse>, Status> {
        let req = request.into_inner(); let service = self.memory_service(&req).await?;
        let value = match decode::<ReadMemory>(&req) {
            Ok(ReadMemory::Query { query }) => service.query(query).map(|value| serde_json::to_value(value).unwrap()),
            Ok(ReadMemory::Read { id }) => service.read(id).map(|value| serde_json::to_value(value).unwrap()),
            Ok(ReadMemory::Evidence { run_id, entry_id }) => service.evidence(run_id, entry_id).await,
            Err(error) => Err(error),
        }; Ok(response(value))
    }
    pub(crate) async fn write_project_note(&self, request: Request<ResourceRequest>) -> std::result::Result<Response<ResourceResponse>, Status> {
        let subject = super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner(); let service = self.memory_service(&req).await?;
        let value = async { let input = decode::<NoteInput>(&req)?; let id = request_id(&req)?; service.write_note(ResourceRole::User, &subject, None, id, req.expected_revision, input).await.map(|entry| serde_json::to_value(entry).unwrap()) }.await;
        Ok(response(value))
    }
    pub(crate) async fn promote_project_convention(&self, request: Request<ResourceRequest>) -> std::result::Result<Response<ResourceResponse>, Status> {
        let subject = super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner(); let service = self.memory_service(&req).await?;
        let value = (|| { let input = decode::<Promote>(&req)?; service.promote(ResourceRole::User, &subject, input.id, req.expected_revision, &input.content_digest, request_id(&req)?).map(|entry| serde_json::to_value(entry).unwrap()) })();
        Ok(response(value))
    }
    pub(crate) async fn update_project_memory_status(&self, request: Request<ResourceRequest>) -> std::result::Result<Response<ResourceResponse>, Status> {
        let req = request.into_inner(); let service = self.memory_service(&req).await?;
        let value = (|| { let input = decode::<StatusInput>(&req)?; service.set_status(ResourceRole::User, input.id, req.expected_revision, request_id(&req)?, input.validity).map(|entry| serde_json::to_value(entry).unwrap()) })();
        Ok(response(value))
    }
}
pub(super) fn request_id(req: &ResourceRequest) -> Result<Uuid> { Uuid::parse_str(&req.request_id).map_err(|_| fault("invalid_request", "A UUID request_id is required")) }
