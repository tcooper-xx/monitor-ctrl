use std::collections::HashMap;

use eframe::egui;

use crate::config::{AppConfig, CycleHotkey, DirectHotkey, MonitorConfig};
use crate::ddc::standard_inputs;
use crate::hotkeys::parse_hotkey;

/// DDC-discovered info about a monitor, passed into the settings window
#[derive(Debug, Clone)]
pub struct MonitorMeta {
    pub backend_id: String,
    /// Model name from DDC capabilities, or fallback to backend_id
    pub display_name: String,
    /// Inputs discovered from DDC capabilities string (empty = unknown)
    pub available_inputs: Vec<(String, u8)>,
}

#[derive(Debug)]
pub enum SettingsMsg {
    Saved(AppConfig),
    Closed,
    ExitRequested,
}

/// Outcome returned by `run_settings_window`.
pub enum SettingsOutcome {
    /// User clicked Save — contains the updated config.
    Saved(AppConfig),
    /// User closed/cancelled without saving.
    Cancelled,
    /// User triggered Exit from the tray while settings was open.
    ExitRequested,
}

/// One row in the Override inputs editor.
#[derive(Clone)]
struct InputEdit {
    label: String,
    code: u8,
    hex_str: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
enum Tab {
    #[default]
    General,
    Monitors,
    Hotkeys,
}

/// State for a single hotkey capture button.
#[derive(Default, Clone)]
struct HotkeyCapture {
    listening: bool,
}

pub struct SettingsApp {
    config: AppConfig,
    monitors: Vec<MonitorMeta>,
    tx: std::sync::mpsc::Sender<SettingsMsg>,
    hotkey_errors: HashMap<String, String>,
    /// Per-index error messages for cycle hotkey combos
    cycle_hotkey_errors: Vec<String>,
    /// Per-index error messages for direct hotkey combos
    direct_hotkey_errors: Vec<String>,
    /// Menu item ID of the tray Exit item — watched each frame to detect exit intent
    exit_item_id: String,
    /// Per-monitor input edit buffers (backend_id → rows).
    input_edits: HashMap<String, Vec<InputEdit>>,
    /// Currently selected settings tab
    selected_tab: Tab,
    /// Capture state for the General tab "Apply Default Inputs" hotkey
    capture_general: HotkeyCapture,
    /// Capture state per cycle hotkey (parallel to config.cycle_hotkeys)
    captures_cycle: Vec<HotkeyCapture>,
    /// Capture state per direct hotkey (parallel to config.direct_hotkeys)
    captures_direct: Vec<HotkeyCapture>,
}

impl SettingsApp {
    pub fn new(
        config: AppConfig,
        monitors: Vec<MonitorMeta>,
        tx: std::sync::mpsc::Sender<SettingsMsg>,
        exit_item_id: String,
    ) -> Self {
        let mut input_edits: HashMap<String, Vec<InputEdit>> = HashMap::new();
        for meta in &monitors {
            let edits = build_input_edits(&config, meta);
            input_edits.insert(meta.backend_id.clone(), edits);
        }

        let cycle_len = config.cycle_hotkeys.len();
        let direct_len = config.direct_hotkeys.len();
        Self {
            config,
            monitors,
            tx,
            hotkey_errors: HashMap::new(),
            cycle_hotkey_errors: vec![String::new(); cycle_len],
            direct_hotkey_errors: vec![String::new(); direct_len],
            exit_item_id,
            input_edits,
            selected_tab: Tab::default(),
            capture_general: HotkeyCapture::default(),
            captures_cycle: vec![HotkeyCapture::default(); cycle_len],
            captures_direct: vec![HotkeyCapture::default(); direct_len],
        }
    }

    fn validate_hotkeys(&mut self) {
        self.hotkey_errors.clear();
        for (action, combo) in &self.config.hotkeys.bindings {
            if !combo.is_empty() {
                if let Err(e) = parse_hotkey(combo) {
                    self.hotkey_errors.insert(action.clone(), e.to_string());
                }
            }
        }
        self.cycle_hotkey_errors.resize(self.config.cycle_hotkeys.len(), String::new());
        for (i, ch) in self.config.cycle_hotkeys.iter().enumerate() {
            self.cycle_hotkey_errors[i] = if ch.hotkey.is_empty() {
                String::new()
            } else {
                parse_hotkey(&ch.hotkey).err().map(|e| e.to_string()).unwrap_or_default()
            };
        }
        self.direct_hotkey_errors.resize(self.config.direct_hotkeys.len(), String::new());
        for (i, dh) in self.config.direct_hotkeys.iter().enumerate() {
            self.direct_hotkey_errors[i] = if dh.hotkey.is_empty() {
                String::new()
            } else {
                parse_hotkey(&dh.hotkey).err().map(|e| e.to_string()).unwrap_or_default()
            };
        }
    }

    /// Returns inputs to show for a monitor (edit buffer > DDC > standard fallback).
    fn inputs_for(&self, backend_id: &str) -> Vec<(String, u8)> {
        if let Some(edits) = self.input_edits.get(backend_id) {
            if !edits.is_empty() {
                return edits.iter().map(|e| (e.label.clone(), e.code)).collect();
            }
        }
        if let Some(meta) = self.monitors.iter().find(|m| m.backend_id == backend_id) {
            if meta.available_inputs.len() >= 2 {
                return meta.available_inputs.clone();
            }
        }
        standard_inputs().into_iter().map(|(l, c)| (l.to_string(), c)).collect()
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Check tray menu events every frame.
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if event.id.0 == self.exit_item_id {
                let _ = self.tx.send(SettingsMsg::ExitRequested);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }

        // ── Bottom panel: Save / Cancel ────────────────────────────────────
        egui::TopBottomPanel::bottom("actions_panel")
            .exact_height(48.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let can_save = self.hotkey_errors.is_empty()
                        && self.cycle_hotkey_errors.iter().all(|e| e.is_empty())
                        && self.direct_hotkey_errors.iter().all(|e| e.is_empty());
                    ui.add_enabled_ui(can_save, |ui| {
                        if ui
                            .add(
                                egui::Button::new("  Save  ")
                                    .fill(egui::Color32::from_rgb(0, 120, 212)),
                            )
                            .clicked()
                        {
                            // Sync input edit buffers back to config before saving
                            for (device_id, edits) in &self.input_edits {
                                let mon = self.config.monitors
                                    .entry(device_id.clone())
                                    .or_insert_with(MonitorConfig::default);
                                mon.inputs = edits.iter()
                                    .map(|e| (e.label.clone(), e.code))
                                    .collect();
                            }
                            let _ = self.tx.send(SettingsMsg::Saved(self.config.clone()));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                    if ui.button("Cancel").clicked() {
                        let _ = self.tx.send(SettingsMsg::Closed);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if !self.hotkey_errors.is_empty()
                        || self.cycle_hotkey_errors.iter().any(|e| !e.is_empty())
                        || self.direct_hotkey_errors.iter().any(|e| !e.is_empty())
                    {
                        ui.colored_label(
                            egui::Color32::from_rgb(200, 80, 80),
                            "Fix hotkey errors before saving",
                        );
                    }
                });
            });

        // ── Left panel: tab navigation ─────────────────────────────────────
        egui::SidePanel::left("nav_panel")
            .exact_width(90.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                let tab_btn = |ui: &mut egui::Ui, tab: Tab, label: &str,
                               selected: &mut Tab| {
                    let is_sel = *selected == tab;
                    if ui
                        .add_sized(
                            [ui.available_width(), 28.0],
                            egui::SelectableLabel::new(is_sel, label),
                        )
                        .clicked()
                    {
                        *selected = tab;
                    }
                };
                tab_btn(ui, Tab::General,  "General",  &mut self.selected_tab);
                tab_btn(ui, Tab::Monitors, "Monitors", &mut self.selected_tab);
                tab_btn(ui, Tab::Hotkeys,  "Hotkeys",  &mut self.selected_tab);
            });

        // ── Central panel: tab content ─────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Monitor Ctrl — Settings");
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(4.0);

                match self.selected_tab {
                    Tab::General => {
                        // ── General ───────────────────────────────────────
                        ui.add_space(4.0);
                        ui.checkbox(&mut self.config.start_at_login, "Start at Login");
                        ui.add_space(8.0);

                        let action_id = "all_default";
                        let error = self.hotkey_errors.get(action_id).cloned();
                        self.config.hotkeys.bindings
                            .entry(action_id.to_string())
                            .or_insert_with(String::new);
                        let combo = self.config.hotkeys.bindings.get_mut(action_id).unwrap();
                        let changed = ui.horizontal(|ui| {
                            ui.label("Apply Default Inputs hotkey:");
                            let r = hotkey_capture_ui(
                                ui,
                                &mut self.capture_general,
                                combo,
                                egui::Id::new(("hotkey", action_id)),
                            );
                            if let Some(err) = &error {
                                ui.colored_label(
                                    egui::Color32::from_rgb(200, 80, 80),
                                    format!("⚠ {err}"),
                                );
                            }
                            r
                        }).inner;
                        if changed { self.validate_hotkeys(); }
                        ui.add_space(4.0);
                    }

                    Tab::Monitors => {
                        // ── Monitors ──────────────────────────────────────
                        ui.add_space(4.0);
                        let metas: Vec<MonitorMeta> = self.monitors.clone();
                        for meta in &metas {
                            let device_id = &meta.backend_id;

                            let mon = self.config.monitors
                                .entry(device_id.clone())
                                .or_insert_with(|| MonitorConfig {
                                    name: meta.display_name.clone(),
                                    ..Default::default()
                                });
                            if mon.name == *device_id {
                                mon.name = meta.display_name.clone();
                            }

                            let has_unknown = meta.available_inputs.iter()
                                .any(|(label, _)| label.starts_with("Input 0x"));

                            let dropdown_inputs: Vec<(String, u8)> = self.inputs_for(device_id);

                            ui.group(|ui| {
                                let mon = self.config.monitors.get_mut(device_id).unwrap();

                                egui::Grid::new(("mon_grid", device_id.as_str()))
                                    .num_columns(2)
                                    .spacing([8.0, 6.0])
                                    .min_col_width(90.0)
                                    .show(ui, |ui| {
                                        ui.label("Name:");
                                        ui.add(
                                            egui::TextEdit::singleline(&mut mon.name)
                                                .id(egui::Id::new(("mon_name", device_id.as_str())))
                                                .desired_width(220.0),
                                        );
                                        ui.end_row();

                                        ui.label("Default input:");
                                        let current = mon.default_input.unwrap_or(0);
                                        let current_label = dropdown_inputs.iter()
                                            .find(|(_, c)| *c == current)
                                            .map(|(l, _)| l.as_str())
                                            .unwrap_or("None");
                                        egui::ComboBox::from_id_salt(
                                            ("default", device_id.as_str()),
                                        )
                                        .selected_text(current_label)
                                        .width(220.0)
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut mon.default_input,
                                                None,
                                                "None",
                                            );
                                            for (label, code) in &dropdown_inputs {
                                                ui.selectable_value(
                                                    &mut mon.default_input,
                                                    Some(*code),
                                                    label.as_str(),
                                                );
                                            }
                                        });
                                        ui.end_row();
                                    });

                                // Inputs — always visible, separated from name/default
                                ui.separator();
                                if has_unknown {
                                    ui.label(
                                        egui::RichText::new("Unknown inputs — rename below:")
                                            .color(egui::Color32::from_rgb(200, 160, 0)),
                                    );
                                } else {
                                    ui.label(egui::RichText::new("Inputs").weak());
                                }
                                ui.add_space(2.0);

                                let edits = self.input_edits.get_mut(device_id).unwrap();
                                let mon_hidden = &mut self.config.monitors
                                    .get_mut(device_id)
                                    .unwrap()
                                    .hidden_inputs;

                                const W_NAME: f32 = 130.0;
                                const W_CODE: f32 = 64.0;

                                egui::Grid::new(("inputs_grid", device_id.as_str()))
                                    .num_columns(4)
                                    .spacing([8.0, 4.0])
                                    .show(ui, |ui| {
                                        ui.add_sized(
                                            [W_NAME, ui.spacing().interact_size.y],
                                            egui::Label::new(egui::RichText::new("Name").strong()),
                                        );
                                        ui.add_sized(
                                            [W_CODE, ui.spacing().interact_size.y],
                                            egui::Label::new(egui::RichText::new("VCP Code").strong()),
                                        );
                                        ui.label(egui::RichText::new("Show in tray").strong());
                                        ui.label("");
                                        ui.end_row();

                                        for (i, edit) in edits.iter_mut().enumerate() {
                                            ui.add_sized(
                                                [W_NAME, ui.spacing().interact_size.y],
                                                egui::TextEdit::singleline(&mut edit.label)
                                                    .id(egui::Id::new((
                                                        "input_label",
                                                        device_id.as_str(),
                                                        i,
                                                    )))
                                                    .hint_text("e.g. HDMI-1"),
                                            );

                                            if ui.add_sized(
                                                [W_CODE, ui.spacing().interact_size.y],
                                                egui::TextEdit::singleline(&mut edit.hex_str)
                                                    .id(egui::Id::new((
                                                        "hex",
                                                        device_id.as_str(),
                                                        i,
                                                    )))
                                                    .hint_text("0x12"),
                                            ).changed() {
                                                if let Ok(parsed) = parse_hex(&edit.hex_str) {
                                                    edit.code = parsed;
                                                }
                                            }

                                            let mut visible = !mon_hidden.contains(&edit.code);
                                            if ui.checkbox(&mut visible, "").changed() {
                                                if visible {
                                                    mon_hidden.retain(|&c| c != edit.code);
                                                } else if !mon_hidden.contains(&edit.code) {
                                                    mon_hidden.push(edit.code);
                                                }
                                            }

                                            if ui.button("Reset")
                                                .on_hover_text("Resets the input to the detected values")
                                                .clicked()
                                            {
                                                let detected = meta.available_inputs.iter()
                                                    .find(|(_, c)| *c == edit.code)
                                                    .map(|(l, _)| l.clone())
                                                    .or_else(|| {
                                                        standard_inputs().into_iter()
                                                            .find(|(_, c)| *c == edit.code)
                                                            .map(|(l, _)| l.to_string())
                                                    })
                                                    .unwrap_or_else(|| format!("Input 0x{:02X}", edit.code));
                                                edit.label = detected;
                                                edit.hex_str = format!("0x{:02X}", edit.code);
                                            }
                                            ui.end_row();
                                        }
                                    });

                                if ui.small_button("+ Add input").clicked() {
                                    edits.push(InputEdit {
                                        label: format!("Input-{}", edits.len() + 1),
                                        hex_str: "0x11".to_string(),
                                        code: 0x11,
                                    });
                                }
                            });
                            ui.add_space(4.0);
                        }
                    }

                    Tab::Hotkeys => {
                        // ── Cycle Hotkeys ─────────────────────────────────
                        ui.add_space(4.0);
                        ui.strong("Cycle Hotkeys");
                        ui.label(
                            egui::RichText::new(
                                "A single key that flips one monitor between two inputs.",
                            )
                            .weak(),
                        );
                        ui.add_space(6.0);

                        let monitor_ids: Vec<String> = self.monitors.iter()
                            .map(|m| m.backend_id.clone())
                            .collect();
                        let mut to_remove: Option<usize> = None;
                        let cycle_len = self.config.cycle_hotkeys.len();

                        for idx in 0..cycle_len {
                            let error = self.cycle_hotkey_errors
                                .get(idx)
                                .cloned()
                                .unwrap_or_default();
                            let inputs = self.inputs_for(
                                &self.config.cycle_hotkeys[idx].monitor_id.clone(),
                            );

                            ui.group(|ui| {
                                egui::Grid::new(("cycle_grid", idx))
                                    .num_columns(2)
                                    .spacing([8.0, 6.0])
                                    .min_col_width(90.0)
                                    .show(ui, |ui| {
                                        // Monitor picker
                                        ui.label("Monitor:");
                                        let cur_id =
                                            self.config.cycle_hotkeys[idx].monitor_id.clone();
                                        let cur_name = self.monitors.iter()
                                            .find(|m| m.backend_id == cur_id)
                                            .map(|m| m.display_name.as_str())
                                            .unwrap_or(cur_id.as_str());
                                        egui::ComboBox::from_id_salt(("cycle_mon", idx))
                                            .selected_text(cur_name)
                                            .width(220.0)
                                            .show_ui(ui, |ui| {
                                                for mid in &monitor_ids {
                                                    let name = self.monitors.iter()
                                                        .find(|m| &m.backend_id == mid)
                                                        .map(|m| m.display_name.as_str())
                                                        .unwrap_or(mid.as_str());
                                                    let selected =
                                                        self.config.cycle_hotkeys[idx].monitor_id
                                                            == *mid;
                                                    if ui.selectable_label(selected, name).clicked() {
                                                        self.config.cycle_hotkeys[idx].monitor_id =
                                                            mid.clone();
                                                        let new_inputs = self.inputs_for(mid);
                                                        self.config.cycle_hotkeys[idx].input_a =
                                                            new_inputs.get(0).map(|(_, c)| *c).unwrap_or(0x11);
                                                        self.config.cycle_hotkeys[idx].input_b =
                                                            new_inputs.get(1).map(|(_, c)| *c).unwrap_or(0x0F);
                                                    }
                                                }
                                            });
                                        ui.end_row();

                                        // Input 1 picker
                                        ui.label("Input 1:");
                                        let cur_a = self.config.cycle_hotkeys[idx].input_a;
                                        let label_a = inputs.iter()
                                            .find(|(_, c)| *c == cur_a)
                                            .map(|(l, _)| l.as_str())
                                            .unwrap_or("?");
                                        egui::ComboBox::from_id_salt(("cycle_a", idx))
                                            .selected_text(label_a)
                                            .width(220.0)
                                            .show_ui(ui, |ui| {
                                                for (label, code) in &inputs {
                                                    if ui.selectable_label(
                                                        cur_a == *code,
                                                        label.as_str(),
                                                    ).clicked() {
                                                        self.config.cycle_hotkeys[idx].input_a = *code;
                                                    }
                                                }
                                            });
                                        ui.end_row();

                                        // Input 2 picker
                                        ui.label("Input 2:");
                                        let cur_b = self.config.cycle_hotkeys[idx].input_b;
                                        let label_b = inputs.iter()
                                            .find(|(_, c)| *c == cur_b)
                                            .map(|(l, _)| l.as_str())
                                            .unwrap_or("?");
                                        egui::ComboBox::from_id_salt(("cycle_b", idx))
                                            .selected_text(label_b)
                                            .width(220.0)
                                            .show_ui(ui, |ui| {
                                                for (label, code) in &inputs {
                                                    if ui.selectable_label(
                                                        cur_b == *code,
                                                        label.as_str(),
                                                    ).clicked() {
                                                        self.config.cycle_hotkeys[idx].input_b = *code;
                                                    }
                                                }
                                            });
                                        ui.end_row();

                                        // Hotkey capture
                                        ui.label("Hotkey:");
                                        ui.horizontal(|ui| {
                                            let capture = &mut self.captures_cycle[idx];
                                            let combo =
                                                &mut self.config.cycle_hotkeys[idx].hotkey;
                                            let changed = hotkey_capture_ui(
                                                ui,
                                                capture,
                                                combo,
                                                egui::Id::new(("cycle_hk", idx)),
                                            );
                                            if changed { self.validate_hotkeys(); }
                                            if ui.small_button("Remove").clicked() {
                                                to_remove = Some(idx);
                                            }
                                        });
                                        ui.end_row();
                                    });

                                let ch = &self.config.cycle_hotkeys[idx];
                                let mon_display = self.monitors.iter()
                                    .find(|m| m.backend_id == ch.monitor_id)
                                    .map(|m| m.display_name.as_str())
                                    .unwrap_or(ch.monitor_id.as_str());
                                let a_label = inputs.iter()
                                    .find(|(_, c)| *c == ch.input_a)
                                    .map(|(l, _)| l.as_str())
                                    .unwrap_or("?");
                                let b_label = inputs.iter()
                                    .find(|(_, c)| *c == ch.input_b)
                                    .map(|(l, _)| l.as_str())
                                    .unwrap_or("?");
                                let summary = if ch.hotkey.is_empty() {
                                    format!("{mon_display}: {a_label} ↔ {b_label}")
                                } else {
                                    format!(
                                        "{mon_display}: {a_label} ↔ {b_label}  [{}]",
                                        ch.hotkey
                                    )
                                };
                                ui.label(egui::RichText::new(summary).weak().small());

                                if !error.is_empty() {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(200, 80, 80),
                                        format!("⚠ {error}"),
                                    );
                                }
                            });
                            ui.add_space(4.0);
                        }

                        if let Some(idx) = to_remove {
                            self.config.cycle_hotkeys.remove(idx);
                            self.cycle_hotkey_errors
                                .resize(self.config.cycle_hotkeys.len(), String::new());
                            if idx < self.captures_cycle.len() {
                                self.captures_cycle.remove(idx);
                            }
                        }

                        if ui.button("+ Add cycle hotkey").clicked() {
                            let first_id = monitor_ids.first().cloned().unwrap_or_default();
                            let inputs = self.inputs_for(&first_id);
                            self.config.cycle_hotkeys.push(CycleHotkey {
                                monitor_id: first_id,
                                input_a: inputs.get(0).map(|(_, c)| *c).unwrap_or(0x11),
                                input_b: inputs.get(1).map(|(_, c)| *c).unwrap_or(0x0F),
                                hotkey: String::new(),
                            });
                            self.cycle_hotkey_errors.push(String::new());
                            self.captures_cycle.push(HotkeyCapture::default());
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);

                        // ── Per-Input Hotkeys ──────────────────────────────
                        ui.strong("Per-Input Hotkeys");
                        ui.label(
                            egui::RichText::new(
                                "Bind a key to switch a monitor directly to one specific input.",
                            )
                            .weak(),
                        );
                        ui.add_space(6.0);

                        let monitor_ids: Vec<String> = self.monitors.iter()
                            .map(|m| m.backend_id.clone())
                            .collect();
                        let mut to_remove: Option<usize> = None;
                        let direct_len = self.config.direct_hotkeys.len();

                        for idx in 0..direct_len {
                            let error = self.direct_hotkey_errors
                                .get(idx).cloned().unwrap_or_default();
                            let inputs = self.inputs_for(
                                &self.config.direct_hotkeys[idx].monitor_id.clone(),
                            );

                            ui.group(|ui| {
                                egui::Grid::new(("direct_grid", idx))
                                    .num_columns(2)
                                    .spacing([8.0, 6.0])
                                    .min_col_width(90.0)
                                    .show(ui, |ui| {
                                        // Monitor picker
                                        ui.label("Monitor:");
                                        let cur_id = self.config.direct_hotkeys[idx].monitor_id.clone();
                                        let cur_name = self.monitors.iter()
                                            .find(|m| m.backend_id == cur_id)
                                            .map(|m| m.display_name.as_str())
                                            .unwrap_or(cur_id.as_str());
                                        egui::ComboBox::from_id_salt(("direct_mon", idx))
                                            .selected_text(cur_name)
                                            .width(220.0)
                                            .show_ui(ui, |ui| {
                                                for mid in &monitor_ids {
                                                    let name = self.monitors.iter()
                                                        .find(|m| &m.backend_id == mid)
                                                        .map(|m| m.display_name.as_str())
                                                        .unwrap_or(mid.as_str());
                                                    let selected =
                                                        self.config.direct_hotkeys[idx].monitor_id == *mid;
                                                    if ui.selectable_label(selected, name).clicked() {
                                                        self.config.direct_hotkeys[idx].monitor_id = mid.clone();
                                                        let new_inputs = self.inputs_for(mid);
                                                        self.config.direct_hotkeys[idx].input =
                                                            new_inputs.first().map(|(_, c)| *c).unwrap_or(0x11);
                                                    }
                                                }
                                            });
                                        ui.end_row();

                                        // Input picker
                                        ui.label("Switch to:");
                                        let cur_input = self.config.direct_hotkeys[idx].input;
                                        let cur_label = inputs.iter()
                                            .find(|(_, c)| *c == cur_input)
                                            .map(|(l, _)| l.as_str())
                                            .unwrap_or("?");
                                        egui::ComboBox::from_id_salt(("direct_input", idx))
                                            .selected_text(cur_label)
                                            .width(220.0)
                                            .show_ui(ui, |ui| {
                                                for (label, code) in &inputs {
                                                    if ui.selectable_label(
                                                        cur_input == *code,
                                                        label.as_str(),
                                                    ).clicked() {
                                                        self.config.direct_hotkeys[idx].input = *code;
                                                    }
                                                }
                                            });
                                        ui.end_row();

                                        // Hotkey capture
                                        ui.label("Hotkey:");
                                        ui.horizontal(|ui| {
                                            let capture = &mut self.captures_direct[idx];
                                            let combo =
                                                &mut self.config.direct_hotkeys[idx].hotkey;
                                            let changed = hotkey_capture_ui(
                                                ui,
                                                capture,
                                                combo,
                                                egui::Id::new(("direct_hk", idx)),
                                            );
                                            if changed { self.validate_hotkeys(); }
                                            if ui.small_button("Remove").clicked() {
                                                to_remove = Some(idx);
                                            }
                                        });
                                        ui.end_row();
                                    });

                                let dh = &self.config.direct_hotkeys[idx];
                                let mon_display = self.monitors.iter()
                                    .find(|m| m.backend_id == dh.monitor_id)
                                    .map(|m| m.display_name.as_str())
                                    .unwrap_or(dh.monitor_id.as_str());
                                let input_label = inputs.iter()
                                    .find(|(_, c)| *c == dh.input)
                                    .map(|(l, _)| l.as_str())
                                    .unwrap_or("?");
                                let summary = if dh.hotkey.is_empty() {
                                    format!("{mon_display}: → {input_label}")
                                } else {
                                    format!("{mon_display}: → {input_label}  [{}]", dh.hotkey)
                                };
                                ui.label(egui::RichText::new(summary).weak().small());

                                if !error.is_empty() {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(200, 80, 80),
                                        format!("⚠ {error}"),
                                    );
                                }
                            });
                            ui.add_space(4.0);
                        }

                        if let Some(idx) = to_remove {
                            self.config.direct_hotkeys.remove(idx);
                            self.direct_hotkey_errors
                                .resize(self.config.direct_hotkeys.len(), String::new());
                            if idx < self.captures_direct.len() {
                                self.captures_direct.remove(idx);
                            }
                        }

                        if ui.button("+ Add per-input hotkey").clicked() {
                            let first_id = monitor_ids.first().cloned().unwrap_or_default();
                            let inputs = self.inputs_for(&first_id);
                            self.config.direct_hotkeys.push(DirectHotkey {
                                monitor_id: first_id,
                                input: inputs.first().map(|(_, c)| *c).unwrap_or(0x11),
                                hotkey: String::new(),
                            });
                            self.direct_hotkey_errors.push(String::new());
                            self.captures_direct.push(HotkeyCapture::default());
                        }
                        ui.add_space(8.0);
                    }
                }
            });
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.tx.send(SettingsMsg::Closed);
    }
}

/// Render a hotkey capture button.
///
/// Idle: shows the current combo (or "Click to set…") and an optional × clear
/// button. Listening: shows "Press a key…" highlighted; polls egui events for
/// a key+modifier combination (Ctrl or Alt required). Escape cancels.
/// Returns `true` when the combo string was changed.
fn hotkey_capture_ui(
    ui: &mut egui::Ui,
    capture: &mut HotkeyCapture,
    combo: &mut String,
    _id: egui::Id,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if capture.listening {
            ui.ctx().request_repaint();

            let events = ui.ctx().input(|i| i.events.clone());
            'event_loop: for event in &events {
                if let egui::Event::Key { key, modifiers, pressed: true, .. } = event {
                    if *key == egui::Key::Escape {
                        capture.listening = false;
                        break 'event_loop;
                    }
                    if let Some(token) = egui_key_to_token(*key) {
                        let mod_count = [modifiers.ctrl, modifiers.alt, modifiers.shift]
                            .iter()
                            .filter(|&&m| m)
                            .count();
                        // F-keys need only one modifier; letters/digits need two
                        // (blocks Ctrl+V, Shift+Z and other common combos)
                        let required = if is_function_key(*key) { 1 } else { 2 };
                        if mod_count >= required {
                            let mut parts: Vec<&str> = Vec::new();
                            if modifiers.ctrl  { parts.push("Ctrl"); }
                            if modifiers.alt   { parts.push("Alt"); }
                            if modifiers.shift { parts.push("Shift"); }
                            parts.push(token);
                            *combo = parts.join("+");
                            capture.listening = false;
                            changed = true;
                            break 'event_loop;
                        }
                    }
                }
            }

            // Highlighted "waiting" button; clicking cancels listening
            let btn = egui::Button::new("  Press a key…  ")
                .fill(egui::Color32::from_rgb(0, 80, 160));
            if ui
                .add_sized([175.0, ui.spacing().interact_size.y], btn)
                .clicked()
            {
                capture.listening = false;
            }
        } else {
            let display = if combo.is_empty() { "Click to set…" } else { combo.as_str() };
            if ui
                .add_sized(
                    [150.0, ui.spacing().interact_size.y],
                    egui::Button::new(display),
                )
                .clicked()
            {
                capture.listening = true;
            }
            if !combo.is_empty() && ui.small_button("×").on_hover_text("Clear hotkey").clicked() {
                combo.clear();
                changed = true;
            }
        }
    });
    changed
}

fn is_function_key(key: egui::Key) -> bool {
    matches!(
        key,
        egui::Key::F1  | egui::Key::F2  | egui::Key::F3  | egui::Key::F4
        | egui::Key::F5  | egui::Key::F6  | egui::Key::F7  | egui::Key::F8
        | egui::Key::F9  | egui::Key::F10 | egui::Key::F11 | egui::Key::F12
    )
}

/// Map an egui `Key` to the token string understood by `hotkeys::parse_key_code`.
/// Returns `None` for keys that aren't supported as hotkey targets (modifiers,
/// navigation keys, etc.).
fn egui_key_to_token(key: egui::Key) -> Option<&'static str> {
    use egui::Key::*;
    match key {
        A => Some("A"), B => Some("B"), C => Some("C"), D => Some("D"),
        E => Some("E"), F => Some("F"), G => Some("G"), H => Some("H"),
        I => Some("I"), J => Some("J"), K => Some("K"), L => Some("L"),
        M => Some("M"), N => Some("N"), O => Some("O"), P => Some("P"),
        Q => Some("Q"), R => Some("R"), S => Some("S"), T => Some("T"),
        U => Some("U"), V => Some("V"), W => Some("W"), X => Some("X"),
        Y => Some("Y"), Z => Some("Z"),
        Num0 => Some("0"), Num1 => Some("1"), Num2 => Some("2"),
        Num3 => Some("3"), Num4 => Some("4"), Num5 => Some("5"),
        Num6 => Some("6"), Num7 => Some("7"), Num8 => Some("8"),
        Num9 => Some("9"),
        F1  => Some("F1"),  F2  => Some("F2"),  F3  => Some("F3"),
        F4  => Some("F4"),  F5  => Some("F5"),  F6  => Some("F6"),
        F7  => Some("F7"),  F8  => Some("F8"),  F9  => Some("F9"),
        F10 => Some("F10"), F11 => Some("F11"), F12 => Some("F12"),
        _ => None,
    }
}

/// Build the initial input edit buffer for a monitor from whatever is stored in
/// the config.
fn build_input_edits(config: &AppConfig, meta: &MonitorMeta) -> Vec<InputEdit> {
    if let Some(mon) = config.monitors.get(&meta.backend_id) {
        if !mon.inputs.is_empty() {
            let mut v: Vec<InputEdit> = mon.inputs
                .iter()
                .map(|(l, &c)| InputEdit {
                    label: l.clone(),
                    hex_str: format!("0x{:02X}", c),
                    code: c,
                })
                .collect();
            v.sort_by_key(|e| e.code);
            return v;
        }
    }
    let source: Vec<(String, u8)> = if meta.available_inputs.len() >= 2 {
        meta.available_inputs.clone()
    } else {
        standard_inputs().into_iter().map(|(l, c)| (l.to_string(), c)).collect()
    };
    let mut v: Vec<InputEdit> = source
        .into_iter()
        .map(|(label, code)| InputEdit {
            hex_str: format!("0x{:02X}", code),
            label,
            code,
        })
        .collect();
    v.sort_by_key(|e| e.code);
    v
}

fn parse_hex(s: &str) -> Result<u8, std::num::ParseIntError> {
    let trimmed = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    u8::from_str_radix(trimmed, 16)
}

/// Launch the settings window on the current (main) thread. Blocks until closed.
pub fn run_settings_window(
    config: AppConfig,
    monitors: Vec<MonitorMeta>,
    exit_item_id: String,
) -> SettingsOutcome {
    let (tx, rx) = std::sync::mpsc::channel();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Monitor Ctrl — Settings")
            .with_inner_size([640.0, 620.0])
            .with_min_inner_size([480.0, 400.0])
            .with_resizable(true),
        ..Default::default()
    };

    let _ = eframe::run_native(
        "Monitor Ctrl — Settings",
        native_options,
        Box::new(move |_cc| {
            Ok(Box::new(SettingsApp::new(config, monitors, tx, exit_item_id)))
        }),
    );

    let mut outcome = SettingsOutcome::Cancelled;
    while let Ok(msg) = rx.try_recv() {
        match msg {
            SettingsMsg::ExitRequested => return SettingsOutcome::ExitRequested,
            SettingsMsg::Saved(mut cfg) => {
                cfg.prune_stale();
                outcome = SettingsOutcome::Saved(cfg);
            }
            SettingsMsg::Closed => {}
        }
    }
    outcome
}
