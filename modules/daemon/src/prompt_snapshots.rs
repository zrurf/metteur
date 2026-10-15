//! User selections and immutable per-run manifests, independent of source files.
use std::{collections::BTreeMap, path::Path};
use metteur_shared::resources::*;
use uuid::Uuid;
use crate::{prompt_resource::PromptEntry, storage::{persistence::Db, resources::{ResourceStore, Result, fault}}};

pub struct SelectionStore { store: ResourceStore, db: Db, backup: std::path::PathBuf }
impl SelectionStore {
    pub fn new(db: Db, root: &Path) -> Result<Self> {
        let root=root.canonicalize().map_err(|_| fault("unavailable","Workspace unavailable"))?;
        let scope=ResourceScope::Project(root.to_string_lossy().into_owned());
        Ok(Self{store:ResourceStore::new(db.clone(),&scope,"snapshots")?,db,backup:root.join(".metteur/nx-before-v1")})
    }
    pub fn select(&self, entry:PromptEntry, subject:&str, purpose:PromptPurpose, request:Uuid) -> Result<PromptSnapshot> {
        if let Some(previous)=self.store.get(request)? {
            let snapshot:PromptSnapshot=serde_json::from_value(previous.value).map_err(|_| fault("invalid_record","Invalid snapshot"))?;
            if snapshot.reference != entry.reference || snapshot.selected_by != subject || snapshot.purpose != purpose { return Err(fault("request_conflict","Selection request ID is already used")); }
            return Ok(snapshot);
        }
        let snapshot=PromptSnapshot{schema_version:1,id:request,reference:entry.reference,name:entry.content.header.name,source_raw_digest:entry.content.raw_digest,rendered_body_digest:entry.content.body_digest,body:entry.content.body,selected_by:subject.into(),selected_at:chrono::Utc::now().timestamp_millis() as u64,purpose,origin:Origin::UserSelectedPrompt,parent_refs:vec![]};
        ResourceStore::prepare(&self.db,&self.backup)?;
        self.store.write(request,0,request,serde_json::to_value(&snapshot).unwrap())?;
        Ok(snapshot)
    }
    pub fn read(&self, id:Uuid, subject:&str) -> Result<PromptSnapshot> {
        let stored=self.store.get(id)?.ok_or_else(|| fault("unavailable","Selection is not in this workspace"))?;
        let snapshot:PromptSnapshot=serde_json::from_value(stored.value).map_err(|_| fault("invalid_record","Invalid snapshot"))?;
        if snapshot.selected_by != subject { return Err(fault("permission_denied","Selection belongs to another subject")); }
        Ok(snapshot)
    }
    pub fn freeze_run(&self, run:Uuid, selections:&BTreeMap<Uuid,PromptSnapshot>) -> Result<()> {
        if selections.is_empty() { return Ok(()); }
        ResourceStore::prepare(&self.db,&self.backup)?;
        let id=Uuid::new_v5(&run,b"prompt-manifest");
        self.store.write(id,0,id,serde_json::to_value(selections).unwrap())?; Ok(())
    }
    pub fn run_snapshot(&self, run:Uuid, node:Uuid) -> Result<PromptSnapshot> {
        let stored=self.store.get(Uuid::new_v5(&run,b"prompt-manifest"))?.ok_or_else(|| fault("unavailable","Run has no frozen prompt manifest"))?;
        let mut manifest:BTreeMap<Uuid,PromptSnapshot>=serde_json::from_value(stored.value).map_err(|_| fault("invalid_record","Invalid run manifest"))?;
        manifest.remove(&node).ok_or_else(|| fault("permission_denied","Node is not in the frozen run manifest"))
    }
    pub fn record_origin(&self, run:Uuid, node:Uuid, text:&str, refs:Vec<ResourceRef>, direct:bool, equal:bool) -> Result<Origin> {
        let origin=if direct && equal {Origin::UserSelectedPrompt}else{Origin::Derived};
        let body_digest=crate::storage::resources::digest(text.as_bytes());
        let id=Uuid::new_v5(&run,format!("origin:{node}:{body_digest}").as_bytes());
        self.store.write(id,0,id,serde_json::json!({"origin":origin,"parent_refs":refs,"node_id":node,"body_digest":body_digest}))?;
        Ok(origin)
    }
}
pub fn prompt_ancestors(graph:&metteur_shared::Blueprint,node:Uuid,pin:Uuid)->Vec<(Uuid,bool)> {
    let mut queue:Vec<_>=graph.edges.iter().filter(|edge|edge.target_node==node && edge.target_pin==pin).map(|edge|(edge.source_node,true)).collect();
    let mut seen=std::collections::HashSet::new();let mut found=Vec::new();
    while let Some((id,direct))=queue.pop() {
        if !seen.insert(id) || seen.len()>256 {continue;}
        if graph.nodes.iter().any(|node|node.id==id && node.kind=="PromptResource") {found.push((id,direct));}
        else {queue.extend(graph.edges.iter().filter(|edge|edge.target_node==id).map(|edge|(edge.source_node,false)));}
    } found
}
pub fn user_text(snapshot:&PromptSnapshot) -> String {
    format!("[User-selected prompt: {} / {:?} / revision {} / snapshot {}]\n{}",snapshot.name,snapshot.reference.scope,snapshot.reference.revision,snapshot.id,snapshot.body)
}
