//! Optional operator-owned, tool-free model opinion worker. No plugin can
//! choose its executable, settings, credentials, endpoint, model or budget.
use super::preview_targets::Snapshot;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, sync::OnceLock};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Semaphore,
};

fn absolute_file(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os(name)?);
    (path.is_absolute() && path.is_file()).then_some(path)
}

pub(crate) async fn review(snapshot: &Snapshot) -> Value {
    let (Some(python), Some(script), Some(settings)) = (
        absolute_file("METTEUR_PREVIEW_MODEL_PYTHON"),
        absolute_file("METTEUR_PREVIEW_MODEL_REVIEWER"),
        absolute_file("METTEUR_PREVIEW_MODEL_SETTINGS"),
    ) else {
        return json!({"model_status":"not_configured"});
    };
    static LIMIT: OnceLock<Semaphore> = OnceLock::new();
    let Ok(_permit) = LIMIT.get_or_init(|| Semaphore::new(1)).try_acquire() else {
        return json!({"model_status":"busy"});
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(40), async {
        let mut cmd = Command::new(python);
        cmd.arg("-I")
            .arg(script)
            .env_clear()
            .env("METTEUR_PREVIEW_MODEL_SETTINGS", settings)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // Isolated Python ignores PYTHONPATH/user hooks. Only OS essentials.
        for key in ["SYSTEMROOT", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        let mut child = cmd.spawn().map_err(|_| "Model worker could not start")?;
        let payload = serde_json::to_vec(
            &json!({"schema_version":1,"html":snapshot.html,"digest":snapshot.digest}),
        )
        .map_err(|_| "Cannot encode selected snapshot")?;
        child
            .stdin
            .take()
            .ok_or("Missing model worker stdin")?
            .write_all(&payload)
            .await
            .map_err(|_| "Model worker input failed")?;
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .ok_or("Missing model worker stdout")?
            .take(32769)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "Model worker output failed")?;
        if bytes.len() > 32768 {
            return Err("Model opinion exceeds limit");
        }
        if !child.wait().await.map_err(|_| "Model worker wait failed")?.success() {
            return Err("Model worker failed");
        }
        let evidence: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid model worker output")?;
        validate(&evidence, &snapshot.digest)?;
        Ok(evidence)
    })
    .await
    .unwrap_or(Err("Model worker exceeded its time budget"));
    result.unwrap_or_else(|error| json!({"model_status":"failed","model_error":error}))
}

pub(crate) fn validate(value: &Value, digest: &str) -> Result<(), &'static str> {
    if value["model_status"] != "completed" {
        return if value["model_status"] == "failed" {
            Ok(())
        } else {
            Err("Invalid model status")
        };
    }
    if value["schema_version"] != 1 || value["digest"] != digest {
        return Err("Model opinion does not match selected snapshot");
    }
    let opinion = &value["model_opinion"];
    if opinion["kind"] != "model_source_opinion"
        || opinion["summary"].as_str().is_none_or(|s| s.len() > 16000)
        || opinion["model"].as_str().is_none_or(|s| s.is_empty() || s.len() > 256)
        || opinion["provider"].as_str().is_none_or(|s| s.is_empty() || s.len() > 128)
        || opinion["call_id"].as_str().is_none_or(|s| uuid::Uuid::parse_str(s).is_err())
        || opinion["findings"].as_array().is_none_or(|a| {
            a.len() > 8 || a.iter().any(|s| s.as_str().is_none_or(|s| s.len() > 4000))
        })
    {
        return Err("Invalid model opinion schema");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opinion_is_bound_to_snapshot_and_cannot_supply_authority() {
        let mut value = json!({"schema_version":1,"digest":"current","model_status":"completed","model_opinion":{"kind":"model_source_opinion","model":"deepseek/test","provider":"synthetic","call_id":uuid::Uuid::new_v4(),"summary":"Source opinion","findings":[]}});
        assert!(validate(&value, "current").is_ok());
        assert!(validate(&value, "old").is_err());
        value["model_opinion"]["findings"] = json!([true]);
        assert!(validate(&value, "current").is_err());
        value["model_status"] = json!("approved");
        assert!(validate(&value, "current").is_err());
    }
}
