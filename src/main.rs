#![windows_subsystem = "windows"]

mod config;
mod ddc;
mod hotkeys;
mod settings_ui;
mod startup;
mod tray;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use log::{error, info, warn};
use tray_icon::menu::MenuEvent;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
};

use config::AppConfig;
use ddc::{enumerate_ddc_displays, read_input, set_input, DdcDisplay};
use hotkeys::HotkeyManager;
use settings_ui::{MonitorMeta, SettingsOutcome};
use tray::{AppTray, TrayAction};

fn main() {
    // Tell Windows that this process supports dark mode so that Win32 popup
    // menus (the tray context menu) automatically use the system theme.
    // SetPreferredAppMode is an undocumented uxtheme.dll export at ordinal 135;
    // value 1 = AllowDark (follow system preference).
    unsafe { apply_dark_mode(); }

    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .init();

    info!("Monitor Ctrl starting");

    let mut config = AppConfig::load().unwrap_or_else(|e| {
        warn!("Failed to load config: {e}. Using defaults.");
        AppConfig::default()
    });

    if config.start_at_login != startup::is_enabled() {
        if let Err(e) = startup::set_enabled(config.start_at_login) {
            warn!("Could not sync auto-launch: {e}");
        }
    }

    let mut displays = enumerate_ddc_displays();
    info!("{} DDC/CI display(s) found", displays.len());

    let mut hotkey_mgr = match HotkeyManager::new() {
        Ok(m) => m,
        Err(e) => { error!("Hotkey manager init failed: {e}"); std::process::exit(1); }
    };
    register_hotkeys(&mut hotkey_mgr, &config);

    let icon_path = icon_path();
    let mut app_tray = match AppTray::build(&config, &displays, &icon_path) {
        Ok(t) => t,
        Err(e) => { error!("Failed to build tray: {e}"); std::process::exit(1); }
    };

    // Poll interval: read VCP 0x60 periodically to catch manual switches and
    // failed DDC commands so the tray checkmark stays accurate.
    const POLL_INTERVAL: Duration = Duration::from_secs(3);
    let mut last_poll = Instant::now();

    // Main loop — pump Win32 messages so tray-icon's hidden window on this
    // thread receives events, then drain the resulting event channels.
    loop {
        // Pump all pending Win32 messages for windows owned by this thread
        // (tray-icon creates its hidden tray window on the calling thread)
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // Drain tray menu events
        while let Ok(menu_event) = MenuEvent::receiver().try_recv() {
            if let Some(action) = app_tray.action_for_menu_event(&menu_event).cloned() {
                match action {
                    TrayAction::Exit => {
                        info!("Exit requested");
                        return;
                    }
                    TrayAction::OpenSettings => {
                        // Unregister hotkeys before the window opens so that:
                        //  1. Already-bound combos can be captured in the UI
                        //     (OS would otherwise intercept them before egui sees them)
                        //  2. Events can't accumulate and fire once the window closes
                        hotkey_mgr.unregister_all();

                        let monitor_metas: Vec<MonitorMeta> = displays
                            .iter()
                            .map(|d| MonitorMeta {
                                backend_id: d.backend_id.clone(),
                                display_name: d.model_name.clone()
                                    .unwrap_or_else(|| d.backend_id.clone()),
                                available_inputs: d.available_inputs.clone(),
                            })
                            .collect();
                        // Blocks on main thread — tray events are handled inside the
                        // settings window each frame, including Exit.
                        match settings_ui::run_settings_window(
                            config.clone(),
                            monitor_metas,
                            app_tray.exit_item_id.clone(),
                        ) {
                            SettingsOutcome::ExitRequested => {
                                info!("Exit requested while settings was open");
                                return;
                            }
                            SettingsOutcome::Saved(new_cfg) => {
                                config = new_cfg;
                                if let Err(e) = config.save() {
                                    error!("Failed to save config: {e}");
                                }
                                if let Err(e) = startup::set_enabled(config.start_at_login) {
                                    warn!("auto-launch update failed: {e}");
                                }
                                rebuild_tray(&mut app_tray, &config, &displays, &icon_path);
                                info!("Settings saved");
                            }
                            SettingsOutcome::Cancelled => {}
                        }

                        // Discard any hotkey hits that accumulated while the window
                        // was open (e.g. the user pressed a combo to test it).
                        hotkey_mgr.drain_hits();

                        // Re-register with the current (possibly updated) config.
                        register_hotkeys(&mut hotkey_mgr, &config);
                    }
                    other => {
                        handle_tray_action(
                            other,
                            &mut config,
                            &mut displays,
                            &icon_path,
                            &mut app_tray,
                        );
                    }
                }
            }
        }

        // Drain hotkey hits
        for action in hotkey_mgr.drain_hits() {
            handle_hotkey_action(&action, &config, &mut displays);
        }

        // Periodic DDC poll — update current_input for each monitor and
        // rebuild the tray if anything has changed since the last check.
        if last_poll.elapsed() >= POLL_INTERVAL {
            let mut changed = false;
            for d in &mut displays {
                if let Some(actual) = read_input(d) {
                    if d.current_input != Some(actual) {
                        info!("Input changed on {}: was {:?}, now 0x{actual:02X}", d.backend_id, d.current_input);
                        d.current_input = Some(actual);
                        changed = true;
                    }
                }
            }
            if changed {
                rebuild_tray(&mut app_tray, &config, &displays, &icon_path);
            }
            last_poll = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

fn handle_tray_action(
    action: TrayAction,
    config: &mut AppConfig,
    displays: &mut Vec<DdcDisplay>,
    icon_path: &PathBuf,
    app_tray: &mut AppTray,
) {
    match action {
        TrayAction::SwitchInput { monitor_idx, input_code } => {
            let switched = if let Some(d) = displays.get_mut(monitor_idx) {
                match set_input(&mut d.display, input_code) {
                    Ok(()) => { d.current_input = Some(input_code); true }
                    Err(e) => { error!("set_input failed: {e}"); false }
                }
            } else { false };
            if switched {
                rebuild_tray(app_tray, config, displays, icon_path);
            }
        }
        TrayAction::SetAllDefault => {
            set_all_default(config, displays);
            rebuild_tray(app_tray, config, displays, icon_path);
        }
        // Exit and OpenSettings are handled in the main loop
        TrayAction::Exit | TrayAction::OpenSettings => {}
    }
}

fn handle_hotkey_action(action: &str, config: &AppConfig, displays: &mut Vec<DdcDisplay>) {
    if action == "all_default" {
        set_all_default(config, displays);
        return;
    }
    if let Some(rest) = action.strip_prefix("direct_") {
        if let Ok(idx) = rest.parse::<usize>() {
            if let Some(dh) = config.direct_hotkeys.get(idx) {
                if let Some(d) = displays.iter_mut().find(|d| d.backend_id == dh.monitor_id) {
                    match set_input(&mut d.display, dh.input) {
                        Ok(()) => { d.current_input = Some(dh.input); info!("Direct hotkey: {} → 0x{:02X}", dh.monitor_id, dh.input); }
                        Err(e) => error!("direct hotkey set_input: {e}"),
                    }
                }
            }
        }
        return;
    }
    if let Some(rest) = action.strip_prefix("cycle_") {
        if let Ok(idx) = rest.parse::<usize>() {
            if let Some(ch) = config.cycle_hotkeys.get(idx) {
                if let Some(d) = displays.iter_mut().find(|d| d.backend_id == ch.monitor_id) {
                    let next = match d.current_input {
                        Some(c) if c == ch.input_a => ch.input_b,
                        Some(c) if c == ch.input_b => ch.input_a,
                        // Not on either cycle input — go to configured default, or input_a
                        _ => config.monitors.get(&ch.monitor_id)
                                .and_then(|m| m.default_input)
                                .unwrap_or(ch.input_a),
                    };
                    match set_input(&mut d.display, next) {
                        Ok(()) => { d.current_input = Some(next); info!("Cycle {}: → 0x{next:02X}", ch.monitor_id); }
                        Err(e) => error!("cycle set_input: {e}"),
                    }
                }
            }
        }
        return;
    }
    if let Some(rest) = action.strip_prefix("mon") {
        let parts: Vec<&str> = rest.splitn(2, '_').collect();
        if parts.len() == 2 {
            if let (Ok(mon_n), Ok(code)) = (parts[0].parse::<usize>(), parts[1].parse::<u8>()) {
                let idx = mon_n.saturating_sub(1);
                if let Some(d) = displays.get_mut(idx) {
                    match set_input(&mut d.display, code) {
                        Ok(()) => { d.current_input = Some(code); info!("Hotkey: monitor {idx} → 0x{code:02X}"); }
                        Err(e) => error!("hotkey set_input: {e}"),
                    }
                }
            }
        }
    }
}

fn set_all_default(config: &AppConfig, displays: &mut Vec<DdcDisplay>) {
    for d in displays.iter_mut() {
        if let Some(mc) = config.monitors.get(&d.backend_id) {
            if let Some(input) = mc.default_input {
                match set_input(&mut d.display, input) {
                    Ok(()) => { d.current_input = Some(input); }
                    Err(e) => error!("set_all_default {}: {e}", d.backend_id),
                }
            }
        }
    }
}

fn rebuild_tray(
    app_tray: &mut AppTray,
    config: &AppConfig,
    displays: &[DdcDisplay],
    icon_path: &PathBuf,
) {
    match AppTray::build(config, displays, icon_path) {
        Ok(t) => *app_tray = t,
        Err(e) => error!("Failed to rebuild tray: {e}"),
    }
}

fn register_hotkeys(mgr: &mut HotkeyManager, config: &AppConfig) {
    for (action, combo) in &config.hotkeys.bindings {
        if !combo.is_empty() {
            if let Err(e) = mgr.register(combo, action) {
                warn!("Could not register hotkey '{combo}' for '{action}': {e}");
            }
        }
    }
    for (idx, ch) in config.cycle_hotkeys.iter().enumerate() {
        if !ch.hotkey.is_empty() {
            let action = format!("cycle_{idx}");
            if let Err(e) = mgr.register(&ch.hotkey, &action) {
                warn!("Could not register cycle hotkey '{}' for '{action}': {e}", ch.hotkey);
            }
        }
    }
    for (idx, dh) in config.direct_hotkeys.iter().enumerate() {
        if !dh.hotkey.is_empty() {
            let action = format!("direct_{idx}");
            if let Err(e) = mgr.register(&dh.hotkey, &action) {
                warn!("Could not register direct hotkey '{}' for '{action}': {e}", dh.hotkey);
            }
        }
    }
}

fn icon_path() -> PathBuf {
    let mut p = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    p.pop();
    p.push("assets");
    p.push("icon.ico");
    p
}

/// Opt the process into dark mode so Win32 popup menus follow the system theme.
///
/// `SetPreferredAppMode` (uxtheme.dll ordinal 135) is undocumented but stable
/// since Windows 10 1809.  Value 1 = AllowDark: menus render dark when the
/// system is in dark mode, light otherwise.  Without this call Win32 menus are
/// always light regardless of system settings.
unsafe fn apply_dark_mode() {
    type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;

    let Ok(hmod) = (unsafe { LoadLibraryA(windows::core::s!("uxtheme.dll")) }) else {
        return;
    };
    // Ordinal 135 = SetPreferredAppMode — use MAKEINTRESOURCEA-style cast
    let ordinal = windows::core::PCSTR(135usize as *const u8);
    if let Some(f) = unsafe { GetProcAddress(hmod, ordinal) } {
        let set_mode: SetPreferredAppMode = unsafe { std::mem::transmute(f) };
        unsafe { set_mode(1) }; // 1 = AllowDark
    }
}
