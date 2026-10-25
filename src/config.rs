use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    pub device_id: String,
    pub device_name: String,
    /// Shared secret. Devices only see each other when these match.
    pub room_key: String,
    pub tcp_port: u16,
    pub disc_port: u16,
    pub save_dir: Option<PathBuf>,
    pub show_devices: bool,
    /// How much history to keep. Nothing is written to disk, but an app left
    /// open for weeks should not grow without limit either.
    #[serde(default = "default_max_messages")]
    pub max_messages: usize,
    #[serde(default = "default_max_files")]
    pub max_files: usize,
    /// Take new versions automatically from other devices in the same room.
    #[serde(default = "default_true")]
    pub auto_update: bool,
    /// Which skin the app wears.
    #[serde(default)]
    pub theme: crate::theme::Theme,
    /// Background colour for the high-contrast skin.
    #[serde(default = "default_background")]
    pub background: [u8; 3],
    /// Reach devices that are not on this network - over a VPN such as
    /// Tailscale, or a different subnet. Off unless asked for, because it
    /// sends this device's name and presence to an address beyond the LAN.
    #[serde(default)]
    pub remote_peers_enabled: bool,
    /// Addresses or hostnames of those devices, as typed.
    #[serde(default)]
    pub remote_peers: Vec<String>,
    /// Bounce the Dock icon, flash the taskbar and badge the icon with a
    /// count when something arrives while the window is in the background.
    #[serde(default = "default_true")]
    pub notify: bool,
}

fn default_background() -> [u8; 3] {
    [16, 16, 18]
}

pub const MESSAGE_LIMITS: [usize; 3] = [50, 100, 150];
pub const FILE_LIMITS: [usize; 3] = [5, 10, 15];

fn default_max_messages() -> usize {
    100
}
fn default_max_files() -> usize {
    10
}
fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config {
            device_id: crate::crypto::random_hex(8),
            device_name: default_device_name(),
            room_key: String::new(),
            tcp_port: 45455,
            disc_port: 45454,
            save_dir: downloads_dir(),
            show_devices: true,
            max_messages: default_max_messages(),
            max_files: default_max_files(),
            auto_update: default_true(),
            theme: crate::theme::Theme::default(),
            background: default_background(),
            remote_peers_enabled: false,
            remote_peers: Vec::new(),
            notify: default_true(),
        }
    }
}

pub fn default_device_name() -> String {
    #[cfg(windows)]
    {
        if let Ok(n) = std::env::var("COMPUTERNAME") {
            if !n.is_empty() {
                return n;
            }
        }
    }
    if let Ok(out) = std::process::Command::new("hostname").output() {
        let n = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !n.is_empty() {
            return n.split('.').next().unwrap_or("Device").to_string();
        }
    }
    "Device".into()
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn downloads_dir() -> Option<PathBuf> {
    let d = home().join("Downloads");
    d.is_dir().then_some(d)
}

pub fn config_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/NetPaste")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home)
            .join("NetPaste")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"))
            .join("netpaste")
    }
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

impl Config {
    pub fn load() -> Config {
        std::fs::read(config_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(config_dir());
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(config_path(), json);
        }
    }
}
