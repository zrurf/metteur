//! Workspace memory. Ordinary note writes cannot manufacture execution facts.

use std::path::{Path, PathBuf};
use metteur_shared::resources::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::storage::{persistence::Db, resources::{ResourceStore, Result, StoredRecord, digest, fault}};
use crate::observability::anon::Anonymizer;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileBinding { pub path: String, pub digest: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Confirmation { pub subject: String, pub revision: u64, pub digest: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryValue {
    pub title: String,
    pub body: String,
    pub origin: Origin,
    pub validity: Validity,
    pub author: String,
    pub source: Option<String>,
    pub files: Vec<FileBinding>,
    pub confirmation: Option<Confirmation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry { pub reference: ResourceRef, pub content: MemoryValue }
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteInput { pub id: Uuid, pub title: String, pub body: String, #[serde(default)] pub files: Vec<String> }
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryQuery {
    pub after: Option<Uuid>, pub limit: usize, pub keyword: String,
    pub origin: Option<Origin>, pub validity: Option<Validity>,
    pub source: Option<String>, pub file_digest: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct MemoryPage { pub entries: Vec<MemoryEntry>, pub next: Option<Uuid>, pub truncated: bool }

#[derive(Clone)]
pub struct ProjectMemory { db: Db, root: PathBuf, scope: ResourceScope, store: ResourceStore }
impl ProjectMemory {
    pub fn new(db: Db, root: &Path) -> Result<Self> {
        let root = root.canonicalize().map_err(|_| fault("workspace_unavailable", "Workspace is unavailable"))?;
        let scope = ResourceScope::Project(root.to_string_lossy().into_owned());
        let store = ResourceStore::new(db.clone(), &scope, "memory")?;
        Ok(Self { db, root, scope, store })
    }
    fn prepare(&self) -> Result<()> { ResourceStore::prepare(&self.db, &self.root.join(".metteur/nx-before-v1")) }
    fn entry(&self, id: Uuid, record: StoredRecord) -> Result<MemoryEntry> {
        let mut content: MemoryValue = serde_json::from_value(record.value).map_err(|_| fault("invalid_record", "Memory record is unreadable"))?;
        if content.validity == Validity::Current && content.files.iter().any(|file| self.file_binding(&file.path).map_or(true, |current| current.digest != file.digest)) {
            content.validity = Validity::NeedsReview;
        }
        Ok(MemoryEntry { reference: ResourceRef { scope: self.scope.clone(), id: ResourceId(id), revision: record.revision, content_digest: record.content_digest }, content })
    }
    fn file_binding(&self, input: &str) -> Result<FileBinding> {
        let fs = crate::workspace::fs::WorkspaceFs::new(self.root.clone());
        let path = fs.resolve_existing(input).map_err(|_| fault("invalid_path", "File must be inside the project"))?;
        if path.components().any(|component| component.as_os_str().to_string_lossy().eq_ignore_ascii_case(".metteur")) {
            return Err(fault("invalid_path", "Metadata cannot be referenced as project evidence"));
        }
        if std::fs::metadata(&path).map_err(|_| fault("unavailable", "File is unavailable"))?.len() > 1024 * 1024 {
            return Err(fault("resource_limit", "File binding exceeds the one MiB limit"));
        }
        let raw = fs.read(input).map_err(|_| fault("unavailable", "File is unavailable"))?;
        Ok(FileBinding { path: path.strip_prefix(fs.root()).map_err(|_| fault("invalid_path", "File is outside workspace"))?.to_string_lossy().into_owned(), digest: digest(&raw) })
    }
    pub fn read(&self, id: Uuid) -> Result<MemoryEntry> {
        self.entry(id, self.store.get(id)?.ok_or_else(|| fault("unavailable", "Memory entry not found in this project"))?)
    }
    pub fn query(&self, query: MemoryQuery) -> Result<MemoryPage> {
        if query.keyword.len() > 256 { return Err(fault("resource_limit", "Search is too long")); }
        let limit = if query.limit == 0 { 20 } else { query.limit.min(MAX_QUERY_RESULTS) };
        let rows = self.store.list(query.after, limit)?;
        let mut bytes = 0; let mut entries = Vec::new(); let mut next = None; let mut truncated = rows.len() == limit;
        for (id, record) in rows {
            let entry = self.entry(id, record)?;
            let value = &entry.content;
            let matches = query.origin.is_none_or(|origin| origin == value.origin)
                && query.validity.is_none_or(|validity| validity == value.validity)
                && query.source.as_ref().is_none_or(|source| value.source.as_ref() == Some(source))
                && query.file_digest.as_ref().is_none_or(|hash| value.files.iter().any(|file| &file.digest == hash))
                && format!("{} {}", value.title, value.body).to_lowercase().contains(&query.keyword.to_lowercase());
            if matches {
                let size = serde_json::to_vec(&entry).map_err(|_| fault("invalid_record", "Cannot serialize memory"))?.len();
                if bytes + size > MAX_QUERY_BYTES { truncated = true; break; }
                bytes += size; entries.push(entry);
            }
            next = Some(id);
        }
        Ok(MemoryPage { entries, next: if truncated { next } else { None }, truncated })
    }
    pub async fn write_note(&self, role: ResourceRole, subject: &str, source: Option<String>, request: Uuid, expected: u64, input: NoteInput) -> Result<MemoryEntry> {
        if !role.can_write_note() { return Err(fault("permission_denied", "This role cannot write project notes")); }
        if input.body.trim().is_empty() || input.body.len() > MAX_BODY_BYTES || input.title.trim().is_empty() || input.title.len() > 240 || input.files.len() > 16 {
            return Err(fault("resource_limit", "Note title, body or file bindings exceed limits"));
        }
        if let Some(record) = self.store.get(input.id)? {
            let value: MemoryValue = serde_json::from_value(record.value).map_err(|_| fault("invalid_record", "Memory is unreadable"))?;
            if matches!(value.origin, Origin::ExecutionEvidence | Origin::Handoff) || value.validity == Validity::Tombstone { return Err(fault("read_only", "Host records and tombstones cannot be overwritten")); }
        }
        let anon = Anonymizer::new(&[]);
        let value = MemoryValue {
            title: anon.anonymize(&input.title).await, body: anon.anonymize(&input.body).await,
            origin: if role == ResourceRole::User { Origin::UserNote } else { Origin::ModelOpinion },
            validity: Validity::Current, author: subject.into(), source,
            files: input.files.iter().map(|path| self.file_binding(path)).collect::<Result<_>>()?, confirmation: None,
        };
        self.prepare()?;
        self.entry(input.id, self.store.write(input.id, expected, request, serde_json::to_value(value).map_err(|_| fault("invalid_record", "Cannot serialize note"))?)?)
    }
    pub fn promote(&self, role: ResourceRole, subject: &str, id: Uuid, expected: u64, content_digest: &str, request: Uuid) -> Result<MemoryEntry> {
        if !role.can_promote() { return Err(fault("permission_denied", "Only the user control API can confirm conventions")); }
        let current = self.entry(id, self.store.revision(id, expected)?.ok_or_else(|| fault("revision_conflict", "Selected revision is unavailable"))?)?;
        if current.reference.revision != expected || current.reference.content_digest != content_digest {
            return Err(ResourceError { code: "revision_conflict".into(), message: "Review the current content before confirming".into(), expected_revision: Some(expected), actual_revision: Some(current.reference.revision), current_digest: Some(current.reference.content_digest) });
        }
        let mut value = current.content;
        if value.validity != Validity::Current || matches!(value.origin, Origin::ExecutionEvidence | Origin::Handoff) { return Err(fault("read_only", "Only current notes can become conventions")); }
        value.origin = Origin::UserConvention;
        value.confirmation = Some(Confirmation { subject: subject.into(), revision: expected, digest: content_digest.into() });
        self.prepare()?;
        self.entry(id, self.store.write(id, expected, request, serde_json::to_value(value).unwrap())?)
    }
    pub fn set_status(&self, role: ResourceRole, id: Uuid, expected: u64, request: Uuid, validity: Validity) -> Result<MemoryEntry> {
        if role != ResourceRole::User || !matches!(validity, Validity::Archived | Validity::Tombstone | Validity::Current) { return Err(fault("permission_denied", "Only users can archive, delete or restore notes")); }
        let mut entry = self.entry(id, self.store.revision(id, expected)?.ok_or_else(|| fault("revision_conflict", "Selected revision is unavailable"))?)?;
        if matches!(entry.content.origin, Origin::ExecutionEvidence | Origin::Handoff) || entry.content.validity == Validity::Tombstone { return Err(fault("read_only", "This entry cannot be changed")); }
        entry.content.validity = validity; entry.content.confirmation = None;
        if entry.content.origin == Origin::UserConvention { entry.content.origin = Origin::UserNote; }
        if validity == Validity::Tombstone { entry.content.body.clear(); entry.content.title = "Deleted note".into(); }
        self.prepare()?;
        self.entry(id, self.store.write(id, expected, request, serde_json::to_value(entry.content).unwrap())?)
    }
    /// Evidence is projected from the real checkpoint in this database only.
    pub async fn evidence(&self, run: Uuid, entry_id: Option<String>) -> Result<serde_json::Value> {
        let checkpoint = crate::execution::DbCheckpointSink::load(&self.db, run).map_err(|_| fault("storage_unavailable", "Cannot load evidence"))?.ok_or_else(|| fault("unavailable", "Run not found in this project"))?;
        let projection = crate::execution::blackboard::project(&run.to_string(), &checkpoint.view, &checkpoint.exec_tree, &crate::execution::blackboard::Query { last_n: 20, entry_id, ..Default::default() }, &Anonymizer::new(&[])).await;
        serde_json::to_value(projection).map_err(|_| fault("invalid_record", "Cannot serialize evidence"))
    }
    /// Host handoff creation requires an existing source session in this project.
    pub async fn record_handoff(&self, session: Uuid, request: Uuid, summary: &str) -> Result<MemoryEntry> {
        if summary.is_empty() || summary.len() > MAX_BODY_BYTES { return Err(fault("resource_limit", "Handoff summary exceeds limits")); }
        if crate::chat::session::load_thread(&self.db, &session).map_err(|_| fault("storage_unavailable", "Cannot resolve source session"))?.is_none() {
            return Err(fault("unavailable", "Source session is not in this project"));
        }
        let id = Uuid::new_v5(&session, request.as_bytes());
        let value = MemoryValue { title: "Session handoff".into(), body: Anonymizer::new(&[]).anonymize(summary).await, origin: Origin::Handoff, validity: Validity::Current, author: "host".into(), source: Some(format!("session:{session}")), files: vec![], confirmation: None };
        self.prepare()?;
        self.entry(id, self.store.write(id, 0, request, serde_json::to_value(value).unwrap())?)
    }
    /// Explicit project instructions; conventions never change tool permissions.
    pub async fn confirmed_conventions(&self) -> Result<String> {
        let mut after=None; let mut output=String::new();
        loop {
            let page=self.query(MemoryQuery {after,limit:50,origin:Some(Origin::UserConvention),validity:Some(Validity::Current),..Default::default()})?;
            for entry in page.entries.into_iter().filter(|entry|entry.content.confirmation.is_some()) {
                output.push_str(&format!("[Project convention {} r{}]\n{}\n\n",entry.reference.id.0,entry.reference.revision,entry.content.body));
                if output.len()>MAX_QUERY_BYTES {return Err(fault("resource_limit","Confirmed conventions exceed context budget; archive unused conventions"));}
            }
            after=page.next; if after.is_none(){break;}
        }
        Ok(output)
    }
}
