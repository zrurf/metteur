//! Addon manifest parsing and validation.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{DaemonError, DaemonResult};
use crate::registry::tool::is_valid_tool_name;

/// One tool exported by an addon.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolEntry {
    /// Registered name; must be PascalCase imperative.
    pub name: String,
    /// The wasm export invoked for calls.
    pub function: String,
    #[serde(default)]
    pub description: String,
    /// JSON Schema of the arguments, inlined as a TOML table.
    #[serde(default = "default_parameters")]
    pub parameters: toml::Table,
}

fn default_parameters() -> toml::Table {
    toml::Table::new()
}

/// One prompt fragment contributed by an addon.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FragmentEntry {
    pub name: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub scope: String,
    /// Package-relative UTF-8 text file with the fragment content.
    pub file: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpEntry {
    pub name: String,
    pub server: McpDeclaration,
}
/// Declared endpoints/argv and named environment references are fingerprinted;
/// credentials themselves never belong in the package.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpDeclaration {
    pub transport: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub env_refs: BTreeMap<String, String>,
    #[serde(default)]
    pub url: Option<String>,
    /// Environment reference holding the bearer token, without the scheme.
    #[serde(default)]
    pub auth_env: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LspEntry {
    pub name: String,
    pub language: LspDeclaration,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LspDeclaration {
    pub id: String,
    pub extensions: Vec<String>,
    pub command: Vec<String>,
    #[serde(default)]
    pub env_refs: BTreeMap<String, String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookEntry {
    pub name: String,
    pub event: String,
    pub function: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeEntry {
    pub name: String,
    #[serde(deserialize_with = "bounded_node_signature")]
    pub signature: metteur_shared::node_catalog::NodeSignature,
    pub function: String,
}
fn bounded_node_signature<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> Result<metteur_shared::node_catalog::NodeSignature, D::Error> {
    // Check compact type syntax before its recursive parser sees package input.
    let value = serde_json::Value::deserialize(de)?;
    let pins = value
        .get("pins")
        .and_then(|v| v.as_array())
        .ok_or_else(|| serde::de::Error::custom("node pins required"))?;
    if pins.len() > 64 {
        return Err(serde::de::Error::custom("node pin limit"));
    }
    for pin in pins {
        let ty = pin
            .get("data_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| serde::de::Error::custom("compact node type required"))?;
        let mut depth = 0i32;
        for b in ty.bytes() {
            if b == b'<' || b == b'{' {
                depth += 1;
            }
            if b == b'>' || b == b'}' {
                depth -= 1;
            }
            if !(0..=8).contains(&depth) {
                return Err(serde::de::Error::custom("node type nesting limit"));
            }
        }
        if ty.len() > 2048 || depth != 0 {
            return Err(serde::de::Error::custom("node type limit"));
        }
    }
    serde_json::from_value(value).map_err(serde::de::Error::custom)
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionEntry {
    pub name: String,
    pub file: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

/// The parsed and validated `manifest.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Reverse-DNS identifier, e.g. `com.example.stats`.
    pub id: String,
    pub version: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    /// SemVer requirement matched against the daemon version.
    #[serde(default)]
    pub metteur_version: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub permissions: PermissionsSection,
    #[serde(default)]
    pub addon: AddonSection,
    #[serde(default)]
    pub tools: Vec<ToolEntry>,
    #[serde(default)]
    pub fragments: Vec<FragmentEntry>,
    #[serde(default)]
    pub mcp: Vec<McpEntry>,
    #[serde(default)]
    pub lsp: Vec<LspEntry>,
    #[serde(default)]
    pub hooks: Vec<HookEntry>,
    #[serde(default)]
    pub nodes: Vec<NodeEntry>,
    #[serde(default)]
    pub functions: Vec<FunctionEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionsSection {
    #[serde(default)]
    pub required: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct AddonSection {
    /// Package-relative path of the wasm module.
    pub entry: String,
    /// Per-call timeout override in milliseconds (0 = global default).
    #[serde(default)]
    pub call_timeout_ms: u64,
}

impl Manifest {
    /// Parses and validates the manifest text located at `package_dir`.
    ///
    /// Validation covers required fields, id format, tool naming, entry
    /// existence, fragment file existence and the metteur version range.
    pub fn load(package_dir: &Path) -> DaemonResult<(Self, PathBuf)> {
        let snapshot = super::signature::Snapshot::read(package_dir)?;
        Ok((Self::from_files(&snapshot.files)?, package_dir.to_path_buf()))
    }

    pub(crate) fn from_files(files: &BTreeMap<String, Vec<u8>>) -> DaemonResult<Self> {
        let text = files
            .get("manifest.toml")
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .ok_or_else(|| DaemonError::Addon("manifest.toml must be a UTF-8 file".into()))?;
        let manifest: Manifest = toml::from_str(text)
            .map_err(|_| DaemonError::Addon("invalid addon manifest schema".into()))?;
        manifest.validate(files)?;
        Ok(manifest)
    }

    fn validate(&self, files: &BTreeMap<String, Vec<u8>>) -> DaemonResult<()> {
        if self.id.is_empty() || self.version.is_empty() || self.name.is_empty() {
            return Err(DaemonError::Addon("id, version and name are required".to_string()));
        }
        // Reverse-DNS id: lowercase dot-separated non-empty labels.
        let labels: Vec<&str> = self.id.split('.').collect();
        if labels.len() < 2
            || labels.iter().any(|label| {
                label.is_empty()
                    || !label.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
            })
        {
            return Err(DaemonError::Addon(format!(
                "addon id '{}' must be reverse-DNS (e.g. com.example.stats)",
                self.id
            )));
        }
        if semver::Version::parse(&self.version).is_err() {
            return Err(DaemonError::Addon(format!("invalid addon version '{}'", self.version)));
        }
        if !self.metteur_version.is_empty() {
            let requirement = semver::VersionReq::parse(&self.metteur_version)
                .map_err(|err| DaemonError::Addon(format!("invalid metteur_version: {err}")))?;
            let current =
                semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("workspace version");
            if !requirement.matches(&current) {
                return Err(DaemonError::Addon(format!(
                    "addon requires metteur {}, but daemon is {}",
                    self.metteur_version, current
                )));
            }
        }
        if self.addon.entry.is_empty()
            && (!self.tools.is_empty()
                || !self.hooks.is_empty()
                || !self.nodes.is_empty()
                || (self.mcp.is_empty() && self.lsp.is_empty() && self.functions.is_empty()))
        {
            return Err(DaemonError::Addon("[addon] entry is required".to_string()));
        }
        if !self.addon.entry.is_empty() {
            package_file(files, &self.addon.entry)?;
        }
        let mut names = HashSet::new();
        for tool in &self.tools {
            if !is_valid_tool_name(&tool.name) {
                return Err(DaemonError::Addon(format!(
                    "tool name '{}' must be PascalCase imperative",
                    tool.name
                )));
            }
            if tool.function.is_empty() {
                return Err(DaemonError::Addon(format!("tool '{}' has no function", tool.name)));
            }
            if !names.insert(tool.name.as_str()) {
                return Err(DaemonError::Addon("duplicate tool contribution".into()));
            }
        }
        names.clear();
        for fragment in &self.fragments {
            if fragment.name.is_empty() || !names.insert(fragment.name.as_str()) {
                return Err(DaemonError::Addon("empty or duplicate fragment contribution".into()));
            }
            let bytes = package_file(files, &fragment.file)?;
            if bytes.len() > 65536 || std::str::from_utf8(bytes).is_err() {
                return Err(DaemonError::Addon("fragment must be UTF-8 and at most 64 KiB".into()));
            }
        }
        for permission in &self.permissions.required {
            if super::runtime::Permission::parse(permission).is_none() {
                return Err(DaemonError::Addon("unknown required addon permission".into()));
            }
        }
        names.clear();
        for contribution in &self.mcp {
            if !is_valid_tool_name(&contribution.name) || !names.insert(contribution.name.as_str())
            {
                return Err(DaemonError::Addon(
                    "MCP contribution names must be unique PascalCase names".into(),
                ));
            }
            contribution.server.validate(files, &self.permissions.required)?;
        }
        names.clear();
        let mut extensions = HashSet::new();
        let mut languages = HashSet::new();
        if self.lsp.len() > 16 {
            return Err(DaemonError::Addon("Addon LSP declaration limit exceeded".into()));
        }
        for entry in &self.lsp {
            let language = &entry.language;
            if !is_valid_tool_name(&entry.name)
                || !names.insert(&entry.name)
                || language.id.is_empty()
                || language.id.len() > 128
                || !language.id.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                || !languages.insert(&language.id)
                || language.extensions.is_empty()
                || language.extensions.len() > 64
            {
                return Err(DaemonError::Addon("Invalid or duplicate LSP declaration".into()));
            }
            for ext in &language.extensions {
                if ext.is_empty()
                    || ext.len() > 64
                    || !ext.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                    || !extensions.insert(ext.to_ascii_lowercase())
                {
                    return Err(DaemonError::Addon("Invalid or duplicate LSP extension".into()));
                }
            }
            McpDeclaration {
                transport: "stdio".into(),
                command: language.command.clone(),
                env_refs: language.env_refs.clone(),
                url: None,
                auth_env: None,
            }
            .validate(files, &self.permissions.required)?;
        }
        names.clear();
        if self.hooks.len() > 16 {
            return Err(DaemonError::Addon("Addon hook declaration limit exceeded".into()));
        }
        let mut subscriptions = HashSet::new();
        for hook in &self.hooks {
            if !is_valid_tool_name(&hook.name)
                || !names.insert(hook.name.as_str())
                || hook.function.is_empty()
                || !subscriptions.insert((&hook.event, &hook.function))
                || !matches!(
                    hook.event.as_str(),
                    "workspace.open" | "workspace.close" | "node.finished" | "run.terminal"
                )
            {
                return Err(DaemonError::Addon(
                    "Invalid or duplicate lifecycle hook declaration".into(),
                ));
            }
        }
        names.clear();
        if self.nodes.len() > 64 {
            return Err(DaemonError::Addon("Addon node declaration limit exceeded".into()));
        }
        for node in &self.nodes {
            if !is_valid_tool_name(&node.name)
                || node.name.len() > 128
                || !names.insert(node.name.as_str())
                || node.function.is_empty()
                || node.function.len() > 128
                || node.signature.kind != node.name
                || node.signature.executor_kind != node.name
            {
                return Err(DaemonError::Addon(
                    "Invalid or duplicate addon node declaration".into(),
                ));
            }
            metteur_shared::node_catalog::addon::validate_signature(&node.signature)
                .map_err(DaemonError::Addon)?;
        }
        names.clear();
        if self.functions.len()>64 {return Err(DaemonError::Addon("Addon function limit exceeded".into()));}
        for function in &self.functions {
            if !is_valid_tool_name(&function.name) || function.name.len()>128 || !names.insert(function.name.as_str()) || function.dependencies.len()>128 || function.dependencies.iter().any(|d|!is_valid_tool_name(d) || d.len()>256) {
                return Err(DaemonError::Addon("Invalid or duplicate function declaration".into()));
            }
            if package_file(files,&function.file)?.len()>1024*1024 {return Err(DaemonError::Addon("Addon function body exceeds limit".into()));}
        }
        Ok(())
    }

    /// Resolved per-call timeout, falling back to the section default.
    pub fn call_timeout_ms(&self, fallback_ms: u64) -> u64 {
        if self.addon.call_timeout_ms != 0 {
            self.addon.call_timeout_ms
        } else if fallback_ms == 0 {
            DEFAULT_CALL_TIMEOUT_MS
        } else {
            fallback_ms
        }
    }

    /// Returns the required permissions not covered by `granted`.
    pub fn missing_permissions(&self, granted: &[String]) -> Vec<String> {
        self.permissions
            .required
            .iter()
            .filter(|required| !granted.contains(required))
            .cloned()
            .collect()
    }

    /// Reads all fragment files as `(fragment, content)` pairs.
    pub fn load_fragments(&self, package_dir: &Path) -> DaemonResult<Vec<(FragmentEntry, String)>> {
        self.fragments
            .iter()
            .map(|fragment| {
                let path = package_dir.join(&fragment.file);
                let content = std::fs::read_to_string(&path).map_err(|err| {
                    DaemonError::Addon(format!("cannot read {}: {err}", path.display()))
                })?;
                Ok((fragment.clone(), content))
            })
            .collect()
    }
}

pub(crate) fn package_file<'a>(
    files: &'a BTreeMap<String, Vec<u8>>,
    name: &str,
) -> DaemonResult<&'a [u8]> {
    validate_package_path(name)?;
    files
        .get(name)
        .map(Vec::as_slice)
        .ok_or_else(|| DaemonError::Addon(format!("package file unavailable: {name}")))
}

pub(crate) fn validate_package_path(name: &str) -> DaemonResult<()> {
    if name.is_empty()
        || name.contains(['\\', ':'])
        || name.split('/').any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(DaemonError::Addon(
            "addon paths must be package-relative without traversal".into(),
        ));
    }
    Ok(())
}

impl McpDeclaration {
    fn validate(&self, files: &BTreeMap<String, Vec<u8>>, required: &[String]) -> DaemonResult<()> {
        let invalid =
            || DaemonError::Addon("Invalid MCP declaration or missing declared capability".into());
        let declares = |name: &str| required.iter().any(|permission| permission == name);
        let env_name = |name: &str| {
            !name.is_empty()
                && name.len() <= 128
                && name.bytes().enumerate().all(|(i, c)| {
                    c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                })
        };
        if (!self.env_refs.is_empty() || self.auth_env.is_some()) && !declares("environment") {
            return Err(invalid());
        }
        if self.env_refs.iter().any(|(name, reference)| !env_name(name) || !env_name(reference))
            || self.auth_env.as_ref().is_some_and(|name| !env_name(name))
        {
            return Err(invalid());
        }
        match self.transport.as_str() {
            "stdio" => {
                if !declares("process")
                    || self.command.is_empty()
                    || self.command.len() > 64
                    || self.url.is_some()
                    || self.auth_env.is_some()
                {
                    return Err(invalid());
                }
                for arg in &self.command {
                    if arg.len() > 4096 || arg.contains('\0') {
                        return Err(invalid());
                    }
                    if let Some(relative) = arg.strip_prefix("${package}/") {
                        package_file(files, relative)?;
                    } else if arg.contains("${") {
                        return Err(invalid());
                    }
                }
                if !self.command[0].starts_with("${package}/")
                    && !Path::new(&self.command[0]).is_absolute()
                {
                    return Err(DaemonError::Addon(
                        "MCP executable must be absolute or package-relative via ${package}/"
                            .into(),
                    ));
                }
            }
            "http" => {
                if !declares("network") || !self.command.is_empty() || !self.env_refs.is_empty() {
                    return Err(invalid());
                }
                let url = reqwest::Url::parse(self.url.as_deref().ok_or_else(invalid)?)
                    .map_err(|_| invalid())?;
                if !matches!(url.scheme(), "http" | "https")
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
        Ok(())
    }
}

/// Default plugin-call timeout when neither config nor manifest set one.
const DEFAULT_CALL_TIMEOUT_MS: u64 = 30_000;

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(dir: &Path, body: &str) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("manifest.toml");
        std::fs::write(&path, body).unwrap();
        dir.to_path_buf()
    }

    const VALID: &str = r#"
id = "com.example.stats"
version = "0.1.0"
name = "Stats Tools"
metteur_version = ">=0.0.1"

[permissions]
required = ["tools"]

[addon]
entry = "main.wasm"

[[tools]]
name = "WordCount"
function = "word_count"
description = "Counts words."
[tools.parameters]
type = "object"

[[fragments]]
name = "style_guide"
file = "style.md"
"#;

    #[test]
    fn accepts_valid_manifest_with_matching_version() {
        let dir = write_manifest(
            &std::env::temp_dir().join(format!("addon-ok-{}", uuid::Uuid::new_v4())),
            VALID,
        );
        std::fs::write(dir.join("main.wasm"), b"fake").unwrap();
        std::fs::write(dir.join("style.md"), "text").unwrap();
        let (manifest, _) = Manifest::load(&dir).unwrap();
        assert_eq!(manifest.tools.len(), 1);
        assert_eq!(manifest.missing_permissions(&["tools".into()]), Vec::<String>::new());
        assert_eq!(manifest.missing_permissions(&[]), vec!["tools".to_string()]);
    }

    #[test]
    fn rejects_bad_id_or_missing_entry_or_version_conflict() {
        let base = std::env::temp_dir().join(format!("addon-bad-{}", uuid::Uuid::new_v4()));

        let bad_id =
            write_manifest(&base.join("a"), &VALID.replace("com.example.stats", "Not An Id"));
        std::fs::write(base.join("a").join("main.wasm"), b"x").unwrap();
        std::fs::write(base.join("a").join("style.md"), "t").unwrap();
        assert!(Manifest::load(&bad_id).is_err());

        let no_entry = write_manifest(&base.join("b"), VALID);
        std::fs::write(base.join("b").join("style.md"), "t").unwrap();
        assert!(Manifest::load(&no_entry).is_err());
        std::fs::write(base.join("b").join("main.wasm"), b"x").unwrap();
        assert!(Manifest::load(&no_entry).is_ok());

        let future = write_manifest(&base.join("c"), &VALID.replace(">=0.0.1", ">=99.0.0"));
        std::fs::write(base.join("c").join("main.wasm"), b"x").unwrap();
        std::fs::write(base.join("c").join("style.md"), "t").unwrap();
        assert!(Manifest::load(&future).is_err());
    }

    #[test]
    fn rejects_non_pascal_tool_names() {
        let dir = std::env::temp_dir().join(format!("addon-name-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let body = VALID.replace("WordCount", "word_count");
        std::fs::write(dir.join("manifest.toml"), body).unwrap();
        std::fs::write(dir.join("main.wasm"), b"x").unwrap();
        std::fs::write(dir.join("style.md"), "t").unwrap();
        assert!(Manifest::load(&dir).is_err());
    }
}
