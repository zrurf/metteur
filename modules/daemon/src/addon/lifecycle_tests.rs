//! Real Wasm and isolated disk evidence for package admission and ownership.
use super::*;
use crate::{execution::context::ExecutionContext, llm::LlmClientFactory, registry::Registry};
use std::path::PathBuf;

fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("r07-addon-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn wasm(value: &str) -> Vec<u8> {
    let json = serde_json::to_string(value).unwrap();
    let stores = json
        .bytes()
        .enumerate()
        .map(|(i, b)| format!("local.get $ptr i64.const {i} i64.add i32.const {b} call $store"))
        .collect::<Vec<_>>()
        .join("\n");
    wat::parse_str(format!(
        r#"(module
      (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
      (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
      (import "extism:host/env" "output_set" (func $output (param i64 i64)))
      (func (export "run") (result i32) (local $ptr i64)
        i64.const {} call $alloc local.set $ptr
        {stores} local.get $ptr i64.const {} call $output i32.const 0))"#,
        json.len(),
        json.len()
    ))
    .unwrap()
}
fn package(base: &Path, id: &str, value: &str, permissions: &str) -> PathBuf {
    let path = base.join(format!("source-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    std::fs::write(
        path.join("manifest.toml"),
        format!(
            r#"id = "{id}"
version = "0.1.0"
name = "Test package"
[permissions]
required = [{permissions}]
[addon]
entry = "main.wasm"
[[tools]]
name = "ReadValue"
function = "run"
[[fragments]]
name = "Guidance"
file = "guidance.md"
"#
        ),
    )
    .unwrap();
    std::fs::write(path.join("main.wasm"), wasm(value)).unwrap();
    std::fs::write(path.join("guidance.md"), value).unwrap();
    path
}
fn host(base: &Path) -> Arc<AddonHost> {
    AddonHost::new(
        &base.join("data"),
        Arc::new(Registry::with_builtins()),
        1000,
        &Default::default(),
    )
}

#[test]
fn legacy_checkpoint_without_package_evidence_is_readable_but_not_resumable() {
    let checkpoint = crate::execution::checkpoint::ExecutionCheckpoint::running(
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        1,
    );
    assert!(checkpoint.ensure_addons(&Registry::default()).is_ok());
    let mut value = serde_json::to_value(checkpoint).unwrap();
    value.as_object_mut().unwrap().remove("addon_identity_version");
    value.as_object_mut().unwrap().remove("addon_packages");
    let legacy: crate::execution::checkpoint::ExecutionCheckpoint =
        serde_json::from_value(value).unwrap();
    assert!(legacy.ensure_addons(&Registry::default()).is_err());
}
async fn call(registry: Arc<Registry>, root: &Path, name: &str) -> Value {
    let tool = registry.tool(name).expect("registered tool");
    let mut ctx = ExecutionContext::new(registry, LlmClientFactory::new(), root.into());
    tool.call(&[], &mut ctx).await.unwrap()
}

#[tokio::test]
async fn scopes_real_wasm_fragments_restart_and_owned_cleanup() {
    let base = root();
    let a = base.join("a");
    let b = base.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let host = host(&base);
    let pa = package(&base, "com.test.pkg", "workspace-a", "");
    let pb = package(&base, "com.test.pkg", "workspace-b", "");
    let info = host.install(&pa, Some(&a), &[]).await.unwrap();
    assert_eq!(info.status, "Loaded");
    assert!(!info.fingerprint.is_empty());
    host.install(&pb, Some(&b), &[]).await.unwrap();
    let ra = host.registry_for(Some(&a), false).await.unwrap();
    let rb = host.registry_for(Some(&b), false).await.unwrap();
    assert_eq!(
        call(ra.clone(), &a, "ComTestPkgReadValue").await,
        Value::Json(serde_json::json!("workspace-a"))
    );
    assert_eq!(
        call(rb, &b, "ComTestPkgReadValue").await,
        Value::Json(serde_json::json!("workspace-b"))
    );
    assert_eq!(ra.addon_fragments.values().flatten().next().unwrap().content, "workspace-a");
    assert!(host.registry_for(None, false).await.unwrap().tool("ComTestPkgReadValue").is_none());
    let global = package(&base, "com.test.global", "global-first", "");
    host.install(&global, None, &[]).await.unwrap();
    let composed = host.registry_for(Some(&a), false).await.unwrap();
    assert_eq!(
        composed.addon_fragments.values().flatten().map(|f| f.content.as_str()).collect::<Vec<_>>(),
        vec!["global-first", "workspace-a"]
    );
    host.uninstall("com.test.global", None).await.unwrap();
    let restarted = AddonHost::new(
        &base.join("data"),
        Arc::new(Registry::with_builtins()),
        1000,
        &Default::default(),
    );
    assert!(
        restarted
            .registry_for(Some(&a), false)
            .await
            .unwrap()
            .tool("ComTestPkgReadValue")
            .is_some()
    );
    restarted.set_enabled("com.test.pkg", false, Some(&a)).await.unwrap();
    assert!(
        restarted
            .registry_for(Some(&a), false)
            .await
            .unwrap()
            .tool("ComTestPkgReadValue")
            .is_none()
    );
    assert!(restarted.fragments_for(&a).await.is_empty());
    assert!(
        restarted
            .registry_for(Some(&b), false)
            .await
            .unwrap()
            .tool("ComTestPkgReadValue")
            .is_some()
    );
    restarted.set_enabled("com.test.pkg", true, Some(&a)).await.unwrap();
    restarted.uninstall("com.test.pkg", Some(&a)).await.unwrap();
    assert!(
        restarted
            .registry_for(Some(&a), false)
            .await
            .unwrap()
            .tool("ComTestPkgReadValue")
            .is_none()
    );
    assert!(restarted.registry_for(Some(&b), false).await.unwrap().tool("ReadFile").is_some());
}

#[tokio::test]
async fn grants_are_explicit_fingerprint_bound_and_not_inherited_on_upgrade() {
    let base = root();
    let host = host(&base);
    let source = package(&base, "com.test.grants", "first", r#""fs:read""#);
    assert!(host.install(&source, None, &[]).await.is_err());
    assert!(host.registry_for(None, false).await.unwrap().tool("ComTestGrantsReadValue").is_none());
    host.install(&source, None, &["fs:read".into()]).await.unwrap();
    let changed = package(&base, "com.test.grants", "second", r#""fs:read", "fs:write""#);
    assert!(host.install(&changed, None, &["fs:read".into()]).await.is_err());
    assert_eq!(
        call(host.registry_for(None, false).await.unwrap(), &base, "ComTestGrantsReadValue").await,
        Value::Json(serde_json::json!("first"))
    );
    host.set_enabled("com.test.grants", false, None).await.unwrap();
    assert_eq!(host.list(&[]).await[0].granted_permissions, vec!["fs:read"]);
    host.set_enabled("com.test.grants", true, None).await.unwrap();
    let installed = base.join("data/addons/com.test.grants");
    std::fs::write(installed.join("guidance.md"), "external modification").unwrap();
    assert!(host.registry_for(None, false).await.unwrap().tool("ComTestGrantsReadValue").is_none());
    assert_eq!(host.list(&[]).await[0].status, "Failed");
    // A grant file copied into an old package is never authorization.
    let legacy = package(&base, "com.test.legacy", "legacy", r#""fs:read""#);
    std::fs::write(legacy.join("granted.toml"), "granted = ['fs:read']").unwrap();
    std::fs::rename(legacy, base.join("data/addons/legacy")).unwrap();
    assert!(host.registry_for(None, false).await.unwrap().tool("ComTestLegacyReadValue").is_none());
}

#[tokio::test]
async fn active_snapshots_pin_code_reject_mutation_and_recovery_detects_changes() {
    let base = root();
    let host = host(&base);
    let source = package(&base, "com.test.pin", "old", "");
    host.install(&source, None, &[]).await.unwrap();
    let active = host.registry_for(Some(&base), true).await.unwrap();
    assert!(host.uninstall("com.test.pin", None).await.is_err());
    assert!(host.set_enabled("com.test.pin", false, None).await.is_err());
    let mut checkpoint = crate::execution::checkpoint::ExecutionCheckpoint::running(
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        1,
    );
    checkpoint.addon_packages = active.addon_packages.clone();
    std::fs::write(base.join("data/addons/com.test.pin/main.wasm"), wasm("new")).unwrap();
    assert_eq!(
        call(active.clone(), &base, "ComTestPinReadValue").await,
        Value::Json(serde_json::json!("old"))
    );
    let new_scope = base.join("new-workspace");
    std::fs::create_dir(&new_scope).unwrap();
    let new_registry = host.registry_for(Some(&new_scope), false).await.unwrap();
    assert!(new_registry.tool("ComTestPinReadValue").is_none());
    assert!(checkpoint.ensure_addons(&new_registry).is_err());
    drop(active);
    host.uninstall("com.test.pin", None).await.unwrap();
}

#[tokio::test]
async fn collision_partial_exports_duplicates_and_future_contributions_fail_atomically() {
    let base = root();
    let host = host(&base);
    let first = package(&base, "com.test.same", "keep", "");
    host.install(&first, None, &[]).await.unwrap();
    let second = package(&base, "com.test", "replace", "");
    let text = std::fs::read_to_string(second.join("manifest.toml"))
        .unwrap()
        .replace("ReadValue", "SameReadValue");
    std::fs::write(second.join("manifest.toml"), text).unwrap();
    assert!(host.install(&second, None, &[]).await.is_err());
    let bad = package(&base, "com.test.partial", "bad", "");
    let manifest = std::fs::read_to_string(bad.join("manifest.toml")).unwrap();
    std::fs::write(
        bad.join("manifest.toml"),
        format!("{manifest}\n[[tools]]\nname='Missing'\nfunction='no_export'\n"),
    )
    .unwrap();
    assert!(host.install(&bad, None, &[]).await.is_err());
    let registry = host.registry_for(None, false).await.unwrap();
    assert!(registry.tool("ComTestPartialReadValue").is_none());
    assert!(registry.tool("ReadFile").is_some());
    assert_eq!(
        call(registry, &base, "ComTestSameReadValue").await,
        Value::Json(serde_json::json!("keep"))
    );
    std::fs::write(
        bad.join("manifest.toml"),
        format!("{manifest}\n[[hooks]]\nname='Observe'\nevent='node_finished'\nfunction='run'\n"),
    )
    .unwrap();
    assert!(host.install(&bad, None, &[]).await.is_err());
    // Duplicate installed IDs fail both registrations, never last-writer-wins.
    let duplicate = package(&base, "com.test.same", "duplicate", "");
    std::fs::rename(duplicate, base.join("data/addons/duplicate")).unwrap();
    assert!(host.registry_for(None, false).await.unwrap().tool("ComTestSameReadValue").is_none());
    assert!(host.install(&first, None, &[]).await.is_err());
    let registry = Registry::with_builtins();
    assert!(
        registry
            .replace_owned_tools("test", vec![Arc::new(crate::registry::tools::fs_tools::ReadFile)])
            .is_err()
    );
    assert!(registry.tool("ReadFile").is_some());
}

#[tokio::test]
async fn signature_policy_and_portable_paths_are_checked_before_install() {
    let base = root();
    let source = package(&base, "com.test.signed", "signed", "");
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let signature = signature::sign_package(&source, &key).unwrap();
    std::fs::write(source.join("signature.toml"), signature).unwrap();
    let config = metteur_shared::config::AddonConfig {
        require_signature: true,
        ..Default::default()
    };
    let host =
        AddonHost::new(&base.join("data"), Arc::new(Registry::with_builtins()), 1000, &config);
    host.install(&source, None, &[]).await.unwrap();
    std::fs::write(source.join("guidance.md"), "tampered").unwrap();
    assert!(host.install(&source, None, &[]).await.is_err());
    assert_eq!(
        call(host.registry_for(None, false).await.unwrap(), &base, "ComTestSignedReadValue").await,
        Value::Json(serde_json::json!("signed"))
    );
    for name in ["../escape", "/absolute", "C:/drive", "a\\b", "a/./b", "a//b"] {
        assert!(manifest::validate_package_path(name).is_err(), "{name}");
    }
    let archive = base.join("escape.zip");
    {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        zip.start_file("../escape", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"escape").unwrap();
        zip.finish().unwrap();
    }
    assert!(host.install(&archive, None, &[]).await.is_err());
    assert!(!base.join("data/addons/escape").exists());
    #[cfg(unix)]
    {
        let source = package(&base, "com.test.link", "link", "");
        std::os::unix::fs::symlink(base.join("data"), source.join("outside")).unwrap();
        assert!(host.install(&source, None, &[]).await.is_err());
    }
}

fn write_wasm(path: &str) -> Vec<u8> {
    fn allocation(text: &str, local: &str) -> String {
        let stores = text
            .bytes()
            .enumerate()
            .map(|(i, b)| {
                format!("local.get ${local} i64.const {i} i64.add i32.const {b} call $store")
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!("i64.const {} call $alloc local.set ${local} {stores}", text.len())
    }
    wat::parse_str(format!(
        r#"(module
      (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
      (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
      (import "extism:host/env" "output_set" (func $output (param i64 i64)))
      (import "extism:host/user" "fs_write" (func $write (param i64 i64)))
      (func (export "run") (result i32) (local $path i64) (local $data i64) (local $result i64)
        {} {} {} local.get $path local.get $data call $write
        local.get $result i64.const 4 call $output i32.const 0))"#,
        allocation(path, "path"),
        allocation("written by addon", "data"),
        allocation("true", "result")
    ))
    .unwrap()
}

#[tokio::test]
async fn actual_wasm_file_host_requires_grant_original_approval_and_safe_path() {
    let base = root();
    let host = host(&base);
    for (id, path, grant, mode, allowed) in [
        ("com.test.nogrant", "none.txt", false, crate::sandbox::PermissionMode::Sandbox, false),
        ("com.test.ask", "ask.txt", true, crate::sandbox::PermissionMode::Ask, false),
        ("com.test.allowed", "allowed.txt", true, crate::sandbox::PermissionMode::Sandbox, true),
        (
            "com.test.metadata",
            ".metteur/config.toml",
            true,
            crate::sandbox::PermissionMode::Sandbox,
            false,
        ),
        ("com.test.escape", "../outside.txt", true, crate::sandbox::PermissionMode::Sandbox, false),
    ] {
        let source = package(
            &base,
            id,
            "test",
            if grant {
                r#""fs:write""#
            } else {
                ""
            },
        );
        std::fs::write(source.join("main.wasm"), write_wasm(path)).unwrap();
        host.install(
            &source,
            Some(&base),
            &if grant {
                vec!["fs:write".into()]
            } else {
                vec![]
            },
        )
        .await
        .unwrap();
        let registry = host.registry_for(Some(&base), false).await.unwrap();
        let tool = registry.tool(&format!("{}ReadValue", super::pascal(id))).unwrap();
        let mut ctx = ExecutionContext::new(registry, LlmClientFactory::new(), base.clone());
        ctx.permission_mode = mode;
        let result = tool.call(&[], &mut ctx).await;
        assert_eq!(result.is_ok(), allowed, "{id}: {result:?}");
        if allowed {
            assert_eq!(std::fs::read_to_string(base.join(path)).unwrap(), "written by addon");
        } else if !path.starts_with("..") {
            assert!(!base.join(path).exists(), "{id}");
        }
    }
}
