//! Supervision input identities and bodies commit together, outside rewind.
use super::*;
use crate::storage::resources::digest;
const ID_PREFIX: &[u8] = b"oversight:message-id:";
const HISTORY_PREFIX: &[u8] = b"oversight:conversation:";
const MAX_IDS: usize = 8192;
const MAX_BYTES: usize = 16 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    schema: u32,
    owner: String,
    run: Uuid,
    conversation_id: Uuid,
    digest: String,
    retired: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputReceipt {
    pub schema_version: u32,
    pub id: Uuid,
    pub run: Uuid,
    pub conversation_id: Uuid,
    pub phase: String,
    pub turn: Option<Turn>,
}
fn id_key(id: Uuid) -> Vec<u8> {
    format!("oversight:message-id:{id}").into_bytes()
}
fn identity(db: &Db, id: Uuid) -> DaemonResult<Option<Identity>> {
    let Some(raw) = db.get(cf::EXECUTION_STATE, &id_key(id))? else {
        return Ok(None);
    };
    if raw.len() > 20 * 1024 {
        return Err(DaemonError::Persistence(
            "Supervision identity exceeds its read bound; do not replay".into(),
        ));
    }
    let value = if raw.first() == Some(&b'[') {
        // Historical local-only identities used this tuple. Preserve their
        // meaning without attributing them to a new authenticated owner.
        let (run, conversation_id, text): (Uuid, Uuid, String) =
            serde_json::from_slice(&raw).map_err(|e| DaemonError::Persistence(e.to_string()))?;
        Identity {
            schema: 1,
            owner: "local".into(),
            run,
            conversation_id,
            digest: digest(text.as_bytes()),
            retired: false,
        }
    } else {
        serde_json::from_slice(&raw)
            .map_err(|_| DaemonError::Persistence("Unknown supervision input identity".into()))?
    };
    if value.schema != 1 {
        return Err(DaemonError::Persistence(
            "Unsupported supervision input schema; do not replay".into(),
        ));
    }
    Ok(Some(value))
}
pub(super) fn check_capacity(db: &Db, replacements: &[(Vec<u8>, Vec<u8>)]) -> DaemonResult<()> {
    let mut records = Vec::new();
    for prefix in [ID_PREFIX, HISTORY_PREFIX] {
        let rows = db.resource_scan(prefix, &[], MAX_IDS + 1)?;
        if rows.len() > MAX_IDS {
            return Err(DaemonError::Execution(
                "Supervision input capacity reached; existing messages were kept".into(),
            ));
        }
        records.extend(rows);
    }
    let mut bytes = records.iter().fold(0usize, |sum, (_, raw)| sum.saturating_add(raw.len()));
    let mut count = records.iter().filter(|(key, _)| key.starts_with(ID_PREFIX)).count();
    for (key, value) in replacements {
        if value.len() > 4 * 1024 * 1024 {
            return Err(DaemonError::Execution(
                "Supervision history is full; input was kept".into(),
            ));
        }
        if let Some((_, old)) = records.iter().find(|(old, _)| old == key) {
            bytes = bytes.saturating_sub(old.len());
        } else if key.starts_with(ID_PREFIX) {
            count += 1;
        }
        bytes = bytes.saturating_add(value.len());
    }
    if count > MAX_IDS || bytes > MAX_BYTES {
        return Err(DaemonError::Execution(
            "Supervision input capacity reached; existing messages were kept".into(),
        ));
    }
    Ok(())
}
fn encode(value: &impl Serialize) -> DaemonResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| DaemonError::Serialization(e.to_string()))
}
fn verify(value: &Identity, owner: &str, run: Uuid, conversation_id: Uuid) -> DaemonResult<()> {
    if value.owner != owner || value.run != run || value.conversation_id != conversation_id {
        return Err(DaemonError::PermissionDenied(
            "Input belongs to another owner or supervision conversation".into(),
        ));
    }
    Ok(())
}
/// Missing alone is not proof: retire_if_missing fences the same late input
/// under the gate used by begin_for. No provider, request or approval is run.
pub fn input_receipt(
    db: &Db,
    owner: &str,
    run: Uuid,
    conversation_id: Uuid,
    id: Uuid,
    retire_if_missing: bool,
) -> DaemonResult<InputReceipt> {
    let _guard = db
        .oversight_gate
        .lock()
        .map_err(|_| DaemonError::Persistence("oversight lock poisoned".into()))?;
    if id.is_nil() || run.is_nil() || conversation_id.is_nil() {
        return Err(DaemonError::Execution(
            "A concrete supervision input identity is required".into(),
        ));
    }
    let history = load(db, run)?;
    let turn = history.turns.iter().find(|turn| turn.id == id).cloned();
    let phase = if let Some(value) = identity(db, id)? {
        verify(&value, owner, run, conversation_id)?;
        if value.retired {
            "retired"
        } else if turn.as_ref().is_some_and(|turn| {
            turn.conversation_id == conversation_id
                && digest(turn.original_text.as_bytes()) == value.digest
        }) {
            "accepted"
        } else {
            "unknown"
        }
    } else if turn.is_some() {
        "unknown"
    } else if retire_if_missing {
        let value = Identity {
            schema: 1,
            owner: owner.into(),
            run,
            conversation_id,
            digest: String::new(),
            retired: true,
        };
        let values = vec![(id_key(id), encode(&value)?)];
        check_capacity(db, &values)?;
        db.supervision_input_batch(&values)?;
        "retired"
    } else {
        "missing"
    };
    Ok(InputReceipt {
        schema_version: 1,
        id,
        run,
        conversation_id,
        phase: phase.into(),
        turn: if phase == "accepted" {
            turn
        } else {
            None
        },
    })
}
/// Returns Some only for a previously accepted identity. An accepted identity
/// without its history is unknown and must never start another provider call.
pub fn begin_for(
    db: &Db,
    owner: &str,
    run: Uuid,
    conversation_id: Uuid,
    id: Uuid,
    text: &str,
) -> DaemonResult<Option<Turn>> {
    if id.is_nil()
        || run.is_nil()
        || conversation_id.is_nil()
        || text.trim().is_empty()
        || text.len() > 16384
    {
        return Err(DaemonError::Execution(
            "message must contain 1–16384 bytes and concrete identities".into(),
        ));
    }
    let _guard = db
        .oversight_gate
        .lock()
        .map_err(|_| DaemonError::Persistence("oversight lock poisoned".into()))?;
    let mut history = load(db, run)?;
    if let Some(value) = identity(db, id)? {
        verify(&value, owner, run, conversation_id)?;
        if value.retired {
            return Err(DaemonError::Execution(
                "Input was durably retired; retain its draft".into(),
            ));
        }
        if value.digest != digest(text.as_bytes()) {
            return Err(DaemonError::AlreadyExists("Input id belongs to different content".into()));
        }
        return history
            .turns
            .iter()
            .find(|turn| {
                turn.id == id
                    && turn.conversation_id == conversation_id
                    && turn.original_text == text
            })
            .cloned()
            .map(Some)
            .ok_or_else(|| {
                DaemonError::Persistence(
                    "Accepted supervision input has missing history; do not replay".into(),
                )
            });
    }
    if history.turns.iter().any(|turn| turn.id == id) {
        return Err(DaemonError::Persistence(
            "Supervision identity is missing; do not replay".into(),
        ));
    }
    if history.turns.len() >= 128
        || history
            .turns
            .iter()
            .any(|turn| turn.conversation_id == conversation_id && turn.state == "processing")
    {
        return Err(DaemonError::Execution(
            "Supervision history is full or has an unconfirmed message; inspect its state".into(),
        ));
    }
    let checkpoint = DbCheckpointSink::load(db, run)?
        .ok_or_else(|| DaemonError::NotFound("blueprint run".into()))?;
    if checkpoint.status != RunStatus::Running || requests::load(db, run)?.closed {
        return Err(DaemonError::Execution("run is read-only".into()));
    }
    let value = Identity {
        schema: 1,
        owner: owner.into(),
        run,
        conversation_id,
        digest: digest(text.as_bytes()),
        retired: false,
    };
    history.turns.push(Turn {
        id,
        conversation_id,
        original_text: text.into(),
        at_ms: chrono::Utc::now().timestamp_millis(),
        answer: String::new(),
        state: "processing".into(),
        request_id: None,
        error: None,
    });
    let values = vec![(id_key(id), encode(&value)?), (key(run), encode(&history)?)];
    check_capacity(db, &values)?;
    db.supervision_input_batch(&values)?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::{CheckpointSink, ExecutionCheckpoint};
    fn setup() -> (Db, Uuid) {
        let root = std::env::temp_dir().join(format!("supervision-input-{}", Uuid::new_v4()));
        let db = Db::open(&root).unwrap();
        let run = Uuid::new_v4();
        DbCheckpointSink::new(db.clone(), run)
            .write(&ExecutionCheckpoint::running(run, Uuid::new_v4(), 0))
            .unwrap();
        (db, run)
    }
    #[test]
    fn identity_and_body_are_one_wal_and_a_failed_wal_cannot_be_replayed_after_retirement() {
        let (db, run) = setup();
        let id = Uuid::new_v4();
        db.fail_delivery_write.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(begin_for(&db, "owner", run, run, id, "  raw\r\n中文  ").is_err());
        assert!(identity(&db, id).unwrap().is_none());
        assert!(load(&db, run).unwrap().turns.is_empty());
        assert_eq!(input_receipt(&db, "owner", run, run, id, true).unwrap().phase, "retired");
        assert!(begin_for(&db, "owner", run, run, id, "  raw\r\n中文  ").is_err());
        assert!(load(&db, run).unwrap().turns.is_empty());
    }
    #[test]
    fn owner_run_content_and_exact_retries_are_bound_without_another_turn() {
        let (db, run) = setup();
        let id = Uuid::new_v4();
        assert!(begin_for(&db, "owner", run, run, id, "  raw\r\n中文  ").unwrap().is_none());
        assert!(begin_for(&db, "owner", run, run, id, "  raw\r\n中文  ").unwrap().is_some());
        assert!(begin_for(&db, "owner", run, run, id, "changed").is_err());
        assert!(input_receipt(&db, "another", run, run, id, true).is_err());
        assert!(input_receipt(&db, "owner", Uuid::new_v4(), run, id, true).is_err());
        let receipt = input_receipt(&db, "owner", run, run, id, true).unwrap();
        assert_eq!(receipt.phase, "accepted");
        assert_eq!(receipt.turn.unwrap().original_text, "  raw\r\n中文  ");
        assert_eq!(load(&db, run).unwrap().turns.len(), 1);
    }
    #[test]
    fn accepted_input_and_identity_survive_database_reopen_before_any_provider_call() {
        let (db, run) = setup();
        let id = Uuid::new_v4();
        let path = db.directory().to_path_buf();
        begin_for(&db, "owner", run, run, id, "  raw\r\n中文  ").unwrap();
        drop(db);
        let reopened = Db::open(&path).unwrap();
        let receipt = input_receipt(&reopened, "owner", run, run, id, false).unwrap();
        assert_eq!(receipt.phase, "accepted");
        assert_eq!(receipt.turn.unwrap().original_text, "  raw\r\n中文  ");
        assert!(begin_for(&reopened, "owner", run, run, id, "  raw\r\n中文  ").unwrap().is_some());
        assert!(budget::load(&reopened, run).unwrap().calls.is_empty());
    }
    #[test]
    fn legacy_partial_acceptance_and_future_schema_are_unknown_without_replay() {
        let (db, run) = setup();
        let id = Uuid::new_v4();
        db.put_durable(
            cf::EXECUTION_STATE,
            &id_key(id),
            &encode(&(run, run, "old input")).unwrap(),
        )
        .unwrap();
        assert_eq!(input_receipt(&db, "local", run, run, id, true).unwrap().phase, "unknown");
        assert!(begin(&db, run, run, id, "old input").is_err());
        db.put_durable(cf::EXECUTION_STATE, &id_key(id), b"{\"schema\":9}").unwrap();
        assert!(input_receipt(&db, "local", run, run, id, true).is_err());
        assert!(load(&db, run).unwrap().turns.is_empty());
    }
    #[test]
    fn quota_refusal_keeps_existing_records_and_does_not_admit_a_new_identity() {
        let (db, run) = setup();
        let id = Uuid::new_v4();
        let orphan = key(Uuid::new_v4());
        db.put_durable(cf::EXECUTION_STATE, &orphan, &vec![b'x'; MAX_BYTES]).unwrap();
        assert!(begin_for(&db, "local", run, run, id, "new").is_err());
        assert!(identity(&db, id).unwrap().is_none());
        assert!(load(&db, run).unwrap().turns.is_empty());
        assert_eq!(db.get(cf::EXECUTION_STATE, &orphan).unwrap().unwrap().len(), MAX_BYTES);
    }
    #[test]
    fn acceptance_and_retirement_race_has_one_durable_winner() {
        let (db, run) = setup();
        let id = Uuid::new_v4();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let writer = db.clone();
        let gate = barrier.clone();
        let first = std::thread::spawn(move || {
            gate.wait();
            begin(&writer, run, run, id, "input")
        });
        let reader = db.clone();
        let second = std::thread::spawn(move || {
            barrier.wait();
            input_receipt(&reader, "local", run, run, id, true)
        });
        let accepted = first.join().unwrap().is_ok();
        let receipt = second.join().unwrap().unwrap();
        assert_eq!(
            receipt.phase,
            if accepted {
                "accepted"
            } else {
                "retired"
            }
        );
        assert_eq!(load(&db, run).unwrap().turns.len(), usize::from(accepted));
    }
}
