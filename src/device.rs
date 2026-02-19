//! Device/Server detection module.
//!
//! Detects whether the TUI is running on a local device or an SSH server,
//! and resolves a human-friendly name with zero user configuration.
//!
//! **Server detection**: Parses `SSH_CONNECTION` env var, then queries editor
//! CLIs (code/cursor/windsurf/antigravity) in parallel to extract the SSH
//! host alias from `"Extensions installed on SSH: <alias>:"`.
//!
//! **Local detection**: Uses platform-specific APIs to get a friendly device name
//! (Windows `COMPUTERNAME`, macOS `scutil`, Linux `gethostname`).
//!
//! Results are cached to `~/.cache/opencode-stats-tui/device.json` so subsequent
//! launches resolve in <1ms without spawning any subprocess.

use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// Cached global device info — resolved once, accessible everywhere.
static DEVICE: OnceLock<DeviceInfo> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Human-friendly name: SSH alias ("mail") or device name ("DESKTOP-HP")
    pub name: String,
    /// "server" or "device"
    pub kind: String,
    /// SSH user (only set for servers)
    #[serde(default)]
    pub user: Option<String>,
    /// Stable machine identifier for syncing — never changes across reboots,
    /// hostname changes, or IP changes. Read from /etc/machine-id (Linux),
    /// IOPlatformUUID (macOS), or MachineGuid registry (Windows).
    #[serde(default)]
    pub machine_id: Option<String>,
}

impl DeviceInfo {
    /// Formatted display: "mail (docker_user)" for servers, "hostname (OS)" for local
    pub fn display_name(&self) -> String {
        match (&self.kind, &self.user) {
            (k, Some(u)) if k == "server" => format!("{} ({})", self.name, u),
            _ => {
                // Local device: show hostname + OS
                let os = get_os_name();
                format!("{} ({})", self.name, os)
            }
        }
    }

    /// Label for display: "Local" or "Server"
    pub fn display_label(&self) -> &'static str {
        if self.kind == "server" {
            "Server"
        } else {
            "Local"
        }
    }
}

/// Get the device info (cached after first call).
#[inline]
pub fn get_device_info() -> &'static DeviceInfo {
    DEVICE.get_or_init(detect_device)
}

/// Top-level detection: check cache → detect → persist.
fn detect_device() -> DeviceInfo {
    // 1. Try cache first (<1ms)
    if let Some(cached) = load_cache() {
        return cached;
    }

    // 2. Detect
    let info = if env::var_os("SSH_CONNECTION").is_some() {
        detect_server()
    } else {
        detect_local()
    };

    // 3. Persist for next launch
    save_cache(&info);
    info
}

// ─── Server Detection ────────────────────────────────────────────────────────

/// Editor CLI binaries to probe, ordered by market share.
/// Each entry: (binary_name, server_dir_name)
const EDITOR_CLIS: &[(&str, &str)] = &[
    ("code", ".vscode-server"),
    ("cursor", ".cursor-server"),
    ("windsurf", ".windsurf-server"),
    ("antigravity", ".antigravity-server"),
];

fn detect_server() -> DeviceInfo {
    let user = env::var("USER").ok().filter(|s| !s.is_empty());
    let machine_id = get_machine_id();

    // Fast path: check which editors are installed (dir exists), then probe in parallel
    if let Some(alias) = probe_editor_clis() {
        return DeviceInfo {
            name: alias,
            kind: "server".into(),
            user,
            machine_id,
        };
    }

    // Fallback: USER@hostname
    let hostname = get_hostname();
    let name = match &user {
        Some(u) if hostname.is_empty() || hostname.len() > 20 => u.clone(),
        Some(u) => format!("{}@{}", u, hostname),
        None => hostname,
    };
    DeviceInfo {
        name,
        kind: "server".into(),
        user,
        machine_id,
    }
}

/// Probe editor CLIs in parallel. Returns first SSH alias found.
///
/// Strategy:
///   1. Filter to editors whose `~/.<server-dir>` exists (instant fs check)
///   2. For each, find the CLI binary (either in PATH or server dir)
///   3. Spawn all candidates in parallel with a tight timeout
///   4. First response with a valid alias wins; kill the rest
fn probe_editor_clis() -> Option<String> {
    let home = env::var("HOME").ok()?;

    // Collect candidates whose server dir exists
    let candidates: Vec<(&str, Option<PathBuf>)> = EDITOR_CLIS
        .iter()
        .filter_map(|(name, server_dir)| {
            let dir = PathBuf::from(&home).join(server_dir);
            if !dir.is_dir() {
                return None;
            }
            // Find CLI binary: prefer `which` result, else scan server dir
            let bin = find_editor_binary(name, &dir);
            Some((*name, bin))
        })
        .collect();

    if candidates.is_empty() {
        return None;
    }

    // Spawn all candidates in parallel using threads
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    let (tx, rx) = mpsc::channel::<String>();

    for (name, bin_path) in &candidates {
        let tx = tx.clone();
        let cli = bin_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| (*name).to_string());

        thread::spawn(move || {
            if let Some(alias) = run_editor_cli(&cli) {
                let _ = tx.send(alias);
            }
        });
    }
    drop(tx); // Close sender so rx.recv_timeout doesn't hang

    // Wait for first result with a tight deadline
    rx.recv_timeout(Duration::from_millis(1500)).ok()
}

/// Find the editor CLI binary. Check PATH first (instant), then scan server dir.
fn find_editor_binary(name: &str, server_dir: &PathBuf) -> Option<PathBuf> {
    // Fast: check if it's in PATH
    if let Ok(output) = Command::new("which").arg(name).output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }

    // Scan: find bin/remote-cli/<name> in server dir
    let bin_dir = server_dir.join("bin");
    if let Ok(entries) = fs::read_dir(&bin_dir) {
        for entry in entries.flatten() {
            let cli_path = entry.path().join("bin").join("remote-cli").join(name);
            if cli_path.is_file() {
                return Some(cli_path);
            }
        }
    }

    // VSCode uses a different layout: cli/servers/*/server/bin/remote-cli/code
    let cli_dir = server_dir.join("cli").join("servers");
    if let Ok(entries) = fs::read_dir(&cli_dir) {
        for entry in entries.flatten() {
            let cli_path = entry
                .path()
                .join("server")
                .join("bin")
                .join("remote-cli")
                .join(name);
            if cli_path.is_file() {
                return Some(cli_path);
            }
        }
    }

    None
}

/// Run a single editor CLI and extract the SSH alias from its output.
/// Parses: `"Extensions installed on SSH: <alias>:"`
fn run_editor_cli(cli: &str) -> Option<String> {
    let output = Command::new(cli)
        .arg("--list-extensions")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next()?;

    // Parse: "Extensions installed on SSH: mail:"
    let marker = "SSH: ";
    let start = first_line.find(marker)? + marker.len();
    let rest = &first_line[start..];
    let end = rest.find(':')?;
    let alias = rest[..end].trim();

    if alias.is_empty() {
        None
    } else {
        Some(alias.to_string())
    }
}

// ─── Local Detection ─────────────────────────────────────────────────────────

fn detect_local() -> DeviceInfo {
    let name = get_local_device_name();
    DeviceInfo {
        name,
        kind: "device".into(),
        user: None,
        machine_id: get_machine_id(),
    }
}

/// Platform-specific friendly device name.
fn get_local_device_name() -> String {
    // Windows: COMPUTERNAME env var → "DESKTOP-ABC123"
    #[cfg(target_os = "windows")]
    {
        if let Ok(name) = env::var("COMPUTERNAME") {
            if !name.is_empty() {
                return name;
            }
        }
    }

    // macOS: `scutil --get ComputerName` → "Hoang's MacBook Pro"
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("scutil")
            .args(["--get", "ComputerName"])
            .output()
        {
            if output.status.success() {
                let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !name.is_empty() {
                    return name;
                }
            }
        }
    }

    // Linux / fallback: gethostname
    get_hostname()
}

/// Get a human-friendly OS name for display.
fn get_os_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "Windows"
    }
    #[cfg(target_os = "macos")]
    {
        "macOS"
    }
    #[cfg(target_os = "linux")]
    {
        "Linux"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        "Unknown"
    }
}

/// POSIX gethostname via libc (already in Cargo.toml).
#[cfg(unix)]
fn get_hostname() -> String {
    let mut buf = vec![0u8; 256];
    let ret = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if ret == 0 {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    } else {
        String::from("unknown")
    }
}

#[cfg(not(unix))]
fn get_hostname() -> String {
    env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

// ─── Machine ID ──────────────────────────────────────────────────────────────

/// Stable, unique machine identifier that never changes across reboots,
/// hostname/IP changes, or SSH config differences.
fn get_machine_id() -> Option<String> {
    // Linux: /etc/machine-id (systemd) or /var/lib/dbus/machine-id
    #[cfg(target_os = "linux")]
    {
        for path in &["/etc/machine-id", "/var/lib/dbus/machine-id"] {
            if let Ok(id) = fs::read_to_string(path) {
                let id = id.trim().to_string();
                if !id.is_empty() {
                    return Some(id);
                }
            }
        }
    }

    // macOS: IOPlatformUUID via ioreg
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if line.contains("IOPlatformUUID") {
                        if let Some(uuid) = line.split('"').nth(3) {
                            return Some(uuid.to_string());
                        }
                    }
                }
            }
        }
    }

    // Windows: MachineGuid from registry
    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = Command::new("reg")
            .args([
                "query",
                r"HKLM\SOFTWARE\Microsoft\Cryptography",
                "/v",
                "MachineGuid",
            ])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(line) = stdout.lines().find(|l| l.contains("MachineGuid")) {
                    if let Some(guid) = line.split_whitespace().last() {
                        return Some(guid.to_string());
                    }
                }
            }
        }
    }

    None
}

// ─── Cache ───────────────────────────────────────────────────────────────────

fn cache_path() -> PathBuf {
    let cache_dir = env::var("XDG_CACHE_HOME").unwrap_or_else(|_| {
        let home = env::var("HOME").unwrap_or_else(|_| ".".into());
        format!("{}/.cache", home)
    });
    PathBuf::from(cache_dir)
        .join("opencode-stats-tui")
        .join("device.json")
}

fn load_cache() -> Option<DeviceInfo> {
    let path = cache_path();
    let data = fs::read(&path).ok()?;
    let info: DeviceInfo = serde_json::from_slice(&data).ok()?;
    // Validate: non-empty name
    if info.name.is_empty() {
        return None;
    }
    // Invalidate stale cache missing user detail
    if info.kind == "server" && info.user.is_none() {
        let _ = fs::remove_file(&path);
        return None;
    }
    Some(info)
}

fn save_cache(info: &DeviceInfo) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(data) = serde_json::to_vec_pretty(info) {
        let _ = fs::write(&path, data);
    }
}
