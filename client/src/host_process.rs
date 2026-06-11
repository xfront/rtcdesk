//! Host service process management.
//!
//! Spawns `rtcdesk-host` (or `rtcdesk-service`) as a child process with the
//! appropriate environment variables, monitors its lifecycle, and provides
//! stop/kill.  Also detects whether the host is already running, and supports
//! installing/uninstalling the host as an OS service:
//! - **Linux** — systemd user service (no root needed)
//! - **Windows** — Windows service via `sc.exe` (requires admin privileges)

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Arc, Mutex};

/// Manages the lifetime of a `rtcdesk-service` child process.
pub struct HostProcess {
    child: Option<Child>,
    /// Whether this process was spawned by the client (vs. pre-existing system service).
    /// Only kill on drop if we spawned it.
    spawned_by_client: bool,
}

impl HostProcess {
    /// Spawn the host-service binary.
    /// 
    /// The host-service reads all configuration from its config file
    /// (`~/.config/rtcdesk/rtcdesk.json`), NOT from environment variables.
    /// This ensures it works correctly as a systemd service or Windows service.
    /// 
    /// # Parameters
    /// * `node_id` - The host's unique identifier
    /// * `fps` - Streaming frame rate
    /// 
    /// # Returns
    /// HostProcess with `spawned_by_client = true`, meaning it will be killed
    /// when the client exits.
    pub fn spawn(
        node_id: &str,
        fps: u32,
    ) -> Result<Self, String> {
        let bin_path = find_service_binary()?;

        // Host-service reads config from ~/.config/rtcdesk/rtcdesk.json
        // No environment variables needed - config file is the single source of truth
        let mut cmd = std::process::Command::new(&bin_path);
        cmd.env("RTCDESK_ID", node_id);  // Only pass node_id for display/logging

        // Detach stdin/stdout so the child doesn't inherit our console
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| format!("无法启动 host 服务: {}", e))?;

        log::info!(
            "[HostProcess] Spawned host service (pid={})",
            child.id()
        );

        Ok(Self { 
            child: Some(child),
            spawned_by_client: true,  // Mark as spawned by client
        })
    }

    /// Check whether the child process is still running.
    pub fn is_running(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    log::info!(
                        "[HostProcess] rtcdesk-service exited: {}",
                        status
                    );
                    false
                }
                Ok(None) => true, // still running
                Err(e) => {
                    log::warn!(
                        "[HostProcess] try_wait error: {}",
                        e
                    );
                    false
                }
            }
        } else {
            false
        }
    }

    /// Create a HostProcess wrapper for an existing (pre-spawned) process.
    /// 
    /// This is used when the host was already running (e.g., system service)
    /// and we just want to track it WITHOUT killing it on client exit.
    pub fn from_existing() -> Self {
        Self {
            child: None,
            spawned_by_client: false,  // Don't kill on drop
        }
    }

    /// Stop the child process (kill + wait to reap).
    /// Only kills if this process was spawned by the client.
    pub fn stop(&mut self) {
        if !self.spawned_by_client {
            log::info!("[HostProcess] Skipping stop - host was not spawned by client");
            return;
        }

        if let Some(ref mut child) = self.child {
            let pid = child.id();
            log::info!("[HostProcess] Stopping rtcdesk-service (pid={})", pid);

            // Kill the process (on Unix this sends SIGKILL, on Windows it calls TerminateProcess)
            if let Err(e) = child.kill() {
                log::warn!("[HostProcess] kill() failed: {}", e);
            }

            // Wait to reap the zombie process
            match child.wait() {
                Ok(status) => {
                    log::info!("[HostProcess] Process exited: {}", status);
                }
                Err(e) => {
                    log::warn!("[HostProcess] wait() after kill failed: {}", e);
                }
            }

            self.child = None;
        }
    }

    /// Get the process ID, if running.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(|c| c.id())
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Thread-safe wrapper for `HostProcess` that can be shared across Dioxus signals.
pub type SharedHostProcess = Arc<Mutex<Option<HostProcess>>>;

/// Check whether the host service process is already running.
///
/// Uses platform-specific process-name matching:
/// - **Linux / macOS** — `pgrep -x` against the binary file-stem (and its
///   15‑char truncated form for the Linux `comm` limit).
/// - **Windows** — `tasklist /FI "IMAGENAME eq …"`.
/// - **Other** — always returns `false`.
pub fn is_host_running() -> bool {
    // Resolve the binary path so we know which names to look for.
    let names: Vec<String> = match find_service_binary_path() {
        Some(p) => {
            let stem = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("rtcdesk-host");
            let mut v = vec![stem.to_string()];
            // Linux procfs truncates comm to 15 chars.
            if stem.len() > 15 {
                v.push(stem[..15].to_string());
            }
            // Also check the alternative deployment name.
            if stem == "rtcdesk-host" {
                v.push("rtcdesk-service".into());
            } else if stem == "rtcdesk-service" {
                v.push("rtcdesk-host".into());
            }
            v
        }
        None => vec![
            "rtcdesk-host".into(),
            "rtcdesk-service".into(),
        ],
    };

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        for n in &names {
            if std::process::Command::new("pgrep")
                .args(["-x", n])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                log::info!("[is_host_running] found process '{n}' (pgrep)");
                return true;
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        for n in &names {
            let exe_name = format!("{n}.exe");
            if let Ok(output) = std::process::Command::new("tasklist")
                .args(["/FI", &format!("IMAGENAME eq {exe_name}")])
                .output()
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.contains(&exe_name) {
                    log::info!("[is_host_running] found process '{exe_name}' (tasklist)");
                    return true;
                }
            }
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = names;
    }

    false
}

/// Non-failing variant used by `is_host_running`.
fn find_service_binary_path() -> Option<PathBuf> {
    find_service_binary().ok()
}

/// Locate the host binary (`rtcdesk-host` or `rtcdesk-service`).
///
/// Search order:
/// 1. Same directory as current exe
/// 2. `PATH`
/// 3. Relative dev paths (`target/debug/`, `target/release/`)
fn find_service_binary() -> Result<PathBuf, String> {
    /// Helper: search a directory for candidate names.
    fn try_dir(dir: &Path, names: &[&str]) -> Option<PathBuf> {
        for name in names {
            let p = dir.join(name);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    let names: &[&str] = if cfg!(windows) {
        &["rtcdesk-host.exe", "rtcdesk-service.exe"]
    } else {
        &["rtcdesk-host", "rtcdesk-service"]
    };

    // 1. Same directory as current exe
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if let Some(p) = try_dir(dir, names) {
                return Ok(p);
            }
        }
    }

    // 2. Look in PATH
    if let Ok(path_var) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
            if let Some(p) = try_dir(Path::new(dir), names) {
                return Ok(p);
            }
        }
    }

    // 3. Relative path from CWD (useful during development)
    for build_dir in &["target/debug", "target/release"] {
        if let Some(p) = try_dir(Path::new(build_dir), names) {
            return Ok(p);
        }
    }

    Err(format!(
        "找不到 host 服务可执行文件 ({})。请确保已编译并放在同一目录或 PATH 中。",
        names.join(" / "),
    ))
}

// ─── Systemd user service management (Linux only) ─────────────────────────

/// Service name used for systemd.
#[cfg(target_os = "linux")]
const SYSTEMD_SERVICE_NAME: &str = "rtcdesk-host.service";

/// Path to thesystemd user unit directory.
#[cfg(target_os = "linux")]
fn systemd_user_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".config/systemd/user"))
}

/// Path to the environment file for the host service.
#[cfg(target_os = "linux")]
pub fn host_env_file_path() -> PathBuf {
    crate::config::config_dir()
        .map(|d| d.join("host-service.env"))
        .unwrap_or_else(|| PathBuf::from("host-service.env"))
}

/// Path to the systemd unit file.
#[cfg(target_os = "linux")]
fn unit_file_path() -> Option<PathBuf> {
    systemd_user_dir().map(|d| d.join(SYSTEMD_SERVICE_NAME))
}

/// Check if the rtcdesk-host systemd user service is installed.
#[cfg(target_os = "linux")]
pub fn is_systemd_service_installed() -> bool {
    unit_file_path().map(|p| p.exists()).unwrap_or(false)
}

/// Check if the rtcdesk-host systemd user service is currently active/running.
#[cfg(target_os = "linux")]
pub fn is_systemd_service_running() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", SYSTEMD_SERVICE_NAME])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Install the host binary as a systemd **user** service (no root needed).
///
/// Steps:
/// 1. Locate the host binary via `find_service_binary`.
/// 2. Write the environment file to `~/.config/rtcdesk/host-service.env`.
/// 3. Write the unit file to `~/.config/systemd/user/rtcdesk-host.service`.
/// 4. Run `systemctl --user daemon-reload`.
/// 5. Run `systemctl --user enable rtcdesk-host.service`.
/// 6. Run `systemctl --user start rtcdesk-host.service`.
///
/// Returns `Ok(())` on success, or `Err(String)` with diagnostic details.
#[cfg(target_os = "linux")]
pub fn install_systemd_service(
    node_id: &str,
    password: &str,
    stun: &[String],
    fps: u32,
) -> Result<(), String> {
    let bin_path = find_service_binary()?;

    // 1 — Ensure the user systemd directory exists
    let user_dir = systemd_user_dir().ok_or("无法获取 HOME 目录")?;
    std::fs::create_dir_all(&user_dir)
        .map_err(|e| format!("无法创建 systemd user 目录: {}", e))?;

    // 2 — Write environment file
    let env_path = host_env_file_path();
    if let Some(parent) = env_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let env_content = format!(
        "RTCDESK_ID={id}\n\
         RTCDESK_PASSWORD={pw}\n\
         RTCDESK_STUN={stun}\n\
         RTCDESK_FPS={fps}\n",
        id = node_id,
        pw = password,
        stun = stun.join(","),
        fps = fps,
    );
    std::fs::write(&env_path, &env_content)
        .map_err(|e| format!("无法写入环境配置文件: {}", e))?;

    // 3 — Write unit file
    let unit_path = unit_file_path().ok_or("无法确定 unit 文件路径")?;
    let unit_content = format!(
        "[Unit]\n\
         Description=RTCDesk Host Service\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={bin}\n\
         EnvironmentFile={env}\n\
         Restart=always\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        bin = bin_path.display(),
        env = env_path.display(),
    );
    std::fs::write(&unit_path, &unit_content)
        .map_err(|e| format!("无法写入 service 文件: {}", e))?;

    // 4 — daemon-reload
    run_systemctl(&["--user", "daemon-reload"])?;

    // 5 — enable
    run_systemctl(&["--user", "enable", SYSTEMD_SERVICE_NAME])?;

    // 6 — start
    run_systemctl(&["--user", "start", SYSTEMD_SERVICE_NAME])?;

    log::info!(
        "[systemd] Service installed and started: {} (bin={})",
        SYSTEMD_SERVICE_NAME,
        bin_path.display()
    );
    Ok(())
}

/// Uninstall the rtcdesk-host systemd user service.
///
/// Steps:
/// 1. Stop the service.
/// 2. Disable the service.
/// 3. Remove the unit file.
/// 4. Remove the environment file.
/// 5. daemon-reload.
#[cfg(target_os = "linux")]
pub fn uninstall_systemd_service() -> Result<(), String> {
    // 1 — stop (ignore errors if not running)
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "stop", SYSTEMD_SERVICE_NAME])
        .status();

    // 2 — disable (ignore errors if not enabled)
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", SYSTEMD_SERVICE_NAME])
        .status();

    // 3 — remove unit file
    if let Some(up) = unit_file_path() {
        if up.exists() {
            std::fs::remove_file(&up)
                .map_err(|e| format!("无法删除 service 文件: {}", e))?;
        }
    }

    // 4 — remove environment file
    let env_path = host_env_file_path();
    if env_path.exists() {
        let _ = std::fs::remove_file(&env_path);
    }

    // 5 — daemon-reload
    run_systemctl(&["--user", "daemon-reload"])?;

    log::info!("[systemd] Service uninstalled: {}", SYSTEMD_SERVICE_NAME);
    Ok(())
}

/// Synchronise the environment file with current settings.
/// Call this after the user changes host ID, password, or STUN servers.
#[cfg(target_os = "linux")]
pub fn update_systemd_env(
    node_id: &str,
    password: &str,
    stun: &[String],
    fps: u32,
) {
    let env_path = host_env_file_path();
    let env_content = format!(
        "RTCDESK_ID={id}\n\
         RTCDESK_PASSWORD={pw}\n\
         RTCDESK_STUN={stun}\n\
         RTCDESK_FPS={fps}\n",
        id = node_id,
        pw = password,
        stun = stun.join(","),
        fps = fps,
    );
    if let Some(parent) = env_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&env_path, &env_content) {
        log::warn!("[systemd] Failed to update env file: {}", e);
    } else {
        log::info!("[systemd] Env file updated: {}", env_path.display());
    }
}

/// Restart the systemd service (e.g. after updating env or binary).
#[cfg(target_os = "linux")]
pub fn restart_systemd_service() -> Result<(), String> {
    run_systemctl(&["--user", "restart", SYSTEMD_SERVICE_NAME])
}

/// Helper: run `systemctl` with given args and return Ok or Err with stderr output.
#[cfg(target_os = "linux")]
fn run_systemctl(args: &[&str]) -> Result<(), String> {
    let output = std::process::Command::new("systemctl")
        .args(args)
        .output()
        .map_err(|e| format!("执行 systemctl 失败: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(format!("systemctl {} 失败:\n{}", args.join(" "), if stderr.is_empty() { stdout } else { stderr }.to_string()))
    }
}

// ─── Windows service management (sc.exe) ──────────────────────────────────────

/// Service name used for Windows sc.exe.
#[cfg(target_os = "windows")]
const WINDOWS_SERVICE_NAME: &str = "rtcdesk-host";

#[cfg(target_os = "windows")]
pub fn host_env_file_path() -> PathBuf {
    crate::config::config_dir()
        .map(|d| d.join("rtcdesk").join("host-service.env"))
        .unwrap_or_else(|| PathBuf::from("host-service.env"))
}

/// Check if the rtcdesk-host Windows service is installed.
#[cfg(target_os = "windows")]
pub fn is_systemd_service_installed() -> bool {
    std::process::Command::new("sc")
        .args(["query", WINDOWS_SERVICE_NAME])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if the rtcdesk-host Windows service is running.
#[cfg(target_os = "windows")]
pub fn is_systemd_service_running() -> bool {
    std::process::Command::new("sc")
        .args(["query", WINDOWS_SERVICE_NAME])
        .output()
        .map(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.contains("RUNNING")
        })
        .unwrap_or(false)
}

/// Install the host binary as a Windows service (requires admin).
///
/// Steps:
/// 1. Locate the host binary via `find_service_binary`.
/// 2. Write the environment file to `%APPDATA%/rtcdesk/host-service.env`.
/// 3. Delete existing service if any.
/// 4. Create the service via `sc create … start= auto`.
/// 5. Start the service via `sc start …`.
#[cfg(target_os = "windows")]
pub fn install_systemd_service(
    node_id: &str,
    password: &str,
    stun: &[String],
    fps: u32,
) -> Result<(), String> {
    let bin_path = find_service_binary()?;

    // 1 — Write environment file
    let env_path = host_env_file_path();
    if let Some(parent) = env_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let env_content = format!(
        "RTCDESK_ID={id}\n\
         RTCDESK_PASSWORD={pw}\n\
         RTCDESK_STUN={stun}\n\
         RTCDESK_FPS={fps}\n",
        id = node_id,
        pw = password,
        stun = stun.join(","),
        fps = fps,
    );
    std::fs::write(&env_path, &env_content)
        .map_err(|e| format!("无法写入环境配置文件: {}", e))?;

    // 2 — Delete existing service if any, then create
    let _ = std::process::Command::new("sc")
        .args(["delete", WINDOWS_SERVICE_NAME])
        .status();
    std::thread::sleep(std::time::Duration::from_millis(300));

    // 3 — Create with auto-start (sc syntax: "binPath= " with a trailing space)
    let bin_escaped = format!("\"{}\"", bin_path.display());
    run_sc_create(&bin_escaped)?;

    // 4 — Start the service
    run_sc(&["start", WINDOWS_SERVICE_NAME])?;

    log::info!(
        "[windows] Service installed and started: {} (bin={})",
        WINDOWS_SERVICE_NAME,
        bin_path.display()
    );
    Ok(())
}

/// Create a Windows service with the given binary path.
#[cfg(target_os = "windows")]
fn run_sc_create(bin_path_quoted: &str) -> Result<(), String> {
    let output = std::process::Command::new("sc")
        .args(["create", WINDOWS_SERVICE_NAME, "binPath=", bin_path_quoted, "start=", "auto"])
        .output()
        .map_err(|e| format!("执行 sc create 失败: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let msg = if stderr.is_empty() { stdout } else { stderr }.to_string();
        if msg.contains("[SC] OpenService") && msg.contains("5") {
            Err("权限不足: 安装 Windows 服务需要以管理员身份运行".into())
        } else {
            Err(format!("sc create 失败:\n{}", msg))
        }
    }
}

/// Uninstall the rtcdesk-host Windows service.
///
/// Steps:
/// 1. Stop the service.
/// 2. Delete the service.
/// 3. Remove the environment file.
#[cfg(target_os = "windows")]
pub fn uninstall_systemd_service() -> Result<(), String> {
    // 1 — stop (ignore errors)
    let _ = std::process::Command::new("sc")
        .args(["stop", WINDOWS_SERVICE_NAME])
        .status();

    // 2 — delete
    run_sc(&["delete", WINDOWS_SERVICE_NAME])?;

    // 3 — remove environment file
    let env_path = host_env_file_path();
    if env_path.exists() {
        let _ = std::fs::remove_file(&env_path);
    }

    log::info!("[windows] Service uninstalled: {}", WINDOWS_SERVICE_NAME);
    Ok(())
}

/// Synchronise the environment file with current settings.
#[cfg(target_os = "windows")]
pub fn update_systemd_env(
    node_id: &str,
    password: &str,
    stun: &[String],
    fps: u32,
) {
    let env_path = host_env_file_path();
    let env_content = format!(
        "RTCDESK_ID={id}\n\
         RTCDESK_PASSWORD={pw}\n\
         RTCDESK_STUN={stun}\n\
         RTCDESK_FPS={fps}\n",
        id = node_id,
        pw = password,
        stun = stun.join(","),
        fps = fps,
    );
    if let Some(parent) = env_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&env_path, &env_content) {
        log::warn!("[windows] Failed to update env file: {}", e);
    } else {
        log::info!("[windows] Env file updated: {}", env_path.display());
    }
}

/// Restart the Windows service.
#[cfg(target_os = "windows")]
pub fn restart_systemd_service() -> Result<(), String> {
    // stop → brief wait → start
    let _ = std::process::Command::new("sc")
        .args(["stop", WINDOWS_SERVICE_NAME])
        .status();
    // Wait for service to fully stop
    for _ in 0..10 {
        if !is_systemd_service_running() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    run_sc(&["start", WINDOWS_SERVICE_NAME])?;
    Ok(())
}

/// Helper: run `sc.exe` with given args and return Ok or Err with stderr output.
#[cfg(target_os = "windows")]
fn run_sc(args: &[&str]) -> Result<(), String> {
    let output = std::process::Command::new("sc")
        .args(args)
        .output()
        .map_err(|e| format!("执行 sc 命令失败: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let msg = if stderr.is_empty() { stdout } else { stderr }.to_string();
        // Detect access denied
        if msg.contains("5") && (msg.contains("OpenSCManager") || msg.contains("OpenService")) {
            Err(format!("权限不足: 需要以管理员身份运行 (sc {} )", args.join(" ")))
        } else {
            Err(format!("sc {} 失败:\n{}", args.join(" "), msg))
        }
    }
}

// ─── Other platforms stubs ─────────────────────────────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn is_systemd_service_installed() -> bool { false }
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn is_systemd_service_running() -> bool { false }
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn install_systemd_service(_: &str, _: &str, _: &[String], _: u32) -> Result<(), String> {
    Err("当前系统不支持系统服务安装".into())
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn uninstall_systemd_service() -> Result<(), String> {
    Err("当前系统不支持系统服务安装".into())
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn update_systemd_env(_: &str, _: &str, _: &[String], _: u32) {}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn restart_systemd_service() -> Result<(), String> {
    Err("当前系统不支持系统服务".into())
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn host_env_file_path() -> PathBuf { PathBuf::from("host-service.env") }
