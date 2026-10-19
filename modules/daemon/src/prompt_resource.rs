//! Two-scope prompt files, bounded metadata and recoverable file/index intents.
use std::{path::{Path, PathBuf}, io::Write};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use metteur_shared::resources::{ResourceScope, ResourceRef, ResourceId, MAX_BODY_BYTES};
use crate::storage::{persistence::Db, resources::{ResourceStore, StoredRecord, Result, digest, fault}};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header { pub schema_version: u32, pub id: Uuid, pub name: String, #[serde(default)] pub tags: Vec<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptValue {
    pub header: Header, pub body: String, pub raw_digest: String, pub body_digest: String,
    pub deleted: bool, pub copied_from: Option<ResourceRef>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptEntry { pub reference: ResourceRef, pub content: PromptValue }
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptDraft { pub id: Uuid, pub name: String, #[serde(default)] pub tags: Vec<String>, pub body: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Intent { id: Uuid, expected: u64, before: Option<String>, raw: Option<String>, value: PromptValue, done: bool }

pub fn parse(raw: &[u8]) -> Result<PromptValue> {
    if raw.len() > 64 * 1024 { return Err(fault("resource_limit", "Prompt file exceeds size limit")); }
    let text = std::str::from_utf8(raw).map_err(|_| fault("invalid_prompt", "Prompt must be UTF-8"))?;
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().ok_or_else(|| fault("invalid_prompt", "Missing TOML front matter"))?;
    if first.trim_end_matches(['\r','\n']) != "+++" { return Err(fault("import_draft", "Plain text must be explicitly saved as a new prompt")); }
    let mut offset = first.len(); let mut end = None;
    for line in lines { if line.trim_end_matches(['\r','\n']) == "+++" { end = Some((offset, offset + line.len())); break; } offset += line.len(); }
    let (header_end, body_start) = end.ok_or_else(|| fault("invalid_prompt", "Unterminated TOML front matter"))?;
    let header: Header = toml::from_str(&text[first.len()..header_end]).map_err(|_| fault("invalid_prompt", "Unknown or invalid prompt metadata"))?;
    let body = &text[body_start..];
    if header.schema_version != 1 || header.name.trim().is_empty() || header.name.len() > 240 || header.tags.len() > 20 || header.tags.iter().any(|tag| tag.len() > 64) || body.trim().is_empty() || body.len() > MAX_BODY_BYTES {
        return Err(fault("invalid_prompt", "Prompt metadata or body exceeds limits"));
    }
    Ok(PromptValue { header, body: body.into(), raw_digest: digest(raw), body_digest: digest(body.as_bytes()), deleted:false, copied_from:None })
}

#[derive(Clone)]
pub struct PromptService { pub(crate) db: Db, pub(crate) scope: ResourceScope, directory: PathBuf, store: ResourceStore, intents: ResourceStore }
impl PromptService {
    pub fn project(db: Db, workspace: &Path) -> Result<Self> {
        let root = workspace.canonicalize().map_err(|_| fault("unavailable", "Workspace is unavailable"))?;
        Self::new(db, ResourceScope::Project(root.to_string_lossy().into_owned()), root.join(".metteur/prompts"))
    }
    pub fn personal(db: Db, subject: &str) -> Result<Self> {
        let root = db.directory().parent().ok_or_else(|| fault("unavailable", "User data directory is unavailable"))?.to_path_buf();
        let directory = root.join("prompts").join(digest(subject.as_bytes()));
        Self::new(db, ResourceScope::Personal(subject.into()), directory)
    }
    fn new(db: Db, scope: ResourceScope, directory: PathBuf) -> Result<Self> {
        Ok(Self { store:ResourceStore::new(db.clone(), &scope, "prompts")?, intents:ResourceStore::new(db.clone(), &scope, "prompt_intents")?, db, scope, directory })
    }
    fn safe_path(&self, id: Uuid, create: bool) -> Result<PathBuf> {
        // UUID filenames remove caller-controlled path syntax entirely. Reject
        // symlinks in every existing ancestor, including the prompts directory.
        for path in self.directory.ancestors() {
            if let Ok(meta) = std::fs::symlink_metadata(path) {
                if meta.file_type().is_symlink() { return Err(fault("unsafe_path", "Prompt directory contains a symbolic link")); }
            }
        }
        if create { std::fs::create_dir_all(&self.directory).map_err(|_| fault("storage_unavailable", "Cannot create prompt directory"))?; }
        let path = self.directory.join(format!("{id}.prompt.md"));
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) { return Err(fault("unsafe_path", "Prompt symbolic links are not allowed")); }
        Ok(path)
    }
    fn prepare(&self) -> Result<()> {
        let backup = self.db.directory().parent().ok_or_else(|| fault("unavailable", "Storage directory unavailable"))?.join("nx-before-v1");
        ResourceStore::prepare(&self.db, &backup)
    }
    fn raw(&self, id: Uuid) -> Result<Option<Vec<u8>>> {
        let path = self.safe_path(id, false)?;
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() && meta.len() <= 64 * 1024 => std::fs::read(path).map(Some).map_err(|_| fault("storage_unavailable", "Cannot read prompt file")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            _ => Err(fault("invalid_prompt", "Prompt is not a bounded regular file")),
        }
    }
    fn entry(&self, record: StoredRecord) -> Result<PromptEntry> {
        let content: PromptValue = serde_json::from_value(record.value).map_err(|_| fault("invalid_record", "Prompt index is unreadable"))?;
        Ok(PromptEntry { reference:ResourceRef { scope:self.scope.clone(), id:ResourceId(content.header.id), revision:record.revision, content_digest:content.raw_digest.clone() }, content })
    }
    fn reconcile_file(&self, id: Uuid) -> Result<PromptEntry> {
        let old = self.store.get(id)?;
        let Some(raw) = self.raw(id)? else {
            if let Some(old) = old {
                let mut value = self.entry(old.clone())?.content;
                if !value.deleted { value.deleted = true; self.prepare()?; self.store.write(id, old.revision, Uuid::new_v4(), serde_json::to_value(value).unwrap())?; }
            }
            return Err(fault("unavailable", "Prompt source is missing; history snapshots remain available"));
        };
        let mut parsed = parse(&raw)?;
        if parsed.header.id != id { return Err(fault("identity_mismatch", "Prompt filename and metadata identity disagree")); }
        if let Some(old) = &old {
            let previous = self.entry(old.clone())?;
            if !previous.content.deleted && previous.content.raw_digest == parsed.raw_digest { return Ok(previous); }
            parsed.copied_from = previous.content.copied_from;
        }
        self.prepare()?;
        self.entry(self.store.write(id, old.map_or(0, |old| old.revision), Uuid::new_v4(), serde_json::to_value(parsed).unwrap())?)
    }
    fn recover(&self) -> Result<()> {
        let mut after = None;
        loop {
            let rows = self.intents.list(after, 50)?;
            let count = rows.len();
            for (request, record) in rows {
                after = Some(request);
                let intent: Intent = serde_json::from_value(record.value).map_err(|_| fault("invalid_record", "Prompt intent is unreadable"))?;
                if !intent.done { self.finish(request, record.revision, intent)?; }
            }
            if count < 50 { break; }
        }
        Ok(())
    }
    fn finish(&self, request: Uuid, intent_revision: u64, mut intent: Intent) -> Result<PromptEntry> {
        let current = self.raw(intent.id)?.map(|raw| digest(&raw));
        let target = intent.raw.as_ref().map(|raw| digest(raw.as_bytes()));
        if current != target {
            if current != intent.before { return Err(fault("file_conflict", "Prompt changed during a pending save; recovery requires review")); }
            let path = self.safe_path(intent.id, true)?;
            if let Some(raw) = &intent.raw {
                let temporary = self.directory.join(format!(".{request}.tmp"));
                if temporary.exists() {
                    if std::fs::symlink_metadata(&temporary).map_err(|_| fault("storage_unavailable", "Cannot inspect staged prompt"))?.file_type().is_symlink()
                        || std::fs::read(&temporary).map_err(|_| fault("storage_unavailable", "Cannot inspect staged prompt"))? != raw.as_bytes() {
                        return Err(fault("file_conflict", "Staged prompt differs from its durable intent"));
                    }
                } else {
                    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|_| fault("storage_unavailable", "Cannot stage prompt; pending save retained"))?;
                    file.write_all(raw.as_bytes()).and_then(|_| file.sync_all()).map_err(|_| fault("storage_unavailable", "Cannot persist staged prompt"))?;
                }
                std::fs::rename(&temporary, &path).map_err(|_| fault("storage_unavailable", "Cannot publish staged prompt; pending save retained"))?;
            } else if path.exists() { std::fs::remove_file(&path).map_err(|_| fault("storage_unavailable", "Cannot remove prompt source"))?; }
        }
        let stored = self.store.write(intent.id, intent.expected, request, serde_json::to_value(&intent.value).unwrap())?;
        intent.done = true;
        self.intents.write(request, intent_revision, Uuid::new_v5(&request,b"done"), serde_json::to_value(intent).unwrap())?;
        self.entry(stored)
    }
    pub fn read(&self, id: Uuid) -> Result<PromptEntry> {
        let _guard = self.db.prompt_gate.lock().map_err(|_| fault("storage_unavailable", "Prompt lock unavailable"))?;
        self.recover()?; self.reconcile_file(id)
    }
    pub fn list(&self, after: Option<Uuid>, limit: usize) -> Result<Vec<PromptEntry>> {
        let _guard = self.db.prompt_gate.lock().map_err(|_| fault("storage_unavailable", "Prompt lock unavailable"))?;
        self.safe_path(Uuid::nil(), false)?; self.recover()?;
        if !self.directory.exists() { return Ok(vec![]); }
        let mut ids = Vec::new();
        for (count,item) in std::fs::read_dir(&self.directory).map_err(|_| fault("storage_unavailable", "Cannot list prompts"))?.enumerate() {
            if count >= 1000 { return Err(fault("resource_limit", "Prompt directory exceeds limit")); }
            let item = item.map_err(|_| fault("storage_unavailable", "Cannot read prompt entry"))?;
            let name = item.file_name().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".prompt.md") { ids.push(Uuid::parse_str(stem).map_err(|_| fault("import_draft", "Prompt filenames must be IDs; import this file explicitly"))?); }
        }
        if ids.len() > 1000 { return Err(fault("resource_limit", "Prompt index exceeds limit")); }
        ids.sort(); ids.into_iter().filter(|id| after.is_none_or(|after| *id > after)).take(limit.clamp(1,50)).map(|id| self.reconcile_file(id)).collect()
    }
    pub fn save(&self, draft: PromptDraft, expected: u64, request: Uuid, copied_from: Option<ResourceRef>) -> Result<PromptEntry> {
        let header = Header { schema_version:1, id:draft.id, name:draft.name, tags:draft.tags };
        let raw = format!("+++\n{}+++\n{}", toml::to_string(&header).map_err(|_| fault("invalid_prompt", "Cannot encode metadata"))?, draft.body);
        let mut value = parse(raw.as_bytes())?; value.copied_from = copied_from;
        self.mutate(draft.id, expected, request, Some(raw), value)
    }
    pub fn delete(&self, id: Uuid, expected: u64, request: Uuid) -> Result<PromptEntry> {
        if let Some(receipt)=self.intents.get(request)? {
            let previous:Intent=serde_json::from_value(receipt.value).map_err(|_|fault("invalid_record","Unreadable intent"))?;
            if previous.id!=id || previous.expected!=expected || previous.raw.is_some() {return Err(fault("request_conflict","Request ID is already used"));}
            return self.mutate(id,expected,request,None,previous.value);
        }
        let mut value = self.read(id)?.content; value.deleted = true;
        self.mutate(id, expected, request, None, value)
    }
    fn mutate(&self, id: Uuid, expected: u64, request: Uuid, raw: Option<String>, mut value: PromptValue) -> Result<PromptEntry> {
        let _guard = self.db.prompt_gate.lock().map_err(|_| fault("storage_unavailable", "Prompt lock unavailable"))?;
        self.safe_path(id, false)?; self.prepare()?; self.recover()?;
        if let Some(receipt) = self.intents.get(request)? {
            let previous: Intent = serde_json::from_value(receipt.value).map_err(|_| fault("invalid_record", "Unreadable intent"))?;
            if previous.id != id || previous.expected != expected || previous.raw != raw { return Err(fault("request_conflict", "Request ID is already used")); }
            return self.entry(self.store.revision(id, expected + 1)?.ok_or_else(|| fault("unavailable", "Saved revision unavailable"))?);
        }
        let before = self.raw(id)?.map(|bytes| digest(&bytes));
        let old = self.store.get(id)?;
        if value.copied_from.is_none() { if let Some(record)=&old {value.copied_from=self.entry(record.clone())?.content.copied_from;} }
        if old.is_some() && before.is_none() { return Err(fault("unavailable", "Prompt source disappeared; select or import a new resource")); }
        if before.is_some() { self.reconcile_file(id)?; }
        let actual = self.store.get(id)?.map_or(0, |record| record.revision);
        if actual != expected || old.as_ref().is_some_and(|record| record.revision != actual) {
            return Err(metteur_shared::resources::ResourceError { code:"revision_conflict".into(), message:"Prompt source changed; reload before saving".into(), expected_revision:Some(expected), actual_revision:Some(actual), current_digest:before });
        }
        let intent = Intent { id, expected, before, raw, value, done:false };
        self.intents.write(request, 0, request, serde_json::to_value(&intent).unwrap())?;
        self.finish(request, 1, intent)
    }
}
