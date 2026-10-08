//! Package integrity signing and verification for addons.
//!
//! A package may carry a `signature.toml` produced over the canonical digest
//! of every file inside the package. The daemon verifies it at
//! install time so that an on-disk or in-transit package cannot be tampered
//! with after a trusted developer published it.

use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::error::{DaemonError, DaemonResult};

/// The signature algorithm currently supported.
pub const ALGORITHM_ED25519: &str = "ed25519";

/// Signature verification policy shared with the addon host.
#[derive(Debug, Clone)]
pub struct Policy {
    /// Require a signature; reject the package when missing.
    pub require_signature: bool,
    /// Trusted public keys (base64); empty accepts any well-formed signature.
    pub trusted_keys: Vec<String>,
}

impl Policy {
    /// Builds a policy from the addon configuration section.
    pub fn from_config(cfg: &metteur_shared::config::AddonConfig) -> Self {
        Self {
            require_signature: cfg.require_signature,
            trusted_keys: cfg.signing_keys.clone(),
        }
    }
}

/// Named fields of a package `signature.toml`.
#[derive(serde::Deserialize)]
struct SignatureFile {
    algorithm: String,
    /// Base64-encoded 32-byte Ed25519 public key.
    public_key: String,
    /// Base64-encoded signature over the canonical digest.
    value: String,
}

/// Verifies `package_dir` against its `signature.toml`.
///
/// Returns an error when the signature is missing, malformed, does not carry
/// the expected algorithm, or does not validate against the trusted public
/// key. When `trusted_keys` is empty, any well-formed self-contained signature
/// is accepted (useful during development); otherwise at least one listed key
/// must verify.
pub fn verify(
    package_dir: &Path,
    trusted_keys: &[String],
    require_signature: bool,
) -> DaemonResult<()> {
    Snapshot::read(package_dir)?.verify(trusted_keys, require_signature)
}

/// Immutable bytes used for both verification and execution. No verified path
/// is reopened later to load a different implementation.
pub(crate) struct Snapshot {
    pub files: BTreeMap<String, Vec<u8>>,
}
impl Snapshot {
    pub fn read(root: &Path) -> DaemonResult<Self> {
        let mut files = BTreeMap::new();
        collect_files(root, root, &mut files, &mut 0)?;
        Ok(Self {
            files,
        })
    }
    pub fn fingerprint(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.payload());
        hash.update(b"\0signature\0");
        if let Some(signature) = self.files.get("signature.toml") {
            hash.update(signature);
        }
        hex(&hash.finalize())
    }
    fn payload(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (path, bytes) in &self.files {
            if path == "signature.toml" {
                continue;
            }
            out.extend_from_slice(path.as_bytes());
            out.push(0);
            out.extend_from_slice(hex(&Sha256::digest(bytes)).as_bytes());
            out.push(b'\n');
        }
        out
    }
    pub fn verify(&self, trusted_keys: &[String], require_signature: bool) -> DaemonResult<()> {
        let Some(bytes) = self.files.get("signature.toml") else {
            return if require_signature {
                Err(DaemonError::Addon("missing signature.toml".into()))
            } else {
                Ok(())
            };
        };
        let text = std::str::from_utf8(bytes)
            .map_err(|_| DaemonError::Addon("signature.toml is not UTF-8".into()))?;
        let file: SignatureFile = toml::from_str(text)
            .map_err(|err| DaemonError::Addon(format!("invalid signature.toml: {err}")))?;
        if file.algorithm != ALGORITHM_ED25519 {
            return Err(DaemonError::Addon(format!(
                "unsupported signature algorithm '{}'",
                file.algorithm
            )));
        }

        let public_key = decode_key(&file.public_key)?;
        let signature_bytes = B64
            .decode(&file.value)
            .map_err(|_| DaemonError::Addon("signature value is not valid base64".to_string()))?;
        let signature = Signature::from_slice(&signature_bytes).map_err(|_| {
            DaemonError::Addon("signature is not a valid Ed25519 signature".to_string())
        })?;

        let digest = self.payload();
        let key = VerifyingKey::from_bytes(&public_key)
            .map_err(|_| DaemonError::Addon("public key is not a valid Ed25519 key".to_string()))?;
        if key.verify_strict(&digest, &signature).is_err() {
            return Err(DaemonError::Addon(
                "signature does not match the package contents".to_string(),
            ));
        }

        // When a trusted key set exists the embedded public_key must be one of them.
        if !trusted_keys.is_empty() && !trusted_keys.iter().any(|k| matches_key(k, &public_key)) {
            return Err(DaemonError::Addon(
                "package public key is not in the trusted key set".to_string(),
            ));
        }
        Ok(())
    }
}

/// Signs `package_dir`, returning the content of its `signature.toml`.
///
/// The counterpart of [`verify`]: fingerprints every package file, signs the
/// canonical digest and encodes the public key together with the signature so
/// the daemon can later validate the package at install time.
pub fn sign_package(package_dir: &Path, signing_key: &SigningKey) -> DaemonResult<String> {
    let digest = canonical_digest(package_dir)?;
    let signature: Signature = signing_key.sign(&digest);
    let public_key = signing_key.verifying_key().to_bytes();
    Ok(format!(
        "algorithm = \"ed25519\"\npublic_key = \"{}\"\nvalue = \"{}\"\n",
        B64.encode(public_key),
        B64.encode(signature.to_bytes())
    ))
}

/// Decodes a base64 Ed25519 public key into its raw 32 bytes.
fn decode_key(encoded: &str) -> DaemonResult<[u8; 32]> {
    let bytes = B64
        .decode(encoded)
        .map_err(|_| DaemonError::Addon("public key is not valid base64".to_string()))?;
    bytes.try_into().map_err(|_| DaemonError::Addon("public key must be 32 bytes".to_string()))
}

fn matches_key(encoded: &str, expected: &[u8; 32]) -> bool {
    B64.decode(encoded).map(|bytes| bytes.as_slice() == expected).unwrap_or(false)
}

/// Builds the canonical signed payload for every regular file in `package_dir`.
///
/// Files are enumerated by their package-relative path and sorted; the
/// signature.toml itself is excluded from the digest. Each entry contributes
/// `{relative_path}\0{sha256_hex}\n`, so any file added, removed or modified
/// changes the result.
fn canonical_digest(package_dir: &Path) -> DaemonResult<Vec<u8>> {
    Ok(Snapshot::read(package_dir)?.payload())
}

fn collect_files(
    root: &Path,
    dir: &Path,
    entries: &mut BTreeMap<String, Vec<u8>>,
    total: &mut usize,
) -> DaemonResult<()> {
    if std::fs::symlink_metadata(dir)?.file_type().is_symlink() {
        return Err(DaemonError::Addon("package symlinks are not supported".into()));
    }
    for entry in std::fs::read_dir(dir).map_err(DaemonError::Io)? {
        let entry = entry.map_err(DaemonError::Io)?;
        let path = entry.path();
        let relative = match path.strip_prefix(root) {
            Ok(relative) => relative
                .components()
                .map(|c| {
                    c.as_os_str()
                        .to_str()
                        .ok_or_else(|| DaemonError::Addon("Package paths must be UTF-8".into()))
                })
                .collect::<DaemonResult<Vec<_>>>()?
                .join("/"),
            Err(_) => continue,
        };
        super::manifest::validate_package_path(&relative)?;
        let file_type = entry.file_type().map_err(DaemonError::Io)?;
        if file_type.is_symlink() {
            return Err(DaemonError::Addon("package symlinks are not supported".into()));
        } else if file_type.is_dir() {
            collect_files(root, &path, entries, total)?;
        } else if file_type.is_file() {
            let size = entry.metadata()?.len();
            if size > 64 * 1024 * 1024
                || (*total as u64).saturating_add(size) > 64 * 1024 * 1024
                || entries.len() >= 1024
            {
                return Err(DaemonError::Addon("package exceeds 64 MiB or 1024 files".into()));
            }
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(&path)?
                .take((64 * 1024 * 1024 - *total + 1) as u64)
                .read_to_end(&mut bytes)?;
            *total = total.saturating_add(bytes.len());
            if *total > 64 * 1024 * 1024 {
                return Err(DaemonError::Addon("package exceeds 64 MiB".into()));
            }
            entries.insert(relative, bytes);
        } else {
            return Err(DaemonError::Addon("package contains a non-regular file".into()));
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use ed25519_dalek::{Signer, SigningKey};

    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn make_package(dir: &Path) -> PathBuf {
        let root = dir.join("pkg");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("manifest.toml"), r#"id="com.example.x"#).unwrap();
        std::fs::write(root.join("main.wasm"), "module").unwrap();
        root
    }

    fn signed_package(dir: &Path) -> (PathBuf, [u8; 32]) {
        let root = make_package(dir);
        let signing_key = test_signing_key();
        let digest = canonical_digest(&root).unwrap();
        let signature: Signature = signing_key.sign(&digest);
        let public_key = signing_key.verifying_key().to_bytes();
        let file = format!(
            "algorithm = \"ed25519\"\npublic_key = \"{}\"\nvalue = \"{}\"\n",
            B64.encode(public_key),
            B64.encode(signature.to_bytes())
        );
        std::fs::write(root.join("signature.toml"), file).unwrap();
        (root, public_key)
    }

    #[test]
    fn accepts_valid_signature() {
        let dir = std::env::temp_dir().join(format!("addon-sig-ok-{}", uuid::Uuid::new_v4()));
        let (root, key) = signed_package(&dir);
        assert!(verify(&root, &[], true).is_ok());
        assert!(verify(&root, &[B64.encode(key)], true).is_ok());
        assert!(verify(&root, &[B64.encode([0u8; 32])], true).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_tampered_file() {
        let dir = std::env::temp_dir().join(format!("addon-sig-tamper-{}", uuid::Uuid::new_v4()));
        let (root, _key) = signed_package(&dir);
        std::fs::write(root.join("main.wasm"), "moduleX").unwrap();
        assert!(verify(&root, &[], true).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn signed_package_round_trips() {
        let dir =
            std::env::temp_dir().join(format!("addon-sig-roundtrip-{}", uuid::Uuid::new_v4()));
        let root = make_package(&dir);
        let signing_key = test_signing_key();
        let content = sign_package(&root, &signing_key).unwrap();
        std::fs::write(root.join("signature.toml"), content).unwrap();
        let public_key = signing_key.verifying_key().to_bytes();
        assert!(verify(&root, &[B64.encode(public_key)], true).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_missing_signature_when_required() {
        let dir = std::env::temp_dir().join(format!("addon-sig-nosig-{}", uuid::Uuid::new_v4()));
        let root = make_package(&dir);
        assert!(verify(&root, &[], true).is_err());
        assert!(verify(&root, &[], false).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
