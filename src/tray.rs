use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Result};
use tray_icon::{
    menu::{
        CheckMenuItemBuilder, Menu, MenuEvent, MenuItemBuilder, PredefinedMenuItem, Submenu,
    },
    TrayIcon, TrayIconBuilder,
};

use crate::config::AppConfig;
use crate::ddc::DdcDisplay;

/// A built tray + menu, with mappings back to actions
pub struct AppTray {
    #[allow(dead_code)]
    pub tray: TrayIcon, // must stay alive — dropping it removes the tray icon
    /// Maps menu item id (String from MenuId) → action
    pub item_actions: HashMap<String, TrayAction>,
    /// The menu item ID of the Exit item — used to detect exit while settings is open
    pub exit_item_id: String,
}

#[derive(Debug, Clone)]
pub enum TrayAction {
    SwitchInput { monitor_idx: usize, input_code: u8 },
    SetAllDefault,
    OpenSettings,
    Exit,
}

impl AppTray {
    pub fn build(
        config: &AppConfig,
        displays: &[DdcDisplay],
        icon_path: &Path,
    ) -> Result<Self> {
        let menu = Menu::new();
        let mut item_actions: HashMap<String, TrayAction> = HashMap::new();

        // Per-monitor sections
        for (idx, display) in displays.iter().enumerate() {
            let mon_config = config.monitors.get(&display.backend_id);
            let default_name = display.model_name
                .as_deref()
                .unwrap_or(&display.backend_id);
            // Skip the config name if it was never user-customised (i.e. it equals the
            // raw backend_id, which is the auto-generated placeholder value).
            let mon_name = mon_config
                .and_then(|m| {
                    let n = m.name.trim();
                    if n.is_empty() || n == display.backend_id.as_str() { None } else { Some(n) }
                })
                .unwrap_or(default_name);

            // Submenu header showing monitor name and current input
            let current_input_str = display.current_input
                .map(|c| crate::ddc::input_label(c))
                .unwrap_or_else(|| "Unknown".to_string());
            let header_text = format!("{mon_name} — {current_input_str}");
            let submenu = Submenu::new(header_text.as_str(), true);

            // Input priority: config override > DDC capabilities > 4-input fallback
            let inputs: Vec<(String, u8)> = if let Some(mc) = mon_config {
                if !mc.inputs.is_empty() {
                    // User has manually defined inputs in Settings
                    let mut v: Vec<(String, u8)> = mc.inputs
                        .iter()
                        .map(|(k, &v)| (k.clone(), v))
                        .collect();
                    v.sort_by_key(|(_, c)| *c);
                    v
                } else if display.available_inputs.len() >= 2 {
                    display.available_inputs.clone()
                } else {
                    fallback_inputs()
                }
            } else if display.available_inputs.len() >= 2 {
                display.available_inputs.clone()
            } else {
                fallback_inputs()
            };

            let hidden = mon_config
                .map(|m| m.hidden_inputs.as_slice())
                .unwrap_or(&[]);

            for (label, code) in inputs {
                if hidden.contains(&code) {
                    continue;
                }
                let is_active = display.current_input == Some(code);
                let item = CheckMenuItemBuilder::new()
                    .text(label.as_str())
                    .enabled(true)
                    .checked(is_active)
                    .build();
                let id: String = item.id().0.clone();
                item_actions.insert(id, TrayAction::SwitchInput {
                    monitor_idx: idx,
                    input_code: code,
                });
                submenu.append(&item).ok();
            }
            
            menu.append(&submenu).ok();
        }

        menu.append(&PredefinedMenuItem::separator()).ok();

        let any_default = config.monitors.values().any(|m| m.default_input.is_some());
        let set_default = MenuItemBuilder::new()
            .text("Apply Default Inputs")
            .enabled(any_default)
            .build();
        item_actions.insert(set_default.id().0.clone(), TrayAction::SetAllDefault);
        menu.append(&set_default).ok();

        menu.append(&PredefinedMenuItem::separator()).ok();

        let settings = MenuItemBuilder::new()
            .text("Settings...")
            .enabled(true)
            .build();
        item_actions.insert(settings.id().0.clone(), TrayAction::OpenSettings);
        menu.append(&settings).ok();

        let exit_item = MenuItemBuilder::new()
            .text("Exit")
            .enabled(true)
            .build();
        let exit_item_id = exit_item.id().0.clone();
        item_actions.insert(exit_item_id.clone(), TrayAction::Exit);
        menu.append(&exit_item).ok();

        let icon = load_icon(icon_path)?;

        let tooltip = build_tooltip(config, displays);
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(tooltip)
            .with_icon(icon)
            .build()
            .map_err(|e| anyhow!("tray build failed: {e:?}"))?;

        Ok(AppTray { tray, item_actions, exit_item_id })
    }

    pub fn action_for_menu_event(&self, event: &MenuEvent) -> Option<&TrayAction> {
        self.item_actions.get(&event.id.0)
    }
}

/// Build a tooltip string showing each monitor's current input, e.g.
/// "Monitor Ctrl\nDell: HDMI-1 | Generic PnP: DP-1"
fn build_tooltip(config: &AppConfig, displays: &[DdcDisplay]) -> String {
    use crate::ddc::input_label;
    let parts: Vec<String> = displays.iter().map(|d| {
        let mon_config = config.monitors.get(&d.backend_id);
        let default_name = d.model_name.as_deref().unwrap_or(&d.backend_id);
        let name = mon_config
            .and_then(|m| {
                let n = m.name.trim();
                if n.is_empty() || n == d.backend_id.as_str() { None } else { Some(n) }
            })
            .unwrap_or(default_name);
        let input = d.current_input
            .map(|c| input_label(c))
            .unwrap_or_else(|| "Unknown".into());
        format!("{name}: {input}")
    }).collect();
    let any_default = config.monitors.values().any(|m| m.default_input.is_some());
    let all_on_default = any_default && displays.iter().all(|d| {
        match config.monitors.get(&d.backend_id) {
            Some(mc) if mc.default_input.is_some() => d.current_input == mc.default_input,
            _ => true,
        }
    });

    if parts.is_empty() {
        "Monitor Ctrl".into()
    } else if all_on_default {
        format!("Monitor Ctrl\n{} ✓ defaults", parts.join(" | "))
    } else {
        format!("Monitor Ctrl\n{}", parts.join(" | "))
    }
}

/// Last-resort fallback when DDC capabilities are unavailable
fn fallback_inputs() -> Vec<(String, u8)> {
    vec![
        ("HDMI-1".into(), 0x11),
        ("HDMI-2".into(), 0x12),
        ("DP-1".into(),   0x0F),
        ("DP-2".into(),   0x10),
    ]
}

fn load_icon(path: &Path) -> Result<tray_icon::Icon> {
    if path.exists() {
        if let Ok(icon) = tray_icon::Icon::from_path(path, Some((32, 32))) {
            return Ok(icon);
        }
    }
    // Fallback: render a monitor shape directly as RGBA pixels
    tray_icon::Icon::from_rgba(monitor_icon_rgba(), 32, 32)
        .map_err(|e| anyhow!("create fallback icon failed: {e:?}"))
}

/// Generate a 32×32 RGBA monitor icon programmatically.
/// Used as a fallback when icon.ico is not found on disk.
fn monitor_icon_rgba() -> Vec<u8> {
    const W: usize = 32;
    let mut buf = vec![[0u8; 4]; W * W];

    let fr = [58u8,  58,  58,  255]; // bezel
    let sc = [12u8,  22,  38,  255]; // screen dark
    let gl = [26u8, 107, 154, 255];  // screen glow
    let hl = [91u8, 164, 207, 255];  // highlight
    let st = [78u8,  78,  78,  255]; // stand

    let mut fill = |x0: usize, y0: usize, x1: usize, y1: usize, c: [u8; 4]| {
        for y in y0..=y1 { for x in x0..=x1 { buf[y * W + x] = c; } }
    };

    fill(1, 1, 30, 21, fr);
    fill(3, 3, 28, 19, sc);
    fill(4, 4, 27, 18, gl);
    fill(4, 4, 27,  8, hl);
    fill(6,10, 22, 17, sc);
    fill(14,22, 17, 25, st);
    fill(8, 26, 23, 27, st);

    buf.iter().flat_map(|p| *p).collect()
}
