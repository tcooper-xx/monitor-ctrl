use std::collections::{HashSet, HashMap};
use std::sync::mpsc;
use std::sync::atomic::{AtomicIsize, AtomicBool, Ordering};
use std::sync::{Arc, RwLock, Mutex};
use once_cell::sync::Lazy;

use anyhow::{anyhow, Result};
use windows::Win32::Foundation::{HMODULE, LPARAM, LRESULT, WPARAM, HWND};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

// ---------------------------------------------------------------------------
// Parsed combo + registration
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
struct ParsedCombo {
    vk: u32,
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
}

#[derive(Clone, Debug)]
struct Registration {
    combo: ParsedCombo,
    action: String,
}

static REGISTRATIONS: Lazy<RwLock<Vec<Registration>>> = Lazy::new(|| RwLock::new(Vec::new()));
static GLOBAL_TX: Lazy<Mutex<Option<mpsc::Sender<String>>>> = Lazy::new(|| Mutex::new(None));
static HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);

// Modifier tracking manually to fix Synergy drops
static CTRL_PRESSED: AtomicBool = AtomicBool::new(false);
static SHIFT_PRESSED: AtomicBool = AtomicBool::new(false);
static ALT_PRESSED: AtomicBool = AtomicBool::new(false);
static WIN_PRESSED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let msg = wparam.0 as u32;
        let kb_struct = *(lparam.0 as *const KBDLLHOOKSTRUCT);
        let vk_code = kb_struct.vkCode;

        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            match vk_code {
                160 | 161 | 16 => SHIFT_PRESSED.store(true, Ordering::SeqCst),
                162 | 163 | 17 => CTRL_PRESSED.store(true, Ordering::SeqCst),
                164 | 165 | 18 => ALT_PRESSED.store(true, Ordering::SeqCst),
                91 | 92 | 21 => WIN_PRESSED.store(true, Ordering::SeqCst), // LWIN, RWIN
                _ => {}
            }
        } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
            match vk_code {
                160 | 161 | 16 => SHIFT_PRESSED.store(false, Ordering::SeqCst),
                162 | 163 | 17 => CTRL_PRESSED.store(false, Ordering::SeqCst),
                164 | 165 | 18 => ALT_PRESSED.store(false, Ordering::SeqCst),
                91 | 92 | 21 => WIN_PRESSED.store(false, Ordering::SeqCst),
                _ => {}
            }
        }

        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            let ctrl = CTRL_PRESSED.load(Ordering::SeqCst);
            let shift = SHIFT_PRESSED.load(Ordering::SeqCst);
            let alt = ALT_PRESSED.load(Ordering::SeqCst);
            let win = WIN_PRESSED.load(Ordering::SeqCst);

            let mut matched_action = None;
            if let Ok(regs) = REGISTRATIONS.read() {
                for r in regs.iter() {
                    if r.combo.vk == vk_code && r.combo.ctrl == ctrl && r.combo.shift == shift && r.combo.alt == alt && r.combo.win == win {
                        matched_action = Some(r.action.clone());
                        break;
                    }
                }
            }

            if let Some(action) = matched_action {
                if let Ok(mut g_tx) = GLOBAL_TX.lock() {
                    if let Some(tx) = g_tx.as_mut() {
                        let _ = tx.send(action);
                    }
                }
                
                CTRL_PRESSED.store(false, Ordering::SeqCst);
                SHIFT_PRESSED.store(false, Ordering::SeqCst);
                ALT_PRESSED.store(false, Ordering::SeqCst);
                WIN_PRESSED.store(false, Ordering::SeqCst);

                return LRESULT(1);
            }
        }
    }

    CallNextHookEx(HHOOK(HOOK_HANDLE.load(Ordering::SeqCst) as *mut std::ffi::c_void), code, wparam, lparam)
}

fn install_hook() {
    unsafe {
        let hook = SetWindowsHookExW( WH_KEYBOARD_LL, Some(keyboard_proc), HMODULE::default(), 0 );
        if let Ok(h) = hook {
            HOOK_HANDLE.store(h.0 as isize, Ordering::SeqCst);
        } else {
            return;
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).into() {
            // Processing loop
        }

        let handle = HOOK_HANDLE.load(Ordering::SeqCst);
        if handle != 0 {
            let _ = UnhookWindowsHookEx(HHOOK(handle as *mut std::ffi::c_void));
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub struct HotkeyManager {
    rx: mpsc::Receiver<String>,
    _thread: Option<std::thread::JoinHandle<()>>,
}

impl HotkeyManager {
    pub fn new() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<String>();
        {
            let mut g_tx = GLOBAL_TX.lock().unwrap();
            *g_tx = Some(tx);
        }

        let thread = std::thread::Builder::new()
            .name("keyboard-hook".into())
            .spawn(move || install_hook())
            .map_err(|e| anyhow!("failed to spawn hook thread: {e}"))?;

        Ok(HotkeyManager { rx, _thread: Some(thread) })
    }

    pub fn register(&mut self, combo: &str, action: &str) -> Result<()> {
        let parsed = parse_combo(combo)?;
        REGISTRATIONS.write().unwrap().push(Registration {
            combo: parsed,
            action: action.to_string(),
        });
        Ok(())
    }

    pub fn unregister_all(&mut self) {
        REGISTRATIONS.write().unwrap().clear();
    }

    pub fn drain_hits(&mut self) -> Vec<String> {
        let mut hits = Vec::new();
        while let Ok(action) = self.rx.try_recv() {
            hits.push(action);
        }
        hits
    }
}

// ---------------------------------------------------------------------------
// Combo parsing
// ---------------------------------------------------------------------------

pub fn parse_hotkey(combo: &str) -> Result<()> {
    parse_combo(combo).map(|_| ())
}

fn parse_combo(combo: &str) -> Result<ParsedCombo> {
    let parts: Vec<&str> = combo.split('+').map(str::trim).collect();
    let mut ctrl  = false;
    let mut alt   = false;
    let mut shift = false;
    let mut win   = false;
    let mut vk: Option<u32> = None;

    for part in &parts {
        match *part {
            "Ctrl" | "Control" => ctrl  = true,
            "Alt"              => alt   = true,
            "Shift"            => shift = true,
            "Win" | "Super"    => win   = true,
            other              => vk = Some(parse_vk(other)?),
        }
    }

    let vk = vk.ok_or_else(|| anyhow!("no key code in hotkey: '{combo}'"))?;
    Ok(ParsedCombo { vk, ctrl, alt, shift, win })
}

fn parse_vk(s: &str) -> Result<u32> {
    let vk: u32 = match s.to_uppercase().as_str() {
        "0" => 0x30, "1" => 0x31, "2" => 0x32, "3" => 0x33, "4" => 0x34,
        "5" => 0x35, "6" => 0x36, "7" => 0x37, "8" => 0x38, "9" => 0x39,
        "A" => 0x41, "B" => 0x42, "C" => 0x43, "D" => 0x44, "E" => 0x45,
        "F" => 0x46, "G" => 0x47, "H" => 0x48, "I" => 0x49, "J" => 0x4A,
        "K" => 0x4B, "L" => 0x4C, "M" => 0x4D, "N" => 0x4E, "O" => 0x4F,
        "P" => 0x50, "Q" => 0x51, "R" => 0x52, "S" => 0x53, "T" => 0x54,
        "U" => 0x55, "V" => 0x56, "W" => 0x57, "X" => 0x58, "Y" => 0x59,
        "Z" => 0x5A,
        "F1"  => 0x70, "F2"  => 0x71, "F3"  => 0x72, "F4"  => 0x73,
        "F5"  => 0x74, "F6"  => 0x75, "F7"  => 0x76, "F8"  => 0x77,
        "F9"  => 0x78, "F10" => 0x79, "F11" => 0x7A, "F12" => 0x7B,
        other => return Err(anyhow!("unknown key: '{other}'")),
    };
    Ok(vk)
}
