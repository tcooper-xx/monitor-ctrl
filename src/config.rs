use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// VCP input code (u8) per label
pub type InputMap = HashMap<String, u8>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MonitorConfig {
    /// User-visible name
    pub name: String,
    /// VCP code for the default input
    pub default_input: Option<u8>,
    /// Known inputs: label → VCP code
    #[serde(default)]
    pub inputs: InputMap,
    /// VCP codes of inputs the user has chosen to hide from the tray menu
    #[serde(default)]
    pub hidden_inputs: Vec<u8>,
}

/// A hotkey that switches a monitor directly to one specific input.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DirectHotkey {
    /// backend_id of the monitor to control
    pub monitor_id: String,
    /// VCP input code to switch to
    pub input: u8,
    /// Key combo string, e.g. "Ctrl+Alt+1"
    pub hotkey: String,
}

/// A hotkey that cycles a single monitor between two inputs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CycleHotkey {
    /// backend_id of the monitor to control
    pub monitor_id: String,
    /// First input VCP code
    pub input_a: u8,
    /// Second input VCP code
    pub input_b: u8,
    /// Key combo string, e.g. "Ctrl+Alt+F1"
    pub hotkey: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HotkeyConfig {
    /// map of action-id → key combo string, e.g. "Ctrl+Alt+1"
    #[serde(flatten)]
    pub bindings: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    /// keyed by monitor device-id string
    #[serde(default)]
    pub monitors: HashMap<String, MonitorConfig>,
    #[serde(default)]
    pub hotkeys: HotkeyConfig,
    /// Input-cycle hotkeys
    #[serde(default)]
    pub cycle_hotkeys: Vec<CycleHotkey>,
    /// Direct per-input hotkeys (keyed by monitor_id, not index)
    #[serde(default)]
    pub direct_hotkeys: Vec<DirectHotkey>,
    /// whether to start at login
    #[serde(default)]
    pub start_at_login: bool,
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        // Alongside the executable
        let mut path = std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("."));
        path.pop();
        path.push("config.toml");
        path
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let config: Self = toml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        let text = toml::to_string_pretty(self)
            .context("serializing config")?;
        std::fs::write(&path, text)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Get or insert a default MonitorConfig for a device id
    #[allow(dead_code)]
    pub fn monitor_mut(&mut self, device_id: &str) -> &mut MonitorConfig {
        self.monitors
            .entry(device_id.to_string())
            .or_default()
    }

    /// Remove config entries that shouldn't persist:
    /// - Internal LVDS/eDP panel entries (never controllable via DDC)
    /// - Legacy index-based hotkey bindings (mon{n}_{code} format) that are
    ///   now superseded by `direct_hotkeys`
    pub fn prune_stale(&mut self) {
        self.monitors.retain(|id, _| !is_internal_id(id));
        // Keep only the "all_default" binding; index-based mon{n}_{code}
        // keys are superseded by direct_hotkeys.
        self.hotkeys.bindings.retain(|key, _| key == "all_default");
    }
}

fn is_internal_id(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    lower.contains(":lvds") || lower.contains(":edp")
}
