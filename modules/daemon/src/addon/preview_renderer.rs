//! Optional operator-owned isolated renderer. Plugins cannot select a command,
//! executable, browser profile, URL, output path, or environment.
use super::preview_targets::Snapshot;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, sync::OnceLock};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Semaphore,
};

fn absolute_env(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os(name)?);
    if p.is_absolute() && p.is_file() {
        Some(p)
    } else {
        None
    }
}
pub(crate) async fn render(snapshot: &Snapshot) -> Value {
    let Some(node) = absolute_env("METTEUR_PREVIEW_NODE") else {
        return json!({"browser_checks":"not_configured","browser_error":"Host renderer is not configured"});
    };
    let Some(script) = absolute_env("METTEUR_PREVIEW_RENDERER") else {
        return json!({"browser_checks":"not_configured","browser_error":"Host renderer script is not configured"});
    };
    static LIMIT: OnceLock<Semaphore> = OnceLock::new();
    let Ok(_permit) = LIMIT.get_or_init(|| Semaphore::new(1)).try_acquire() else {
        return json!({"browser_checks":"busy","browser_error":"Another preview is being checked; try again"});
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let mut cmd = Command::new(node);
        cmd.arg(script)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // OS/runtime essentials only; no API keys, proxy credentials, Node hooks,
        // personal browser profile, or workspace-provided PATH injection.
        for key in [
            "PATH",
            "SYSTEMROOT",
            "WINDIR",
            "TEMP",
            "TMP",
            "TMPDIR",
            "HOME",
            "PLAYWRIGHT_BROWSERS_PATH",
        ] {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        if let Some(path) = absolute_env("METTEUR_PREVIEW_CHROMIUM") {
            cmd.env("METTEUR_PREVIEW_CHROMIUM", path);
        }
        let mut child = cmd.spawn().map_err(|_| "Renderer could not start")?;
        let payload = serde_json::to_vec(
            &json!({"schema_version":1,"html":snapshot.html,"digest":snapshot.digest}),
        )
        .map_err(|_| "Cannot encode snapshot")?;
        child
            .stdin
            .take()
            .ok_or("Renderer stdin missing")?
            .write_all(&payload)
            .await
            .map_err(|_| "Renderer input failed")?;
        let mut output = Vec::new();
        child
            .stdout
            .take()
            .ok_or("Renderer stdout missing")?
            .take(2_200_001)
            .read_to_end(&mut output)
            .await
            .map_err(|_| "Renderer output failed")?;
        if output.len() > 2_200_000 {
            return Err("Renderer output exceeds limit".into());
        }
        let status = child.wait().await.map_err(|_| "Renderer wait failed")?;
        if !status.success() {
            return Err(
                "Isolated renderer failed; check host installation and browser sandbox".into()
            );
        }
        let value: Value =
            serde_json::from_slice(&output).map_err(|_| "Invalid renderer output")?;
        validate(&value, &snapshot.digest)?;
        Ok::<Value, String>(value)
    })
    .await
    .unwrap_or_else(|_| Err("Renderer exceeded its time budget".into()));
    match result {
        Ok(v) => v,
        Err(e) => json!({"browser_checks":"failed","browser_error":e}),
    }
}
pub(crate) fn validate(v: &Value, digest: &str) -> Result<(), String> {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    if v["schema_version"] != 1
        || v["digest"] != digest
        || v["browser_checks"] != "completed"
        || v["screenshot"]["mime"] != "image/jpeg"
    {
        return Err("Renderer evidence does not match the snapshot".into());
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(v["screenshot"]["base64"].as_str().ok_or("Screenshot missing")?)
        .map_err(|_| "Invalid screenshot")?;
    let hash: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
    if data.len() > 1_500_000
        || !data.starts_with(&[0xff, 0xd8, 0xff])
        || v["screenshot"]["sha256"] != hash
    {
        return Err("Screenshot integrity failed".into());
    }
    if v["console"]["entries"].as_array().is_none_or(|a| a.len() > 100) {
        return Err("Invalid console evidence".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mismatched_or_forged_browser_evidence_is_rejected() {
        assert!(
            validate(
                &json!({"schema_version":1,"digest":"other","browser_checks":"completed"}),
                "expected"
            )
            .is_err()
        );
        assert!(validate(&json!({"schema_version":1,"digest":"expected","browser_checks":"completed","screenshot":{"mime":"text/html","base64":"PHNjcmlwdD4="}}),"expected").is_err());
        assert!(validate(&json!({"schema_version":1,"digest":"expected","browser_checks":"completed","screenshot":{"mime":"image/jpeg","base64":"/9j/","sha256":"forged"},"console":{"entries":[]}}),"expected").is_err());
    }
}
