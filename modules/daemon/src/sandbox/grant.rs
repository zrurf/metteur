//! Persistent storage for sandbox grants.

use crate::error::{DaemonError, DaemonResult};
use crate::storage::persistence::{Db, cf};

use super::approval::{Decision, Scope};

/// Looks up and stores persistent (workspace/global) grants.
#[derive(Clone, Default)]
pub struct GrantStore {
    workspace: Option<Db>,
    global: Option<Db>,
}

impl GrantStore {
    /// Creates a store over the optional workspace and global databases.
    pub fn new(workspace: Option<Db>, global: Option<Db>) -> Self {
        Self {
            workspace,
            global,
        }
    }

    /// Looks up a persistent grant for the command hash.
    ///
    /// Workspace-scoped grants take precedence over global ones.
    pub fn lookup(&self, command_hash: u64) -> Option<bool> {
        let key = command_hash.to_be_bytes();
        for db in [&self.workspace, &self.global] {
            let Some(db) = db else {
                continue;
            };
            let Ok(Some(raw)) = db.get(cf::GRANTS, &key) else {
                continue;
            };
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&raw) {
                return Some(v.get("decision").and_then(|d| d.as_str()) == Some("allow"));
            }
        }
        None
    }

    /// Persists a grant for the given scope.
    pub fn store(
        &self,
        scope: Scope,
        command_hash: u64,
        decision: Decision,
        command: &str,
    ) -> DaemonResult<()> {
        let (db, scope_name) = match scope {
            Scope::Workspace => (&self.workspace, "workspace"),
            Scope::Global => (&self.global, "global"),
            Scope::Once | Scope::Run => return Ok(()),
        };
        let Some(db) = db else {
            return Err(DaemonError::Sandbox(format!("{scope_name} approval storage unavailable")));
        };
        let value = serde_json::json!({
            "decision": if decision == Decision::Allow { "allow" } else { "deny" },
            "command": command,
            "created_at": now_millis(),
        });
        db.put_durable(cf::GRANTS, &command_hash.to_be_bytes(), value.to_string().as_bytes())
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> Db {
        let dir = std::env::temp_dir().join(format!("metteur-grants-{}", uuid::Uuid::new_v4()));
        Db::open(&dir).unwrap()
    }

    #[test]
    fn stores_and_lookups_with_workspace_precedence() {
        let ws = temp_db();
        let global = temp_db();
        let store = GrantStore::new(Some(ws.clone()), Some(global.clone()));

        assert_eq!(store.lookup(7), None);
        store.store(Scope::Global, 7, Decision::Allow, "git status").unwrap();
        assert_eq!(store.lookup(7), Some(true));

        store.store(Scope::Workspace, 7, Decision::Deny, "git status").unwrap();
        assert_eq!(store.lookup(7), Some(false));

        // Run/Once scopes are never persisted.
        store.store(Scope::Run, 8, Decision::Allow, "x").unwrap();
        assert_eq!(store.lookup(8), None);
    }

    #[test]
    fn missing_databases_reject_persistent_grants() {
        let store = GrantStore::new(None, None);
        assert_eq!(store.lookup(1), None);
        assert!(store.store(Scope::Workspace, 1, Decision::Allow, "x").is_err());
    }
    #[test]
    fn accepted_persistent_grant_survives_database_reopen() {
        let root = std::env::temp_dir().join(format!("dw-durable-grant-{}", uuid::Uuid::new_v4()));
        let db = Db::open(&root).unwrap();
        let store = GrantStore::new(Some(db.clone()), None);
        store.store(Scope::Workspace, 19, Decision::Deny, "exact command").unwrap();
        drop(store);
        drop(db);
        let reopened = GrantStore::new(Some(Db::open(&root).unwrap()), None);
        assert_eq!(reopened.lookup(19), Some(false));
    }
}
