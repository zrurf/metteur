//! Register the daemon to start automatically at login.
//!
//! Driven by `[daemon].autostart`, the daemon self-registers a native login
//! entry so it is available before any client connects: a HKCU `Run` value on
//! Windows, a LaunchAgent plist on macOS, and an XDG autostart `.desktop` file
//! on Linux. The entry always launches `metteurd --passive`, letting the CLI
//! wake mechanism bring it to full service on demand.

use std::path::PathBuf;

use crate::error::DaemonResult;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use crate::error::DaemonError;

/// The name used for the autostart entry on every platform.
pub const ENTRY_NAME: &str = "metteurd";

/// Arguments stored in the autostart entry.
#[derive(Debug, Clone)]
pub struct AutostartEnv {
    /// Path to the `metteurd` executable.
    pub exe: PathBuf,
    /// Explicit config file to forward, if any.
    pub config: Option<PathBuf>,
    /// Global data directory to forward, if any.
    pub data_dir: Option<PathBuf>,
}

/// Registers the daemon to start at login, overwriting any existing entry.
pub fn install(env: &AutostartEnv) -> DaemonResult<()> {
    #[cfg(target_os = "windows")]
    {
        install_windows(env)
    }
    #[cfg(target_os = "macos")]
    {
        install_macos(env)
    }
    #[cfg(target_os = "linux")]
    {
        install_linux(env)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err(DaemonError::Internal("autostart is not supported on this platform".to_string()))
    }
}

/// Removes the login autostart entry if present.
pub fn uninstall() -> DaemonResult<()> {
    #[cfg(target_os = "windows")]
    {
        uninstall_windows()
    }
    #[cfg(target_os = "macos")]
    {
        uninstall_macos()
    }
    #[cfg(target_os = "linux")]
    {
        uninstall_linux()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err(DaemonError::Internal("autostart is not supported on this platform".to_string()))
    }
}

/// Reports whether the login autostart entry is registered.
pub fn is_installed() -> bool {
    #[cfg(target_os = "windows")]
    {
        is_installed_windows()
    }
    #[cfg(target_os = "macos")]
    {
        is_installed_macos()
    }
    #[cfg(target_os = "linux")]
    {
        is_installed_linux()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// Builds the command-line string stored in the Windows `Run` value.
#[cfg(any(target_os = "windows", test))]
fn windows_command_line(env: &AutostartEnv) -> String {
    let mut cmd = format!("\"{}\" --passive", env.exe.display());
    cmd.push_str(&flag_arg("--config", env.config.as_deref()));
    cmd.push_str(&flag_arg("--data-dir", env.data_dir.as_deref()));
    cmd
}

/// Appends a `--flag "value"` pair, quoting values that contain whitespace.
fn flag_arg(flag: &str, value: Option<&std::path::Path>) -> String {
    match value {
        Some(path) => {
            let text = path.to_string_lossy();
            let quoted = if text.contains(' ') || text.contains('"') {
                format!("\"{}\"", text.replace('"', r#"\""#))
            } else {
                text.to_string()
            };
            format!(" {flag} {quoted}")
        }
        None => String::new(),
    }
}

#[cfg(target_os = "windows")]
fn install_windows(env: &AutostartEnv) -> DaemonResult<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let run_key = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(run_key)
        .map_err(|e| DaemonError::Internal(format!("cannot open Run key {run_key}: {e}")))?;
    key.set_value(ENTRY_NAME, &windows_command_line(env))
        .map_err(|e| DaemonError::Internal(format!("cannot write autostart value: {e}")))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn uninstall_windows() -> DaemonResult<()> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS};

    let _ = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Run", KEY_ALL_ACCESS)
        .and_then(|key| key.delete_value(ENTRY_NAME));
    Ok(())
}

#[cfg(target_os = "windows")]
fn is_installed_windows() -> bool {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};

    match RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Run", KEY_READ)
    {
        Ok(key) => key.get_value::<String, _>(ENTRY_NAME).is_ok(),
        Err(_) => false,
    }
}

/// The `plist` written as a macOS LaunchAgent.
#[cfg(any(target_os = "macos", test))]
fn macos_plist(env: &AutostartEnv) -> String {
    let mut args = format!(
        "<string>{}</string><string>--passive</string>",
        xml_escape(&env.exe.to_string_lossy())
    );
    if let Some(config) = &env.config {
        args.push_str(&format!(
            "<string>--config</string><string>{}</string>",
            xml_escape(&config.to_string_lossy())
        ));
    }
    if let Some(data_dir) = &env.data_dir {
        args.push_str(&format!(
            "<string>--data-dir</string><string>{}</string>",
            xml_escape(&data_dir.to_string_lossy())
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\n\
         <key>Label</key><string>{name}</string>\n\
         <key>ProgramArguments</key><array>{args}</array>\n\
         <key>RunAtLoad</key><true/>\n\
         </dict></plist>\n",
        name = ENTRY_NAME,
        args = args
    )
}

/// The `.desktop` autostart file content for Linux.
#[cfg(any(target_os = "linux", test))]
fn linux_desktop(env: &AutostartEnv) -> String {
    let mut exec = format!("\"{}\" --passive", env.exe.display());
    exec.push_str(&flag_arg("--config", env.config.as_deref()));
    exec.push_str(&flag_arg("--data-dir", env.data_dir.as_deref()));
    format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\nX-GNOME-Autostart-enabled=true\n",
        name = ENTRY_NAME,
        exec = exec
    )
}

#[cfg(target_os = "macos")]
fn launch_agents_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Library").join("LaunchAgents")
}

#[cfg(target_os = "macos")]
fn macos_plist_path() -> PathBuf {
    launch_agents_dir().join(format!("{ENTRY_NAME}.plist"))
}

#[cfg(target_os = "macos")]
fn install_macos(env: &AutostartEnv) -> DaemonResult<()> {
    let path = macos_plist_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, macos_plist(env))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_macos() -> DaemonResult<()> {
    let path = macos_plist_path();
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn is_installed_macos() -> bool {
    macos_plist_path().exists()
}

#[cfg(target_os = "linux")]
fn autostart_dir() -> PathBuf {
    let config_home = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(std::env::temp_dir)
    });
    config_home.join("autostart")
}

#[cfg(target_os = "linux")]
fn linux_desktop_path() -> PathBuf {
    autostart_dir().join(format!("{ENTRY_NAME}.desktop"))
}

#[cfg(target_os = "linux")]
fn install_linux(env: &AutostartEnv) -> DaemonResult<()> {
    let path = linux_desktop_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, linux_desktop(env))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_linux() -> DaemonResult<()> {
    let path = linux_desktop_path();
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn is_installed_linux() -> bool {
    linux_desktop_path().exists()
}

/// Escapes a value for inclusion in a plist `<string>` element.
#[cfg(any(target_os = "macos", test))]
fn xml_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(exe: &str) -> AutostartEnv {
        AutostartEnv {
            exe: PathBuf::from(exe),
            config: None,
            data_dir: None,
        }
    }

    #[test]
    fn windows_command_quotes_paths() {
        let e = AutostartEnv {
            exe: PathBuf::from(r"C:\Program Files\Metteur\metteurd.exe"),
            config: Some(PathBuf::from(r"C:\Program Files\Metteur\config.toml")),
            data_dir: Some(PathBuf::from(r"C:\no space")),
        };
        let cmd = windows_command_line(&e);
        assert!(cmd.contains(r#""C:\Program Files\Metteur\metteurd.exe" --passive"#));
        assert!(cmd.contains(r#"--config "C:\Program Files\Metteur\config.toml""#));
        assert!(cmd.contains(r#"--data-dir "C:\no space""#));
    }

    #[test]
    fn windows_command_has_no_args_when_none() {
        let cmd = windows_command_line(&env(r"C:\metteurd.exe"));
        assert_eq!(cmd, r#""C:\metteurd.exe" --passive"#);
    }

    #[test]
    fn linux_desktop_contains_passive() {
        let content = linux_desktop(&env("/opt/metteur/metteurd"));
        assert!(content.contains("Exec=\"/opt/metteur/metteurd\" --passive"));
        assert!(content.contains("Name=metteurd"));
    }

    #[test]
    fn macos_plist_escapes_and_lists_args() {
        let content = macos_plist(&env("/opt/Metteur Daemon/metteurd"));
        assert!(content.contains("<string>/opt/Metteur Daemon/metteurd</string>"));
        assert!(content.contains("<string>--passive</string>"));
        assert!(content.contains("<key>RunAtLoad</key><true/>"));
    }
}
