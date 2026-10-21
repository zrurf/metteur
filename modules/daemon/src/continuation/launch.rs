//! Host-owned proposals. Confirmation binds one immutable dependency manifest.
use super::{handoff::DependencyManifest, state::Terminal};
use crate::{
    DaemonError, DaemonResult,
    storage::{
        persistence::Db,
        resources::{ResourceStore, digest},
    },
};
use metteur_shared::{
    Blueprint,
    resources::{PromptSnapshot, ResourceScope},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaunchState {
    Proposed,
    AwaitingConfirmation,
    Confirmed,
    YieldCommitted,
    Starting,
    Running,
    Terminal,
    Declined,
    Stale,
    NeedsAttention,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchIntent {
    pub schema_version: u32,
    pub id: Uuid,
    pub owner: String,
    pub task: Uuid,
    pub task_generation: u64,
    pub session: Uuid,
    pub turn: Uuid,
    pub handoff: Uuid,
    pub blueprint: Blueprint,
    pub manifest: DependencyManifest,
    pub prompts: BTreeMap<Uuid, PromptSnapshot>,
    pub resume_on_completion: bool,
    pub expires_ms: u64,
    pub state: LaunchState,
    pub confirmation_digest: String,
    pub confirmed_ms: Option<u64>,
    #[serde(default)]
    pub confirmation_request: Option<String>,
    pub run: Option<Uuid>,
    pub terminal: Option<Terminal>,
    pub result: serde_json::Value,
    pub attention: Option<String>,
}
impl LaunchIntent {
    pub fn seal(&mut self) -> DaemonResult<()> {
        self.confirmation_digest = digest(
            &serde_json::to_vec(&serde_json::json!((
                &self.owner,
                self.task,
                self.task_generation,
                self.session,
                self.turn,
                self.handoff,
                &self.blueprint,
                &self.manifest,
                &self.prompts,
                self.resume_on_completion,
                self.expires_ms
            )))
            .map_err(|e| DaemonError::Serialization(e.to_string()))?,
        );
        Ok(())
    }
    pub fn source_id(&self) -> String {
        format!("blueprint:{}", self.id)
    }
}
#[derive(Clone)]
pub struct LaunchStore {
    store: ResourceStore,
}
impl LaunchStore {
    pub fn new(db: Db, root: &Path) -> DaemonResult<Self> {
        let root = root.canonicalize()?;
        std::fs::create_dir_all(root.join(".metteur"))?;
        ResourceStore::prepare(&db, &root.join(".metteur/nx-before-v1"))
            .map_err(|e| DaemonError::Persistence(e.to_string()))?;
        Ok(Self {
            store: ResourceStore::new(
                db,
                &ResourceScope::Project(root.to_string_lossy().into_owned()),
                "launches",
            )
            .map_err(|e| DaemonError::Persistence(e.to_string()))?,
        })
    }
    pub fn get(&self, id: Uuid, owner: &str) -> DaemonResult<(u64, LaunchIntent)> {
        let r = self
            .store
            .get(id)
            .map_err(|e| DaemonError::Persistence(e.to_string()))?
            .ok_or_else(|| DaemonError::NotFound("Launch intent not found".into()))?;
        let value: LaunchIntent = serde_json::from_value(r.value)
            .map_err(|e| DaemonError::Serialization(e.to_string()))?;
        if value.schema_version != 1 || value.owner != owner {
            return Err(DaemonError::PermissionDenied("Launch owner or schema mismatch".into()));
        }
        Ok((r.revision, value))
    }
    pub fn propose(&self, mut value: LaunchIntent) -> DaemonResult<(u64, LaunchIntent)> {
        value.state = LaunchState::AwaitingConfirmation;
        value.seal()?;
        if serde_json::to_vec(&value).unwrap().len() > 2 * 1024 * 1024 {
            return Err(DaemonError::Execution("Launch snapshot exceeds storage bound".into()));
        }
        if let Some(old) =
            self.store.get(value.id).map_err(|e| DaemonError::Persistence(e.to_string()))?
        {
            let old_value: LaunchIntent = serde_json::from_value(old.value)
                .map_err(|e| DaemonError::Serialization(e.to_string()))?;
            if old_value.confirmation_digest != value.confirmation_digest {
                return Err(DaemonError::Execution(
                    "Launch ID already binds a different proposal".into(),
                ));
            }
            return Ok((old.revision, old_value));
        }
        self.put(0, value)
    }
    fn put(&self, revision: u64, value: LaunchIntent) -> DaemonResult<(u64, LaunchIntent)> {
        let request = Uuid::new_v5(
            &value.id,
            format!("revision:{}:{}", revision, serde_json::to_string(&value).unwrap()).as_bytes(),
        );
        let saved = self
            .store
            .write(value.id, revision, request, serde_json::to_value(&value).unwrap())
            .map_err(|e| DaemonError::Execution(e.to_string()))?;
        Ok((saved.revision, value))
    }
    pub fn revise_choices(
        &self,
        id: Uuid,
        owner: &str,
        original_digest: &str,
        resume: bool,
        handoff: Uuid,
    ) -> DaemonResult<LaunchIntent> {
        let (revision, mut value) = self.get(id, owner)?;
        if value.state != LaunchState::AwaitingConfirmation
            || value.confirmation_digest != original_digest
        {
            return Err(DaemonError::Execution(
                "Approval proposal changed; reopen the current request".into(),
            ));
        }
        value.resume_on_completion = resume;
        value.handoff = handoff;
        value.seal()?;
        self.put(revision, value).map(|(_, v)| v)
    }
    pub fn receipt(&self, id: Uuid, owner: &str, request: String) -> DaemonResult<LaunchIntent> {
        let (revision, mut value) = self.get(id, owner)?;
        if value.state != LaunchState::Confirmed {
            return Err(DaemonError::Execution("Launch is not confirmed".into()));
        }
        if value.confirmation_request.as_ref().is_some_and(|old| old != &request) {
            return Err(DaemonError::Execution("Confirmation receipt changed".into()));
        }
        value.confirmation_request = Some(request);
        self.put(revision, value).map(|(_, v)| v)
    }
    pub fn decide(
        &self,
        id: Uuid,
        owner: &str,
        revision: u64,
        approved: bool,
        exact_digest: &str,
        current: &DependencyManifest,
        now: u64,
    ) -> DaemonResult<(u64, LaunchIntent)> {
        let (actual, mut value) = self.get(id, owner)?;
        if value.confirmation_digest != exact_digest {
            return Err(DaemonError::PermissionDenied(
                "Confirmation does not bind this exact proposal".into(),
            ));
        }
        if matches!(
            value.state,
            LaunchState::Confirmed
                | LaunchState::YieldCommitted
                | LaunchState::Starting
                | LaunchState::Running
                | LaunchState::Terminal
        ) && approved
        {
            return Ok((actual, value));
        }
        if actual != revision || value.state != LaunchState::AwaitingConfirmation {
            return Err(DaemonError::Execution(
                "Launch proposal changed; reload before deciding".into(),
            ));
        }
        if !approved {
            value.state = LaunchState::Declined;
            return self.put(actual, value);
        }
        if now >= value.expires_ms || &value.manifest != current {
            value.state = LaunchState::Stale;
            value.attention = Some("Bound version or authorization expired".into());
            self.put(actual, value)?;
            return Err(DaemonError::Execution("Launch is stale; review a new proposal".into()));
        }
        value.state = LaunchState::Confirmed;
        value.confirmed_ms = Some(now);
        self.put(actual, value)
    }
    /// State changes are host-only. A network/model caller never supplies a run ID.
    pub fn transition(
        &self,
        id: Uuid,
        owner: &str,
        expected: LaunchState,
        next: LaunchState,
        run: Option<Uuid>,
        attention: Option<String>,
    ) -> DaemonResult<LaunchIntent> {
        let (revision, mut value) = self.get(id, owner)?;
        if value.state == next && value.run == run {
            return Ok(value);
        }
        if value.state != expected {
            return Err(DaemonError::Execution(
                "Launch state changed; reconcile instead of replaying".into(),
            ));
        }
        if matches!(expected, LaunchState::Terminal | LaunchState::Declined | LaunchState::Stale) {
            return Err(DaemonError::Execution("A terminal launch cannot change state".into()));
        }
        let allowed = matches!(
            (expected, next),
            (LaunchState::Confirmed, LaunchState::YieldCommitted)
                | (LaunchState::YieldCommitted, LaunchState::Starting)
                | (LaunchState::Starting, LaunchState::Running)
        ) || matches!(next, LaunchState::NeedsAttention | LaunchState::Stale);
        if !allowed {
            return Err(DaemonError::Execution("Invalid launch transition".into()));
        }
        if value.run.is_some() && run != value.run {
            return Err(DaemonError::Execution("A launch cannot bind a second run".into()));
        }
        value.state = next;
        value.run = run;
        value.attention = attention;
        self.put(revision, value).map(|(_, v)| v)
    }
    pub fn finish(
        &self,
        id: Uuid,
        owner: &str,
        run: Uuid,
        terminal: Terminal,
        result: serde_json::Value,
    ) -> DaemonResult<LaunchIntent> {
        let (revision, mut value) = self.get(id, owner)?;
        if value.run != Some(run) {
            return Err(DaemonError::Execution("Terminal event belongs to another run".into()));
        }
        if value.state == LaunchState::Terminal {
            if value.terminal == Some(terminal) && value.result == result {
                return Ok(value);
            }
            return Err(DaemonError::Execution("Conflicting run terminal receipt".into()));
        }
        if !matches!(value.state, LaunchState::Starting | LaunchState::Running) {
            return Err(DaemonError::Execution("Run has no valid launch binding".into()));
        }
        if result.to_string().len() > 24 * 1024 {
            return Err(DaemonError::Execution(
                "Run result requires a bounded artifact reference".into(),
            ));
        }
        value.state = LaunchState::Terminal;
        value.terminal = Some(terminal);
        value.result = result;
        self.put(revision, value).map(|(_, v)| v)
    }
    pub fn list(&self, owner: &str, after: Option<Uuid>) -> DaemonResult<Vec<(u64, LaunchIntent)>> {
        self.store
            .list(after, 50)
            .map_err(|e| DaemonError::Persistence(e.to_string()))?
            .into_iter()
            .map(|(_, r)| {
                let v: LaunchIntent = serde_json::from_value(r.value)
                    .map_err(|e| DaemonError::Serialization(e.to_string()))?;
                Ok((r.revision, v))
            })
            .collect::<DaemonResult<Vec<_>>>()
            .map(|rows| rows.into_iter().filter(|(_, v)| v.owner == owner).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Registry, continuation::handoff::DependencyManifest, storage::versioning::VersionRef,
    };
    fn proposal() -> LaunchIntent {
        let blueprint = Blueprint {
            id: Uuid::new_v4(),
            name: "One precise run".into(),
            nodes: vec![],
            edges: vec![],
            entry_node_id: Uuid::new_v4(),
        };
        let manifest = DependencyManifest::freeze(
            &blueprint,
            VersionRef {
                snapshot_id: Uuid::new_v4(),
                blueprint_uri: "blueprints/review.blueprint".into(),
                blob_hash: "a".repeat(64),
            },
            &Registry::with_builtins(),
            &BTreeMap::new(),
        )
        .unwrap();
        LaunchIntent {
            schema_version: 1,
            id: Uuid::new_v4(),
            owner: "owner".into(),
            task: Uuid::new_v4(),
            task_generation: 1,
            session: Uuid::new_v4(),
            turn: Uuid::new_v4(),
            handoff: Uuid::new_v4(),
            blueprint,
            manifest,
            prompts: BTreeMap::new(),
            resume_on_completion: true,
            expires_ms: 10000,
            state: LaunchState::Proposed,
            confirmation_digest: String::new(),
            confirmed_ms: None,
            confirmation_request: None,
            run: None,
            terminal: None,
            result: serde_json::Value::Null,
            attention: None,
        }
    }
    #[test]
    fn approval_is_exact_idempotent_and_never_binds_a_second_run() {
        let root = std::env::temp_dir().join(format!("nx-launch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db = Db::open(&root.join("db")).unwrap();
        let store = LaunchStore::new(db, &root).unwrap();
        let (r, p) = store.propose(proposal()).unwrap();
        assert!(
            store.decide(p.id, "other", r, true, &p.confirmation_digest, &p.manifest, 1).is_err()
        );
        assert!(store.decide(p.id, "owner", r, true, "wrong", &p.manifest, 1).is_err());
        let approved =
            store.decide(p.id, "owner", r, true, &p.confirmation_digest, &p.manifest, 1).unwrap();
        assert_eq!(
            store.decide(p.id, "owner", r, true, &p.confirmation_digest, &p.manifest, 2).unwrap().0,
            approved.0
        );
        store
            .transition(
                p.id,
                "owner",
                LaunchState::Confirmed,
                LaunchState::YieldCommitted,
                None,
                None,
            )
            .unwrap();
        let run = Uuid::new_v4();
        store
            .transition(
                p.id,
                "owner",
                LaunchState::YieldCommitted,
                LaunchState::Starting,
                Some(run),
                None,
            )
            .unwrap();
        assert!(
            store
                .transition(
                    p.id,
                    "owner",
                    LaunchState::Starting,
                    LaunchState::Running,
                    Some(Uuid::new_v4()),
                    None
                )
                .is_err()
        );
        store
            .transition(p.id, "owner", LaunchState::Starting, LaunchState::Running, Some(run), None)
            .unwrap();
        store
            .finish(p.id, "owner", run, Terminal::Succeeded, serde_json::json!({"verified":false}))
            .unwrap();
        assert!(
            store.finish(p.id, "owner", run, Terminal::Failed, serde_json::Value::Null).is_err()
        );
        let (r, p) = store.propose(proposal()).unwrap();
        let mut changed = p.manifest.clone();
        changed.graph_digest = "changed".into();
        assert!(store.decide(p.id, "owner", r, true, &p.confirmation_digest, &changed, 1).is_err());
        assert_eq!(store.get(p.id, "owner").unwrap().1.state, LaunchState::Stale);
    }
}
