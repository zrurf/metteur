//! Prompt APIs derive ownership from the authenticated request and workspace.
use tonic::{Request, Response, Status};
use serde::Deserialize;
use uuid::Uuid;
use super::{DaemonService, resources::{decode, request_id, response}, super::proto::{ResourceRequest, ResourceResponse}};
use crate::{prompt_resource::{PromptService, PromptDraft}, storage::resources::{Result, fault}};

#[derive(Deserialize)]
#[serde(tag="operation", rename_all="snake_case", deny_unknown_fields)]
enum Query { List { scope: String, after:Option<Uuid>, limit:Option<usize> }, Read { scope:String, id:Uuid } }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Save { scope: String, draft: PromptDraft }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete { scope:String, id:Uuid }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Copy { reference:metteur_shared::resources::ResourceRef }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Select { reference:metteur_shared::resources::ResourceRef, purpose:metteur_shared::resources::PromptPurpose }
impl DaemonService {
    pub(crate) async fn select_prompt_resource(&self, request:Request<ResourceRequest>) -> std::result::Result<Response<ResourceResponse>,Status> {
        let subject=super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into()); let req=request.into_inner();
        let value=async {
            let input=decode::<Select>(&req)?;
            let scope=if matches!(input.reference.scope,metteur_shared::resources::ResourceScope::Personal(_)){"personal"}else{"project"};
            let entry=self.prompt_service(&req.workspace_path,scope,&subject).await?.read(input.reference.id.0)?;
            if entry.reference != input.reference { return Err(fault("revision_conflict","Review the current source before selecting")); }
            let ws=self.state.workspaces.get(std::path::Path::new(&req.workspace_path)).await.ok_or_else(|| fault("unavailable","Workspace not open"))?;
            Ok(serde_json::to_value(crate::prompt_snapshots::SelectionStore::new(ws.db.clone(),ws.root())?.select(entry,&subject,input.purpose,request_id(&req)?)?).unwrap())
        }.await; Ok(response(value))
    }
    pub(super) async fn selected_prompt(&self, workspace:&str, subject:&str, id:Uuid) -> Result<metteur_shared::resources::PromptSnapshot> {
        let ws=self.state.workspaces.get(std::path::Path::new(workspace)).await.ok_or_else(|| fault("unavailable","Workspace not open"))?;
        let snapshot=crate::prompt_snapshots::SelectionStore::new(ws.db.clone(),ws.root())?.read(id,subject)?;
        let scope=if matches!(snapshot.reference.scope,metteur_shared::resources::ResourceScope::Personal(_)){"personal"}else{"project"};
        let current=self.prompt_service(workspace,scope,subject).await?.read(snapshot.reference.id.0)?;
        if current.reference != snapshot.reference { return Err(fault("revision_conflict","Prompt source changed; review and refresh the selected version")); }
        Ok(snapshot)
    }
    pub(super) async fn prompt_service(&self, workspace:&str, scope:&str, subject:&str) -> Result<PromptService> {
        let ws = self.state.workspaces.get(std::path::Path::new(workspace)).await.ok_or_else(|| fault("unavailable", "Workspace not open"))?;
        match scope {
            "project" => PromptService::project(ws.db.clone(), ws.root()),
            "personal" => PromptService::personal(self.state.global_db.clone().ok_or_else(|| fault("unavailable", "Personal resources require user storage"))?, subject),
            _ => Err(fault("invalid_scope", "Scope must be project or personal")),
        }
    }
    pub(crate) async fn query_prompt_resources(&self, request:Request<ResourceRequest>) -> std::result::Result<Response<ResourceResponse>,Status> {
        let subject = super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into()); let req=request.into_inner();
        let value=async { match decode::<Query>(&req)? {
            Query::List{scope,after,limit} => Ok(serde_json::to_value(self.prompt_service(&req.workspace_path,&scope,&subject).await?.list(after,limit.unwrap_or(20))?).unwrap()),
            Query::Read{scope,id} => Ok(serde_json::to_value(self.prompt_service(&req.workspace_path,&scope,&subject).await?.read(id)?).unwrap()),
        }}.await; Ok(response(value))
    }
    pub(crate) async fn write_prompt_resource(&self, request:Request<ResourceRequest>) -> std::result::Result<Response<ResourceResponse>,Status> {
        let subject=super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into()); let req=request.into_inner();
        let value=async { let input=decode::<Save>(&req)?; Ok(serde_json::to_value(self.prompt_service(&req.workspace_path,&input.scope,&subject).await?.save(input.draft,req.expected_revision,request_id(&req)?,None)?).unwrap()) }.await; Ok(response(value))
    }
    pub(crate) async fn delete_prompt_resource(&self, request:Request<ResourceRequest>) -> std::result::Result<Response<ResourceResponse>,Status> {
        let subject=super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into()); let req=request.into_inner();
        let value=async { let input=decode::<Delete>(&req)?; Ok(serde_json::to_value(self.prompt_service(&req.workspace_path,&input.scope,&subject).await?.delete(input.id,req.expected_revision,request_id(&req)?)?).unwrap()) }.await; Ok(response(value))
    }
    pub(crate) async fn copy_prompt_to_project(&self, request:Request<ResourceRequest>) -> std::result::Result<Response<ResourceResponse>,Status> {
        let subject=super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into()); let req=request.into_inner();
        let value=async {
            let input=decode::<Copy>(&req)?;
            let scope=match &input.reference.scope { metteur_shared::resources::ResourceScope::Personal(_) => "personal", _ => "project" };
            let source=self.prompt_service(&req.workspace_path,scope,&subject).await?.read(input.reference.id.0)?;
            if source.reference != input.reference { return Err(fault("revision_conflict", "Selected source changed or is outside this scope")); }
            let request=request_id(&req)?;
            let target=self.prompt_service(&req.workspace_path,"project",&subject).await?;
            Ok(serde_json::to_value(target.save(PromptDraft{id:Uuid::new_v5(&request,b"copy"),name:source.content.header.name,tags:source.content.header.tags,body:source.content.body},0,request,Some(source.reference))?).unwrap())
        }.await; Ok(response(value))
    }
}
