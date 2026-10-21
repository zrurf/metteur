//! Registered local preview snapshots. No URL fetching, redirects, shell or model calls.
use crate::{DaemonError, DaemonResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
    pub id: String,
    pub title: String,
    pub revision: u64,
    pub file: String,
}
#[derive(Clone, Serialize)]
pub(crate) struct Snapshot {
    pub target: Target,
    pub digest: String,
    pub html: String,
}
fn fail(s: &str) -> DaemonError {
    DaemonError::Addon(s.into())
}
fn bounded(root: &Path, path: &str, max: u64) -> DaemonResult<Vec<u8>> {
    use std::io::Read;
    let fs = crate::workspace::fs::WorkspaceFs::new(root.to_path_buf());
    let path = fs.resolve_existing(path)?;
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(fail("Preview source must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(fail("Preview source exceeds limit"));
    }
    Ok(bytes)
}
pub(crate) fn list(root: &Path) -> DaemonResult<Vec<Target>> {
    if !root.join(".metteur/preview-targets.json").exists() {
        return Ok(vec![]);
    }
    let targets: Vec<Target> =
        serde_json::from_slice(&bounded(root, ".metteur/preview-targets.json", 16384)?)
            .map_err(|_| fail("Invalid preview target registry"))?;
    let mut seen = std::collections::BTreeSet::new();
    if targets.len() > 32 {
        return Err(fail("Too many preview targets"));
    }
    for t in &targets {
        if t.id.is_empty()
            || t.id.len() > 64
            || !t.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || !seen.insert(&t.id)
            || t.title.is_empty()
            || t.title.len() > 120
            || t.revision == 0
            || !t.file.ends_with(".html")
            || t.file.starts_with(".metteur/")
        {
            return Err(fail("Invalid registered preview target"));
        }
        super::manifest::validate_package_path(&t.file)?;
    }
    Ok(targets)
}
pub(crate) fn snapshot(root: &Path, id: &str) -> DaemonResult<Snapshot> {
    let target = list(root)?
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| fail("Preview target is not registered"))?;
    let bytes = bounded(root, &target.file, 256 * 1024)?;
    let html = String::from_utf8(bytes.clone()).map_err(|_| fail("Preview target is not UTF-8"))?;
    Ok(Snapshot {
        target,
        digest: Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect(),
        html,
    })
}
pub(crate) fn inspect(snapshot: &Snapshot) -> Value {
    let lower = snapshot.html.to_ascii_lowercase();
    // These are explicitly source checks, never a browser/layout correctness claim.
    let checks = vec![
        json!({"name":"Source decoded as UTF-8","passed":true,"kind":"deterministic_source"}),
        json!({"name":"HTML document type declared","passed":lower.contains("<!doctype html"),"kind":"deterministic_source"}),
        json!({"name":"Document title element present","passed":lower.contains("<title>")&&lower.contains("</title>"),"kind":"deterministic_source"}),
        json!({"name":"Viewport metadata declared","passed":regex::Regex::new(r#"(?is)<meta\b[^>]*\bname\s*=\s*[\"']viewport[\"']"#).unwrap().is_match(&snapshot.html),"kind":"deterministic_source"}),
    ];
    json!({"id":uuid::Uuid::new_v4(),"target":snapshot.target,"digest":snapshot.digest,"created_at":chrono::Utc::now().to_rfc3339(),"checks":checks,"screenshot":null,"console":null,"browser_checks":"not_run","model_opinion":null,"model_status":"not_run","human_acceptance":null,"scope":"bounded source checks; not rendered-layout validation"})
}
pub(crate) fn save_report(
    directory: &Path,
    report: &Value,
    subject: &str,
    workspace: &str,
    package: &super::package::Identity,
) -> DaemonResult<()> {
    use std::io::Write;
    std::fs::create_dir_all(directory)?;
    let record = json!({"schema_version":1,"record_id":uuid::Uuid::new_v4(),"subject":subject,"workspace":workspace,"package":package,"report":report});
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join(format!("{}.json", record["record_id"].as_str().unwrap())))?;
    file.write_all(
        serde_json::to_vec_pretty(&record)
            .map_err(|_| fail("Cannot encode inspection report"))?
            .as_slice(),
    )?;
    file.sync_all()?;
    Ok(())
}
