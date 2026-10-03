//! Workspace session locking.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{DaemonError, DaemonResult};

/// The name of the session lock file inside the metadata directory.
pub const LOCK_FILE: &str = "session.lock";

/// A held workspace session lock.
///
/// The lock is released when dropped.
pub struct SessionLock {
    path: PathBuf,
}

impl SessionLock {
    /// Acquires the lock for the given metadata directory.
    ///
    /// If a stale lock exists (its PID is no longer alive) it is cleared
    /// before acquiring. Returns an error if the lock is held by a live
    /// process.
    pub fn acquire(metadata_dir: &Path) -> DaemonResult<Self> {
        fs::create_dir_all(metadata_dir)?;
        let path = metadata_dir.join(LOCK_FILE);

        if path.exists() {
            if let Some(pid) = read_pid(&path)
                && process_alive(pid)
            {
                return Err(DaemonError::Locked(format!("workspace is locked by pid {pid}")));
            }
            // Stale lock: clear it.
            fs::remove_file(&path)?;
        }

        let content = format!("{}:{}", std::process::id(), now_millis());
        fs::write(&path, content)?;
        Ok(Self {
            path,
        })
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Reads the PID from a lock file, if it is well-formed.
fn read_pid(path: &Path) -> Option<u32> {
    let content = fs::read_to_string(path).ok()?;
    let pid = content.split(':').next()?;
    pid.parse().ok()
}

/// Returns whether a process with the given PID is alive.
fn process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        // On Windows, query the process list to check for existence.
        let status = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output();
        match status {
            Ok(out) => String::from_utf8_lossy(&out.stdout).contains(&pid.to_string()),
            Err(_) => true,
        }
    }
    #[cfg(not(windows))]
    {
        // Sending signal 0 checks for process existence without delivering a
        // signal.
        libc_kill(pid, 0) == 0
    }
}

#[cfg(not(windows))]
fn libc_kill(pid: u32, sig: i32) -> i32 {
    // Minimal libc binding to avoid an extra dependency.
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { kill(pid as i32, sig) }
}

/// Returns the current time in milliseconds since the Unix epoch.
fn now_millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquires_and_releases_lock() {
        let dir = std::env::temp_dir().join(format!("metteur-lock-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        {
            let lock = SessionLock::acquire(&dir).unwrap();
            assert!(dir.join(LOCK_FILE).exists());
            drop(lock);
        }
        assert!(!dir.join(LOCK_FILE).exists());
    }

    #[test]
    fn clears_stale_lock() {
        let dir = std::env::temp_dir().join(format!("metteur-lock-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        // A lock referencing a PID that cannot exist.
        fs::write(dir.join(LOCK_FILE), "99999999:0").unwrap();
        let lock = SessionLock::acquire(&dir).unwrap();
        assert!(dir.join(LOCK_FILE).exists());
        drop(lock);
    }
}
