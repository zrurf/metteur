use super::{
    super::proto::{ResourceRequest, ResourceResponse},
    DaemonService,
    resources::{decode, request_id, response},
};
use crate::{
    execution::DbCheckpointSink,
    storage::resources::{Result, fault},
    workflow::{
        projection::{WorkflowReview, project},
        reviews::{ReviewInput, ReviewStore},
    },
};
use metteur_shared::workflow::{TemplateInput, compile_template, templates};
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use tonic::{Request, Response, Status};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Query {
    Catalog,
    PrepareRun {
        blueprint_id: Uuid,
    },
    Compile {
        input: Box<TemplateInput>,
    },
    Review {
        run_id: Uuid,
    },
    Receipt {
        request_id: Uuid,
    },
}

pub(super) fn confirmation_digest(
    manifest: &crate::continuation::handoff::DependencyManifest,
    config: &metteur_shared::config::Config,
) -> Result<String> {
    // Settings are only hashed; endpoint credentials never enter the response.
    let bytes = serde_json::to_vec(&(manifest, &config.llm, &config.sandbox))
        .map_err(|_| fault("unavailable", "Cannot bind execution settings"))?;
    Ok(crate::storage::resources::digest(&bytes))
}

impl DaemonService {
    fn workflow_run_access(&self, subject: &str) -> Result<()> {
        if !self
            .state
            .acl_store
            .read()
            .unwrap()
            .is_allowed(subject, "/metteur.Daemon/GetExecutionTree")
        {
            return Err(fault("permission_denied", "Current execution access is required"));
        }
        Ok(())
    }
    fn workflow_projection(
        &self,
        ws: &crate::workspace::manager::Workspace,
        subject: &str,
        run_id: Uuid,
    ) -> Result<WorkflowReview> {
        self.workflow_run_access(subject)?;
        let checkpoint = DbCheckpointSink::load(&ws.db, run_id)
            .map_err(|_| fault("storage_unavailable", "Cannot read run checkpoint"))?
            .ok_or_else(|| fault("unavailable", "Run not found in this workspace"))?;
        let mut value = project(ws.root(), &checkpoint)?;
        if super::foreground::unresolved_writes(&ws.db, ws.root())
            .map_err(|_| fault("unknown_effects", "Cannot reconcile background effects"))?
        {
            value.can_review = false;
        }
        Ok(value)
    }
    pub(crate) async fn query_coding_workflows(
        &self,
        request: Request<ResourceRequest>,
    ) -> std::result::Result<Response<ResourceResponse>, Status> {
        let subject =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(Path::new(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("Workspace is not open"))?;
        let value=async {
            match decode::<Query>(&req)? {
                Query::PrepareRun{blueprint_id}=>{
                    self.workflow_run_access(&subject)?;
                    let registry=self.state.registry_for(Some(ws.root()),true).await.map_err(|e|fault("unavailable",&e.to_string()))?;
                    let ctx=super::launches::probe(&self.state,&ws,registry,&subject);
                    let (graph,_,manifest)=crate::registry::tools::launch::manifest(&ctx,blueprint_id).map_err(|e|fault("unavailable",&e.to_string()))?;
                    let contract=metteur_shared::workflow::read_contract(&graph).map_err(|e|fault("invalid_contract",&e.to_string()))?.ok_or_else(||fault("no_contract","No workflow requirements"))?;
                    let config=ws.config.read().await;
                    Ok(json!({"digest":confirmation_digest(&manifest,&config)?,"manifest":manifest,"contract":contract}))
                },
                Query::Catalog=>{
                    let config=ws.config.read().await;
                    let mut models:Vec<_>=config.llm.models.keys().cloned().collect();models.sort();
                    Ok(json!({"templates":templates(),"models":models,"schema_version":1,"review_limits":{"runs":128,"revisions_per_run":64,"record_bytes":16384}}))
                },
                Query::Compile{input}=>{
                    if !ws.config.read().await.llm.models.contains_key(&input.model){return Err(fault("model_unavailable","Select a currently configured model"));}
                    // Templates use existing built-ins. Execution admission may start addon
                    // services, so a read-only compile must not acquire that registry.
                    let graph=compile_template(&input,&self.state.registry.authoring_catalog()).map_err(|e|fault("invalid_template",&e.to_string()))?;
                    let contract=metteur_shared::workflow::read_contract(&graph).map_err(|e|fault("invalid_contract",&e.to_string()))?.ok_or_else(||fault("invalid_contract","Compiled requirements missing"))?;
                    let files=crate::workflow::evidence::capture(ws.root(),&contract).files;
                    Ok(json!({"graph":graph,"preflight":{"executed":false,"model_calls":0,"files":files,"environment":"Commands and external dependencies will be verified when executed. Missing planned output files are warnings, not proof that execution will succeed."}}))
                },
                Query::Review{run_id}=>{
                    let mut projection=self.workflow_projection(&ws,&subject,run_id)?;
                    if self.state.chats.read().await.contains_key(ws.root())||self.state.running.read().await.contains_key(ws.root()){projection.can_review=false;}
                    let mut value=ReviewStore::new(ws.db.clone(),ws.root(),&subject)?.read(projection)?;
                    let anon=crate::observability::anon::Anonymizer::new(&ws.config.read().await.anonymize.extra_patterns);
                    for row in &mut value.criteria {
                        if let Some(output)=&mut row.output {
                            let parsed=serde_json::from_str(output).unwrap_or_else(|_|serde_json::Value::String(output.clone()));
                            *output=anon.anonymize(&crate::execution::blackboard::safe_value(&parsed,0).to_string()).await.chars().take(16*1024).collect();
                        }
                        if let Some(command)=&mut row.command {*command=anon.anonymize(command).await;}
                    }
                    let checkpoint=DbCheckpointSink::load(&ws.db,run_id).map_err(|_|fault("storage_unavailable","Cannot resolve run source"))?.ok_or_else(||fault("unavailable","Run source is unavailable"))?;
                    let source=if let Some(id)=checkpoint.view.workflow_handoff {
                        match crate::continuation::handoff::HandoffStore::new(ws.db.clone(),ws.root()).and_then(|store|store.get(id,&subject)) {
                            Ok(handoff)=>json!({"kind":"chat","session":handoff.session,"task":checkpoint.view.workflow_task}),
                            Err(_)=>json!({"kind":"unavailable"}),
                        }
                    } else {json!({"kind":"independent"})};
                    let mut data=serde_json::to_value(value).unwrap();data["source"]=source;Ok(data)
                },
                Query::Receipt{request_id}=>{
                    self.workflow_run_access(&subject)?;
                    Ok(json!({"request_id":request_id,"receipt":ReviewStore::new(ws.db.clone(),ws.root(),&subject)?.receipt(request_id)?}))
                },
            }
        }.await;
        Ok(response(value))
    }
    pub(crate) async fn record_workflow_review(
        &self,
        request: Request<ResourceRequest>,
    ) -> std::result::Result<Response<ResourceResponse>, Status> {
        let subject =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(Path::new(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("Workspace is not open"))?;
        let value = async {
            if !self
                .state
                .acl_store
                .read()
                .unwrap()
                .is_allowed(&subject, "/metteur.Daemon/RecordWorkflowReview")
            {
                return Err(fault("permission_denied", "Current human review access is required"));
            }
            self.workflow_run_access(&subject)?;
            let input = decode::<ReviewInput>(&req)?;
            let id = request_id(&req)?;
            let store = ReviewStore::new(ws.db.clone(), ws.root(), &subject)?;
            if let Some(receipt) = store.replay(&input, req.expected_revision, id)? {
                return Ok(json!({"receipt":receipt,"request_id":id}));
            }
            if self.state.chats.read().await.contains_key(ws.root())
                || self.state.running.read().await.contains_key(ws.root())
            {
                return Err(fault(
                    "workspace_busy",
                    "Wait for the foreground writer before recording a review",
                ));
            }
            let _admission = ws.admit().await.map_err(|_| {
                fault("workspace_unavailable", "Workspace admission is unavailable")
            })?;
            if self.state.chats.read().await.contains_key(ws.root())
                || self.state.running.read().await.contains_key(ws.root())
            {
                return Err(fault(
                    "workspace_busy",
                    "The workspace became busy; retain the review draft",
                ));
            }
            let projection = self.workflow_projection(&ws, &subject, input.run_id)?;
            let receipt = store.record(input, req.expected_revision, id, &projection)?;
            Ok(json!({"receipt":receipt,"request_id":id}))
        }
        .await;
        Ok(response(value))
    }
}
