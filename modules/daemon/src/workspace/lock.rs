//! Workspace session locking.

use crate::error::{DaemonError, DaemonResult};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const LOCK_FILE: &str = "session.lock";
/// Kernel-owned exclusive lock: process exit releases ownership, PID reuse does not
/// imply ownership. Keep the inode in place; unlinking a held lock admits two owners.
pub struct SessionLock {
    _file: File,
}
impl SessionLock {
    pub fn acquire(metadata_dir: &Path) -> DaemonResult<Self> {
        fs::create_dir_all(metadata_dir)?;
        let path = metadata_dir.join(LOCK_FILE);
        // Never truncate before locking. Do not delete the lock on release.
        let mut file =
            OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        file.try_lock()
            .map_err(|e| DaemonError::Locked(format!("workspace lock unavailable: {e}")))?;
        let mut previous = String::new();
        (&mut file).take(256).read_to_string(&mut previous)?;
        // Older releases used a PID-only marker. Do not open beside a still
        // running legacy daemon during rolling upgrade. Unknown probe = busy.
        if !previous.starts_with("kernel-v1:") {
            if let Some(pid) = previous.split(':').next().and_then(|p| p.parse::<u32>().ok()) {
                if legacy_process_alive(pid) {
                    return Err(DaemonError::Locked(
                        "Legacy daemon may still own this workspace; stop it before upgrading"
                            .into(),
                    ));
                }
            }
        }
        file.rewind()?;
        file.set_len(0)?;
        write!(file, "kernel-v1:{}:{}", std::process::id(), now_millis())?;
        file.sync_data()?;
        Ok(Self {
            _file: file,
        })
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        // Explicit unlock also releases an fd transiently inherited by a
        // concurrently spawning child before exec closes it.
        let _ = self._file.unlock();
    }
}

fn legacy_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return true;
    }
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as i32, 0) == 0
                || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
        }
    }
    #[cfg(windows)]
    {
        match std::process::Command::new("tasklist.exe")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
        {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).lines().any(|l| {
                    l.split(',').nth(1).is_some_and(|s| s.trim_matches('"') == pid.to_string())
                })
            }
            _ => true,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        true
    }
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
        assert!(dir.join(LOCK_FILE).exists(), "lock inode must remain stable");
        let _reopened = SessionLock::acquire(&dir).unwrap();
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
    #[test]
    fn concurrent_owner_and_reused_pid_cannot_steal_lock() {
        let dir = std::env::temp_dir().join(format!("metteur-lock-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        // A stale diagnostic PID, even our own live PID, is not kernel ownership.
        fs::write(dir.join(LOCK_FILE), format!("kernel-v1:{}:0", std::process::id())).unwrap();
        let held = SessionLock::acquire(&dir).unwrap();
        std::thread::scope(|scope| {
            for _ in 0..16 {
                let dir = &dir;
                scope.spawn(move || assert!(SessionLock::acquire(dir).is_err()));
            }
        });
        drop(held);
        assert!(SessionLock::acquire(&dir).is_ok());
    }

    #[test]
    fn lock_child_process() {
        let Some(dir) = std::env::var_os("METTEUR_LOCK_CHILD_TEST") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let _lock = SessionLock::acquire(&dir).unwrap();
        fs::write(dir.join("child-ready"), "ready").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
    #[test]
    fn abrupt_exit_releases_kernel_ownership() {
        let dir = std::env::temp_dir().join(format!("metteur-lock-crash-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "workspace::lock::tests::lock_child_process", "--nocapture"])
            .env("METTEUR_LOCK_CHILD_TEST", &dir)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let start = std::time::Instant::now();
        while !dir.join("child-ready").exists()
            && start.elapsed() < std::time::Duration::from_secs(5)
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ready = dir.join("child-ready").exists();
        let blocked = ready && SessionLock::acquire(&dir).is_err();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(ready && blocked, "second process must not steal the workspace");
        let _recovered = SessionLock::acquire(&dir).unwrap();
    }
}
