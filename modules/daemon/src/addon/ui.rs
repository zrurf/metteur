//! Host-owned Addon page leases. No plugin-supplied owner, workspace, URL,
//! approval, RPC method or filesystem path is accepted by the bridge.
use super::package::{Identity, Package};
use crate::{DaemonError, DaemonResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};
use uuid::Uuid;
fn fail(message: &str) -> DaemonError {
    DaemonError::PermissionDenied(message.into())
}

#[cfg(test)]
mod model_evidence_tests {
    use super::*;
    #[test]
    fn model_text_cannot_accept_grant_replace_checks_or_survive_revocation() {
        let root = std::env::temp_dir().join(format!("nx-opinion-binding-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".metteur")).unwrap();
        std::fs::write(root.join("preview.html"), "<!doctype html><title>Synthetic review</title>").unwrap();
        std::fs::write(root.join(".metteur/preview-targets.json"), r#"[{"id":"sample","title":"Synthetic","revision":1,"file":"preview.html"}]"#).unwrap();
        let workspace = root.to_string_lossy().into_owned();
        let id = Uuid::new_v4();
        let nonce = Uuid::new_v4();
        let snapshot = super::super::preview_targets::snapshot(&root, "sample").unwrap();
        let report = super::super::preview_targets::inspect(&snapshot);
        let report_id = report["id"].as_str().unwrap().to_owned();
        let mut store = PageStore::default();
        store.leases.insert(id, Lease { identity: Identity {id:"synthetic.page".into(),version:"1.0.0".into(),fingerprint:"pinned-package".into(),scope:workspace.clone(),registered_names:vec![]}, workspace:workspace.clone(),subject:"alice".into(),page:"sample".into(),nonce,selected:Some(snapshot.clone()),report:Some(report),expires:Instant::now()+Duration::from_secs(60),window:Instant::now(),count:0,requests:BTreeSet::new(),capabilities:BTreeSet::new() });
        let evidence = json!({"schema_version":1,"digest":snapshot.digest,"model_status":"completed","human_acceptance":{"accepted":true},"permission_grant":true,"checks":[],"model_opinion":{"kind":"model_source_opinion","model":"deepseek/synthetic","provider":"synthetic","call_id":Uuid::new_v4(),"summary":"Model-authored words are data","findings":[],"permission_grant":true}});
        let result = store.attach_model_opinion((id,nonce),&workspace,"alice",&report_id,evidence.clone(),&root.join("reports")).unwrap();
        assert!(result["human_acceptance"].is_null());
        assert!(result["permission_grant"].is_null());
        assert!(result["model_opinion"]["permission_grant"].is_null());
        assert_eq!(result["checks"].as_array().unwrap().len(),4);
        assert!(store.attach_model_opinion((id,nonce),&workspace,"bob",&report_id,evidence.clone(),&root.join("reports")).is_err());
        std::fs::write(root.join("preview.html"),"changed").unwrap();
        assert!(store.attach_model_opinion((id,nonce),&workspace,"alice",&report_id,evidence.clone(),&root.join("reports")).is_err());
        store.close(id,nonce,&workspace,"alice").unwrap();
        assert!(store.attach_model_opinion((id,nonce),&workspace,"alice",&report_id,evidence,&root.join("reports")).is_err());
    }
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PageRequest {
    List,
    Targets,
    Heartbeat { instance_id: Uuid, nonce: Uuid },
    SelectTarget {
        instance_id: Uuid,
        nonce: Uuid,
        target_id: String,
    },
    Review {
        instance_id: Uuid,
        nonce: Uuid,
        report_id: Uuid,
        expected_digest: String,
        accepted: bool,
    },
    Open {
        package_id: String,
        page_id: String,
        expected_digest: String,
    },
    Message {
        instance_id: Uuid,
        nonce: Uuid,
        message: BridgeMessage,
    },
    Close {
        instance_id: Uuid,
        nonce: Uuid,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BridgeMessage {
    pub bridge_version: u32,
    pub request_id: Uuid,
    pub method: String,
    pub params: Value,
}
#[derive(Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub(crate) enum BridgeCommand {
    Theme(Theme),
    Resize(Size),
    ReadSelected(Empty),
    InspectSelected(Empty),
    ReadReport(Empty),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Theme {
    pub mode: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Size {
    pub height: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Empty {}
struct Lease {
    identity: Identity,
    workspace: String,
    subject: String,
    page: String,
    nonce: Uuid,
    selected: Option<super::preview_targets::Snapshot>,
    report: Option<Value>,
    expires: Instant,
    window: Instant,
    count: u32,
    requests: BTreeSet<Uuid>,
    capabilities: BTreeSet<String>,
}
#[derive(Default)]
pub(crate) struct PageStore {
    leases: BTreeMap<Uuid, Lease>,
}
#[derive(Serialize)]
pub(crate) struct OpenPage {
    pub instance_id: Uuid,
    pub nonce: Uuid,
    pub page_id: String,
    pub package: Identity,
    pub bridge_version: u32,
    pub lease_seconds: u64,
    pub title: String,
    pub html: String,
    pub capabilities: Vec<String>,
    pub csp: String,
}
impl PageStore {
    pub fn open(
        &mut self,
        package: &Package,
        page: &str,
        workspace: &str,
        subject: &str,
    ) -> DaemonResult<OpenPage> {
        self.leases.retain(|_, l| l.expires > Instant::now());
        if self.leases.len() >= 64 {
            return Err(fail("Too many open addon pages"));
        }
        let declaration = package
            .manifest
            .pages
            .iter()
            .find(|p| p.id == page)
            .ok_or_else(|| fail("Page not declared"))?;
        let html =
            std::str::from_utf8(super::manifest::package_file(&package.files, &declaration.entry)?)
                .map_err(|_| fail("Invalid page encoding"))?
                .to_owned();
        let instance_id = Uuid::new_v4();
        let nonce = Uuid::new_v4();
        self.leases.insert(
            instance_id,
            Lease {
                identity: package.identity.clone(),
                workspace: workspace.into(),
                subject: subject.into(),
                page: page.into(),
                nonce,
                selected: None,
                report: None,
                expires: Instant::now() + Duration::from_secs(900),
                window: Instant::now(),
                count: 0,
                requests: BTreeSet::new(),
                capabilities: declaration.capabilities.iter().cloned().collect(),
            },
        );
        Ok(OpenPage{instance_id,nonce,page_id:page.into(),package:package.identity.clone(),bridge_version:1,lease_seconds:900,title:declaration.title.clone(),html,capabilities:declaration.capabilities.clone(),csp:"default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; connect-src 'none'; form-action 'none'; base-uri 'none'; object-src 'none'".into()})
    }
    pub fn revoke_workspace(&mut self, workspace: &str) {
        self.leases.retain(|_, l| l.workspace != workspace);
    }
    pub fn prune(&mut self, identities: &BTreeMap<String, String>) {
        self.leases.retain(|_, l| {
            identities.get(&l.identity.owner()) == Some(&l.identity.fingerprint)
                && l.expires > Instant::now()
        });
    }
    fn lease(
        &mut self,
        id: Uuid,
        nonce: Uuid,
        workspace: &str,
        subject: &str,
    ) -> DaemonResult<&mut Lease> {
        let l = self.leases.get_mut(&id).ok_or_else(|| fail("Page instance revoked or unknown"))?;
        if l.nonce != nonce
            || l.workspace != workspace
            || l.subject != subject
            || l.expires <= Instant::now()
        {
            return Err(fail("Page instance binding expired or mismatched"));
        }
        Ok(l)
    }
    pub fn heartbeat(&mut self,id:Uuid,nonce:Uuid,workspace:&str,subject:&str)->DaemonResult<Value>{
        self.lease(id,nonce,workspace,subject)?;Ok(json!({"valid":true}))
    }
    pub fn close(
        &mut self,
        id: Uuid,
        nonce: Uuid,
        workspace: &str,
        subject: &str,
    ) -> DaemonResult<()> {
        self.lease(id, nonce, workspace, subject)?;
        self.leases.remove(&id);
        Ok(())
    }
    pub fn select_target(
        &mut self,
        id: Uuid,
        nonce: Uuid,
        workspace: &str,
        subject: &str,
        target: &str,
    ) -> DaemonResult<Value> {
        let l = self.lease(id, nonce, workspace, subject)?;
        if !l.capabilities.contains("selected-preview") {
            return Err(fail("Selected preview capability missing"));
        }
        let snapshot = super::preview_targets::snapshot(std::path::Path::new(workspace), target)?;
        let value = json!({"target":snapshot.target,"digest":snapshot.digest});
        l.selected = Some(snapshot);
        l.report = None;
        Ok(value)
    }
    pub fn review(
        &mut self,
        id: Uuid,
        nonce: Uuid,
        workspace: &str,
        subject: &str,
        report_id: Uuid,
        digest: &str,
        accepted: bool,
        reports: &std::path::Path,
    ) -> DaemonResult<Value> {
        let l = self.lease(id, nonce, workspace, subject)?;
        let selected = l.selected.as_ref().ok_or_else(|| fail("No selected target"))?;
        let current =
            super::preview_targets::snapshot(std::path::Path::new(workspace), &selected.target.id)?;
        let report = l.report.as_ref().ok_or_else(|| fail("No inspection report"))?;
        if report["id"].as_str() != Some(report_id.to_string().as_str())
            || current.digest != digest
            || current.digest != selected.digest
            || current.target.revision != selected.target.revision
        {
            return Err(fail("Review does not match current preview revision"));
        }
        let mut next = report.clone();
        next["human_acceptance"] = json!({"accepted":accepted,"subject":subject,"at":chrono::Utc::now().to_rfc3339(),"digest":digest,"permission_grant":false});
        super::preview_targets::save_report(reports, &next, subject, workspace, &l.identity)?;
        l.report = Some(next.clone());
        Ok(next)
    }
    pub fn inspection_snapshot(&mut self, id: Uuid, nonce: Uuid, workspace: &str, subject: &str, report_id: &str) -> DaemonResult<super::preview_targets::Snapshot> {
        let l = self.lease(id, nonce, workspace, subject)?;
        if l.report.as_ref().and_then(|r|r["id"].as_str()) != Some(report_id) { return Err(fail("Inspection superseded")); }
        let selected = l.selected.as_ref().ok_or_else(||fail("Target no longer selected"))?;
        let current = super::preview_targets::snapshot(std::path::Path::new(workspace), &selected.target.id)?;
        if current.digest != selected.digest || current.target.revision != selected.target.revision { return Err(fail("Preview changed during inspection")); }
        Ok(current)
    }
    pub fn attach_evidence(&mut self, id: Uuid, nonce: Uuid, workspace: &str, subject: &str, report_id: &str, evidence: Value, reports: &std::path::Path) -> DaemonResult<Value> {
        let snapshot = self.inspection_snapshot(id, nonce, workspace, subject, report_id)?;
        if evidence["browser_checks"] == "completed" { super::preview_renderer::validate(&evidence, &snapshot.digest).map_err(|e|fail(&e))?; }
        let l = self.lease(id, nonce, workspace, subject)?;
        let mut report = l.report.clone().ok_or_else(||fail("Missing report"))?;
        // Whitelist fields. Renderer cannot set human acceptance, source checks,
        // target identity, model opinion, or permissions.
        for key in ["browser_checks", "browser_error", "screenshot", "console", "metrics", "viewport", "engine", "blocked_requests"] {
            if let Some(value) = evidence.get(key) { report[key] = value.clone(); }
        }
        if evidence["browser_checks"] == "completed" {
            report["scope"] = evidence["scope"].clone();
            if let Some(checks) = report["checks"].as_array_mut() {
                let width = evidence["metrics"]["viewportWidth"].as_u64();
                let scroll = evidence["metrics"]["scrollWidth"].as_u64();
                checks.push(json!({"name":"No horizontal overflow at captured viewport","passed":width.zip(scroll).is_some_and(|(w,s)|s<=w),"kind":"isolated_browser"}));
                checks.push(json!({"name":"Images declare an alt attribute","passed":evidence["metrics"]["imagesWithoutAlt"]==0,"kind":"isolated_browser"}));
            }
        }
        report["human_acceptance"] = Value::Null;
        super::preview_targets::save_report(reports, &report, subject, workspace, &l.identity)?;
        l.report = Some(report.clone());
        Ok(report)
    }
    pub fn attach_model_opinion(&mut self, instance: (Uuid, Uuid), workspace: &str, subject: &str, report_id: &str, evidence: Value, reports: &std::path::Path) -> DaemonResult<Value> {
        let (id, nonce) = instance;
        let snapshot = self.inspection_snapshot(id, nonce, workspace, subject, report_id)?;
        let status = evidence["model_status"].as_str().ok_or_else(||fail("Missing model opinion status"))?;
        if !matches!(status, "completed" | "failed" | "not_configured" | "busy") { return Err(fail("Invalid model opinion status")); }
        if status == "completed" { super::preview_model::validate(&evidence, &snapshot.digest).map_err(fail)?; }
        let l = self.lease(id, nonce, workspace, subject)?;
        let mut report = l.report.clone().ok_or_else(||fail("Missing report"))?;
        report["model_status"] = json!(status);
        report["model_opinion"] = if status == "completed" {
            let opinion = &evidence["model_opinion"];
            // An opinion can carry text and provenance only. Ignore every
            // worker-supplied acceptance, action, check or permission field.
            json!({"kind":"model_source_opinion","model":opinion["model"],"provider":opinion["provider"],"call_id":opinion["call_id"],"summary":opinion["summary"],"findings":opinion["findings"],"scope":"Model opinion on selected source; not browser evidence, human acceptance or a permission grant."})
        } else { Value::Null };
        report["human_acceptance"] = Value::Null;
        super::preview_targets::save_report(reports, &report, subject, workspace, &l.identity)?;
        l.report = Some(report.clone());
        Ok(report)
    }
    pub fn message(
        &mut self,
        id: Uuid,
        nonce: Uuid,
        workspace: &str,
        subject: &str,
        m: BridgeMessage,
        reports: &std::path::Path,
    ) -> DaemonResult<Value> {
        let l = self.lease(id, nonce, workspace, subject)?;
        if m.bridge_version != 1 || l.requests.contains(&m.request_id) {
            return Err(fail("Invalid bridge version or duplicate request"));
        }
        if l.window.elapsed() >= Duration::from_secs(1) {
            l.window = Instant::now();
            l.count = 0;
        }
        if l.count >= 32 || l.requests.len() >= 4096 {
            return Err(fail("Bridge rate or lifetime request limit exceeded"));
        }
        let command: BridgeCommand =
            serde_json::from_value(json!({"method":m.method,"params":m.params}))
                .map_err(|_| fail("Unknown method or invalid parameters"))?;
        let capability = match &command {
            BridgeCommand::Theme(_) => "theme",
            BridgeCommand::Resize(_) => "resize",
            BridgeCommand::ReadSelected(_) => "selected-preview",
            BridgeCommand::InspectSelected(_) => "inspect-preview",
            BridgeCommand::ReadReport(_) => "own-reports",
        };
        if !l.capabilities.contains(capability) {
            return Err(fail("Page capability not declared"));
        }
        l.count += 1;
        l.requests.insert(m.request_id);
        let result = match command {
            BridgeCommand::Theme(t) if matches!(t.mode.as_str(), "light" | "dark") => {
                json!({"mode":t.mode})
            }
            BridgeCommand::Resize(s) if (240..=1600).contains(&s.height) => {
                json!({"height":s.height})
            }
            BridgeCommand::ReadSelected(_) => match &l.selected {
                Some(target) => serde_json::to_value(target)
                    .map_err(|_| fail("Cannot encode selected target"))?,
                None => json!({"state":"no_selected_target"}),
            },
            BridgeCommand::InspectSelected(_) => {
                let selected =
                    l.selected.as_ref().ok_or_else(|| fail("Choose a preview target first"))?;
                let current = super::preview_targets::snapshot(
                    std::path::Path::new(workspace),
                    &selected.target.id,
                )?;
                if current.digest != selected.digest
                    || current.target.revision != selected.target.revision
                {
                    return Err(fail("Selected preview changed; select the current revision"));
                }
                let report = super::preview_targets::inspect(&current);
                super::preview_targets::save_report(
                    reports,
                    &report,
                    subject,
                    workspace,
                    &l.identity,
                )?;
                l.report = Some(report.clone());
                report
            }
            BridgeCommand::ReadReport(_) => {
                l.report.clone().unwrap_or_else(|| json!({"state":"no_report","page_id":l.page}))
            }
            _ => return Err(fail("Invalid bridge parameters")),
        };
        Ok(json!({"bridge_version":1,"request_id":m.request_id,"result":result}))
    }
}
