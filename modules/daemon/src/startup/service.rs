//! Windows Service Control Manager (SCM) integration.
//!
//! The daemon can register itself as a native Windows service
//! (`--install-service`), unregister it (`--uninstall-service`), or run as a
//! service under the SCM (`--service`). Registration needs administrator
//! rights; when the current process is not elevated, the install/uninstall
//! commands re-execute with a UAC prompt instead of failing.

use crate::cli::Cli;
use crate::error::{DaemonError, DaemonResult};

/// The service name registered with the SCM.
pub const SERVICE_NAME: &str = "metteurd";

#[cfg(windows)]
windows_service::define_windows_service!(ffi_service_main, service_main);

/// Blocks serving the daemon under the SCM until the service is stopped.
pub fn run_service() -> DaemonResult<()> {
    #[cfg(windows)]
    {
        windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .map_err(|e| DaemonError::Internal(format!("service dispatcher failed: {e}")))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(DaemonError::Internal("--service is only supported on Windows".to_string()))
    }
}

/// Registers the daemon as a Windows service with automatic start.
pub fn install_service(cli: &Cli) -> DaemonResult<()> {
    #[cfg(windows)]
    {
        if !is_elevated() {
            return reexec_elevated();
        }
        use std::ffi::OsString;

        use windows_service::service::{
            ServiceAccess, ServiceDependency, ServiceErrorControl, ServiceInfo, ServiceStartType,
            ServiceType,
        };
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

        let exe = std::env::current_exe()?;
        let mut launch = vec![OsString::from("--service")];
        if let Some(config) = &cli.config {
            launch.push(OsString::from("--config"));
            launch.push(config.clone().into_os_string());
        }
        if let Some(data_dir) = &cli.data_dir {
            launch.push(OsString::from("--data-dir"));
            launch.push(data_dir.clone().into_os_string());
        }

        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )?;
        let info = ServiceInfo {
            name: SERVICE_NAME.into(),
            display_name: SERVICE_DISPLAY_NAME.into(),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe,
            launch_arguments: launch,
            dependencies: Vec::<ServiceDependency>::new(),
            account_name: None,
            account_password: None,
        };

        // Creating over an existing service fails; drop any stale entry first.
        delete_if_exists(&manager)?;
        manager
            .create_service(&info, ServiceAccess::CHANGE_CONFIG)
            .map_err(|e| DaemonError::Internal(format!("cannot create service: {e}")))?;
        tracing::info!("registered Windows service '{SERVICE_NAME}'");
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = cli;
        Err(DaemonError::Internal(
            "registering a Windows service is only supported on Windows".to_string(),
        ))
    }
}

/// Removes the registered Windows service.
pub fn uninstall_service() -> DaemonResult<()> {
    #[cfg(windows)]
    {
        if !is_elevated() {
            return reexec_elevated();
        }
        use windows_service::service::{ServiceAccess, ServiceState};
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let Ok(service) = manager.open_service(
            SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
        ) else {
            tracing::info!("Windows service '{SERVICE_NAME}' is not installed");
            return Ok(());
        };
        if let Ok(status) = service.query_status()
            && status.current_state != ServiceState::Stopped
        {
            let _ = service.stop();
        }
        service
            .delete()
            .map_err(|e| DaemonError::Internal(format!("cannot delete service: {e}")))?;
        tracing::info!("removed Windows service '{SERVICE_NAME}'");
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(DaemonError::Internal("--uninstall-service is only supported on Windows".to_string()))
    }
}

/// Starts the installed Windows service if it is not already running.
pub fn start_service() -> DaemonResult<()> {
    #[cfg(windows)]
    {
        use windows_service::service::{ServiceAccess, ServiceState};
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager
            .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::START)?;
        if service.query_status()?.current_state == ServiceState::Running {
            tracing::info!("Windows service '{SERVICE_NAME}' is already running");
            return Ok(());
        }
        service
            .start(&[] as &[&str])
            .map_err(|e| DaemonError::Internal(format!("cannot start service: {e}")))?;
        tracing::info!("started Windows service '{SERVICE_NAME}'");
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(DaemonError::Internal("--start-service is only supported on Windows".to_string()))
    }
}

/// Stops the installed Windows service if it is running.
pub fn stop_service() -> DaemonResult<()> {
    #[cfg(windows)]
    {
        use windows_service::service::{ServiceAccess, ServiceState};
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager
            .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP)?;
        if service.query_status()?.current_state == ServiceState::Stopped {
            tracing::info!("Windows service '{SERVICE_NAME}' is already stopped");
            return Ok(());
        }
        service.stop().map_err(|e| DaemonError::Internal(format!("cannot stop service: {e}")))?;
        tracing::info!("stopped Windows service '{SERVICE_NAME}'");
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(DaemonError::Internal("--stop-service is only supported on Windows".to_string()))
    }
}

/// Prints the current state of the installed Windows service to stdout.
pub fn service_status() -> DaemonResult<()> {
    #[cfg(windows)]
    {
        use windows_service::service::{ServiceAccess, ServiceState};
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)?;
        let status = service.query_status()?;
        let name = match status.current_state {
            ServiceState::Stopped => "stopped",
            ServiceState::StartPending => "start pending",
            ServiceState::StopPending => "stop pending",
            ServiceState::Running => "running",
            ServiceState::ContinuePending => "continue pending",
            ServiceState::PausePending => "pause pending",
            ServiceState::Paused => "paused",
        };
        println!("{SERVICE_NAME}: {name}");
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(DaemonError::Internal("--service-status is only supported on Windows".to_string()))
    }
}

/// Returns whether the daemon Windows service is currently registered with the
/// SCM. Used to make config-driven service autostart idempotent.
pub fn is_installed() -> DaemonResult<bool> {
    #[cfg(windows)]
    {
        use windows_service::service::ServiceAccess;
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        Ok(manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS).is_ok())
    }
    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

#[cfg(windows)]
fn delete_if_exists(
    manager: &windows_service::service_manager::ServiceManager,
) -> DaemonResult<()> {
    use windows_service::service::ServiceAccess;
    if let Ok(service) = manager.open_service(SERVICE_NAME, ServiceAccess::DELETE) {
        service
            .delete()
            .map_err(|e| DaemonError::Internal(format!("cannot remove existing service: {e}")))?;
    }
    Ok(())
}

/// The Windows service entry point executed by the SCM dispatcher.
#[cfg(windows)]
fn service_main(args: Vec<std::ffi::OsString>) {
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceStatus,
        ServiceType as StatusServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};

    let (re_tx, re_rx) = std::sync::mpsc::channel::<()>();
    let notify = re_tx;

    let status_handle =
        match service_control_handler::register(SERVICE_NAME, move |control| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = notify.send(());
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }) {
            Ok(handle) => handle,
            Err(e) => {
                tracing::error!("cannot register service control handler: {e}");
                return;
            }
        };

    let set_status =
        |status_handle: &windows_service::service_control_handler::ServiceStatusHandle, state| {
            let _ = status_handle.set_service_status(ServiceStatus {
                service_type: StatusServiceType::OWN_PROCESS,
                current_state: state,
                controls_accepted: ServiceControlAccept::STOP,
                exit_code: ServiceExitCode::NO_ERROR,
                checkpoint: 0,
                wait_hint: std::time::Duration::from_secs(3),
                process_id: None,
            });
        };
    set_status(&status_handle, windows_service::service::ServiceState::StartPending);

    let cli = match cli_from_args(&args) {
        Ok(cli) => cli,
        Err(e) => {
            tracing::error!("cannot parse service arguments: {e}");
            set_status(&status_handle, windows_service::service::ServiceState::Stopped);
            return;
        }
    };

    // service_main does not run on a Tokio runtime; build a dedicated one.
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!("cannot build tokio runtime: {e}");
            set_status(&status_handle, windows_service::service::ServiceState::Stopped);
            return;
        }
    };

    // Bridge the SCM stop signal to the async shutdown channel.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    std::thread::spawn(move || {
        while re_rx.recv().is_ok() {
            let _ = shutdown_tx.send(true);
        }
    });

    set_status(&status_handle, windows_service::service::ServiceState::Running);
    let result = runtime.block_on(crate::startup::run(cli, Some(shutdown_rx)));
    set_status(&status_handle, windows_service::service::ServiceState::Stopped);
    if let Err(e) = result {
        tracing::error!("daemon stopped with error: {e}");
    }
}

/// Re-parses the daemon CLI from the arguments handed to the service.
#[cfg(windows)]
fn cli_from_args(args: &[std::ffi::OsString]) -> DaemonResult<Cli> {
    use std::iter;

    use clap::Parser;
    let full = iter::once("metteurd".to_string())
        .chain(args.iter().filter(|a| *a != "--service").map(|a| a.to_string_lossy().into_owned()));
    Cli::try_parse_from(full)
        .map_err(|e| DaemonError::Internal(format!("invalid service arguments: {e}")))
}

#[cfg(windows)]
const SERVICE_DISPLAY_NAME: &str = "Metteur Daemon";

#[cfg(windows)]
fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION {
            TokenIsElevated: 0,
        };
        let mut size = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        ) != 0;
        CloseHandle(token);
        let _ = size;
        ok && elevation.TokenIsElevated != 0
    }
}

/// Re-executes the current command line with a UAC elevation prompt. Returns
/// normally once the elevated process has been launched.
#[cfg(windows)]
fn reexec_elevated() -> DaemonResult<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;

    let exe = std::env::current_exe()?;
    let params = std::env::args()
        .skip(1)
        .map(|a| {
            let lower = a.to_lowercase();
            if lower.contains(' ') || lower.contains('"') {
                format!("\"{}\"", a.replace('"', r#"\""#))
            } else {
                a
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let verb = to_wide("runas");
    let exe_w = to_wide(&exe.to_string_lossy());
    // The empty-params case must still hold a live Vec so the pointer stays
    // valid for the duration of the `ShellExecuteW` call below.
    let params_wide = if params.is_empty() {
        Vec::new()
    } else {
        to_wide(&params)
    };
    let params_w = if params_wide.is_empty() {
        std::ptr::null()
    } else {
        params_wide.as_ptr()
    };

    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            exe_w.as_ptr(),
            params_w,
            std::ptr::null(),
            5, // SW_SHOW
        )
    };
    let status = result as isize;
    if status > 32 {
        Ok(())
    } else {
        Err(DaemonError::Internal(format!("UAC elevation was not granted (code {status})")))
    }
}

#[cfg(windows)]
fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn cli_from_args_drops_service_flag() {
        use std::ffi::OsString;
        use std::path::Path;

        let os = |s: &str| OsString::from(s);
        let args = vec![os("--service"), os("--config"), os("C:\\metteur\\config.toml")];
        let cli = cli_from_args(&args).unwrap();
        assert_eq!(cli.config.as_deref(), Some(Path::new("C:\\metteur\\config.toml")));
        assert!(!cli.service);
    }

    #[cfg(windows)]
    #[test]
    fn wide_encoding_terminates_with_null() {
        let wide = to_wide("runas");
        assert_eq!(wide.last(), Some(&0));
    }
}
