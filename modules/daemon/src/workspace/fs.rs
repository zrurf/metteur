//! Workspace-scoped filesystem access for tools.

use std::path::{Component, Path, PathBuf};

use metteur_shared::{FileSystem, NativeFileSystem, Uri};

use crate::error::{DaemonError, DaemonResult};

/// Metadata must come from symlink_metadata: following links first loses the
/// Windows junction/reparse identity. All contained file stores share this check.
pub(crate) fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

/// A filesystem rooted at a workspace that rejects paths escaping it.
#[derive(Clone)]
pub struct WorkspaceFs {
    root: PathBuf,
    inner: NativeFileSystem,
}

impl WorkspaceFs {
    /// Creates a filesystem constrained to `root`.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: strip_extended_prefix(root),
            inner: NativeFileSystem::new(),
        }
    }

    /// Returns the workspace root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves a tool-supplied path against the workspace root.
    ///
    /// Relative paths are joined to the root; absolute paths must stay inside
    /// it. Dot segments are collapsed and any resulting escape is rejected.
    /// The path may not exist yet (used for writes).
    pub fn resolve(&self, input: &str) -> DaemonResult<PathBuf> {
        let raw = PathBuf::from(input);
        let absolute = if raw.is_absolute() {
            raw
        } else {
            self.root.join(raw)
        };
        let normalized = normalize(&absolute);
        self.ensure_within(&normalized)?;
        Ok(normalized)
    }

    /// Resolves a path that must already exist, verifying it through
    /// canonicalization to defeat symlink escapes.
    pub fn resolve_existing(&self, input: &str) -> DaemonResult<PathBuf> {
        let path = self.resolve(input)?;
        let canonical = path.canonicalize().map_err(DaemonError::Io)?;
        let canonical = strip_extended_prefix(canonical);
        self.ensure_within(&canonical)?;
        Ok(canonical)
    }

    /// Reads a file, verifying it stays inside the workspace.
    pub fn read(&self, input: &str) -> DaemonResult<Vec<u8>> {
        let path = self.resolve_existing(input)?;
        self.inner
            .read(&Uri::from_path(&path))
            .map_err(|e| DaemonError::Io(std::io::Error::other(e.to_string())))
    }

    /// Writes a file, rejecting paths that would escape the workspace.
    pub fn write(&self, input: &str, data: &[u8]) -> DaemonResult<()> {
        let path = self.resolve(input)?;
        self.inner
            .write(&Uri::from_path(&path), data)
            .map_err(|e| DaemonError::Io(std::io::Error::other(e.to_string())))
    }

    /// Lists the entries directly under a directory in the workspace.
    pub fn list(&self, input: &str) -> DaemonResult<Vec<PathBuf>> {
        let path = self.resolve_existing(input)?;
        let mut out = Vec::new();
        for entry in std::fs::read_dir(path)? {
            out.push(entry?.path());
        }
        Ok(out)
    }

    /// Returns whether `path` is contained within the workspace root.
    fn ensure_within(&self, path: &Path) -> DaemonResult<()> {
        if path.starts_with(self.root()) {
            Ok(())
        } else {
            Err(DaemonError::PermissionDenied(format!(
                "path '{}' escapes workspace '{}'",
                path.display(),
                self.root().display()
            )))
        }
    }
}

/// Collapses `.` and `..` components of an absolute path.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Removes the `\\?\` extended-length prefix on Windows.
///
/// `canonicalize` may add this prefix, which would otherwise break path
/// prefix comparisons against plain workspace paths.
fn strip_extended_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        let stripped = s.strip_prefix(r"\\?\").unwrap_or(&s);
        PathBuf::from(stripped.to_string())
    }
    #[cfg(not(windows))]
    {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("metteur-wsfs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rejects_absolute_escape() {
        let fs = WorkspaceFs::new(temp_workspace());
        #[cfg(windows)]
        let outside = r"C:\Windows\System32";
        #[cfg(not(windows))]
        let outside = "/etc/passwd";
        let err = fs.resolve(outside).unwrap_err();
        assert!(matches!(err, DaemonError::PermissionDenied(_)));
    }

    #[test]
    fn rejects_parent_traversal() {
        let root = temp_workspace();
        let fs = WorkspaceFs::new(root.clone());
        let escaped =
            root.parent().unwrap().join(format!("metteur-escape-{}", uuid::Uuid::new_v4()));
        let relative = format!("../{}", escaped.file_name().unwrap().to_string_lossy());
        let err = fs.resolve(&relative).unwrap_err();
        assert!(matches!(err, DaemonError::PermissionDenied(_)));
    }

    #[test]
    fn resolves_inside_workspace() {
        let root = temp_workspace();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let fs = WorkspaceFs::new(root.clone());
        let resolved = fs.resolve("src/main.rs").unwrap();
        assert_eq!(resolved, root.join("src/main.rs"));
    }

    #[test]
    fn read_write_within_root() {
        let root = temp_workspace();
        let fs = WorkspaceFs::new(root.clone());
        fs.write("a.txt", b"hello").unwrap();
        assert_eq!(fs.read("a.txt").unwrap(), b"hello");
    }
}
