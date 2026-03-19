//! Hotkey detection via the Interception kernel-mode filter driver.
//!
//! Interception intercepts keystrokes below all user-mode hooks and
//! `BlockInput()` calls, so hotkeys fire even when Synergy (or a similar
//! software KVM) has captured the keyboard.
//!
//! **One-time driver install (requires Administrator + reboot):**
//!   Download the installer from <https://github.com/oblitum/Interception>
//!   and run it.  `interception.dll` is placed in System32 by the installer.
//!
//! If the driver is absent, `HotkeyManager::new()` returns an `Err` and the
//! app exits with a clear message.

use std::collections::HashSet;
use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::{anyhow, Result};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, MAPVK_VSC_TO_VK, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL,
    VK_RMENU, VK_RSHIFT, VK_RWIN,
};
use windows::core::PCSTR;

// ---------------------------------------------------------------------------
// Parsed combo + registration
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
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

// ---------------------------------------------------------------------------
// Interception driver FFI (loaded dynamically so we can give a clear error
// when the driver is not installed, rather than a cryptic linker error)
// ---------------------------------------------------------------------------

type InterceptionContext = *mut std::ffi::c_void;
type InterceptionDevice  = i32;
type InterceptionFilter  = u16;

// Key state flags in InterceptionKeyStroke::state
const INTERCEPTION_KEY_UP: u16 = 0x01;
const INTERCEPTION_KEY_E0: u16 = 0x02;

// Capture all key events (down + up, regular + extended)
const INTERCEPTION_FILTER_KEY_ALL: u16 = 0xFFFF;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct InterceptionKeyStroke {
    code:        u16, // hardware scan code (no E0/E1 prefix — use state flags)
    state:       u16, // INTERCEPTION_KEY_* flags
    information: u32,
}

// Interception device numbers: keyboards are 1–10, mice 11–20, 0 = timeout.
const INTERCEPTION_MAX_DEVICE: i32 = 20;

type FnCreateContext  = unsafe extern "C" fn() -> InterceptionContext;
type FnDestroyContext = unsafe extern "C" fn(InterceptionContext);
type FnIsKeyboard     = unsafe extern "C" fn(InterceptionDevice) -> i32;
type FnSetFilter      = unsafe extern "C" fn(InterceptionContext, FnIsKeyboard, InterceptionFilter);
type FnWaitTimeout    = unsafe extern "C" fn(InterceptionContext, u32) -> InterceptionDevice;
type FnReceive        = unsafe extern "C" fn(InterceptionContext, InterceptionDevice, *mut std::ffi::c_void, u32) -> i32;
type FnSend           = unsafe extern "C" fn(InterceptionContext, InterceptionDevice, *const std::ffi::c_void, u32) -> i32;

struct InterceptionApi {
    #[allow(dead_code)]
    hmod:            HMODULE, // keeps the DLL pinned for the process lifetime
    create_context:  FnCreateContext,
    destroy_context: FnDestroyContext,
    is_keyboard:     FnIsKeyboard,
    set_filter:      FnSetFilter,
    wait_timeout:    FnWaitTimeout,
    receive:         FnReceive,
    send:            FnSend,
}

// All fields are either HMODULE (Send) or function pointers (Send).
unsafe impl Send for InterceptionApi {}

impl InterceptionApi {
    fn load() -> Result<Self> {
        let hmod = unsafe { LoadLibraryA(windows::core::s!("interception.dll")) }
            .map_err(|_| anyhow!(
                "interception.dll not found. \
                 Install the Interception driver from \
                 https://github.com/oblitum/Interception and reboot."
            ))?;

        macro_rules! get_proc {
            ($name:literal, $ty:ty) => {{
                // concat! with "\0" gives us a null-terminated byte literal at
                // compile time — exactly what PCSTR / GetProcAddress needs.
                const SYM: &[u8] = concat!($name, "\0").as_bytes();
                let proc = unsafe { GetProcAddress(hmod, PCSTR(SYM.as_ptr())) }
                    .ok_or_else(|| anyhow!("interception.dll missing export '{}'", $name))?;
                // On Windows x64 all calling conventions are identical, so
                // transmuting extern "system" fn() to extern "C" fn(...) is safe.
                unsafe { std::mem::transmute::<_, $ty>(proc) }
            }};
        }

        Ok(InterceptionApi {
            hmod,
            create_context:  get_proc!("interception_create_context",    FnCreateContext),
            destroy_context: get_proc!("interception_destroy_context",   FnDestroyContext),
            is_keyboard:     get_proc!("interception_is_keyboard",       FnIsKeyboard),
            set_filter:      get_proc!("interception_set_filter",        FnSetFilter),
            wait_timeout:    get_proc!("interception_wait_with_timeout", FnWaitTimeout),
            receive:         get_proc!("interception_receive",           FnReceive),
            send:            get_proc!("interception_send",              FnSend),
        })
    }
}

// ---------------------------------------------------------------------------
// Public API — identical interface to the previous GetAsyncKeyState version
// ---------------------------------------------------------------------------

pub struct HotkeyManager {
    registrations: Arc<RwLock<Vec<Registration>>>,
    rx:            mpsc::Receiver<String>,
    stop:          Arc<AtomicBool>,
    _thread:       std::thread::JoinHandle<()>,
}

impl HotkeyManager {
    pub fn new() -> Result<Self> {
        let api = InterceptionApi::load()?;

        let registrations = Arc::new(RwLock::new(Vec::<Registration>::new()));
        let (tx, rx)      = mpsc::channel::<String>();
        let stop          = Arc::new(AtomicBool::new(false));

        let regs2  = Arc::clone(&registrations);
        let stop2  = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("interception-hotkeys".into())
            .spawn(move || interception_loop(api, regs2, tx, stop2))
            .map_err(|e| anyhow!("failed to spawn interception thread: {e}"))?;

        Ok(HotkeyManager { registrations, rx, stop, _thread: thread })
    }

    pub fn register(&mut self, combo: &str, action: &str) -> Result<()> {
        let parsed = parse_combo(combo)?;
        self.registrations.write().unwrap().push(Registration {
            combo:  parsed,
            action: action.to_string(),
        });
        Ok(())
    }

    pub fn unregister_all(&mut self) {
        self.registrations.write().unwrap().clear();
    }

    /// Drain actions fired since the last call.  Non-blocking.
    pub fn drain_hits(&mut self) -> Vec<String> {
        let mut hits = Vec::new();
        while let Ok(action) = self.rx.try_recv() {
            hits.push(action);
        }
        hits
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Thread exits within one wait_timeout cycle (≤ 50 ms).
    }
}

// ---------------------------------------------------------------------------
// Background thread — Interception receive loop
// ---------------------------------------------------------------------------

fn interception_loop(
    api:           InterceptionApi,
    registrations: Arc<RwLock<Vec<Registration>>>,
    tx:            mpsc::Sender<String>,
    stop:          Arc<AtomicBool>,
) {
    let ctx = unsafe { (api.create_context)() };
    if ctx.is_null() {
        log::error!("interception_create_context returned null — driver not running?");
        return;
    }

    // Capture every key event on every keyboard device.
    unsafe { (api.set_filter)(ctx, api.is_keyboard, INTERCEPTION_FILTER_KEY_ALL) };

    // Virtual-key codes currently held down.
    let mut down_vks: HashSet<u32>    = HashSet::new();
    // Actions whose full combo is currently held (rising-edge guard).
    let mut active_actions: HashSet<String> = HashSet::new();

    while !stop.load(Ordering::Relaxed) {
        // Block up to 50 ms, then loop to re-check the stop flag.
        let device = unsafe { (api.wait_timeout)(ctx, 50) };
        if device <= 0 || device > INTERCEPTION_MAX_DEVICE {
            continue; // timeout or invalid
        }

        let mut ks = InterceptionKeyStroke::default();
        let received = unsafe {
            (api.receive)(ctx, device, &mut ks as *mut _ as *mut _, 1)
        };

        // Always forward the stroke — we observe only, never consume.
        if received > 0 {
            unsafe { (api.send)(ctx, device, &ks as *const _ as *const _, 1) };
        } else {
            continue;
        }

        let is_up = (ks.state & INTERCEPTION_KEY_UP) != 0;
        let is_e0 = (ks.state & INTERCEPTION_KEY_E0) != 0;
        let vk    = scancode_to_vk(ks.code, is_e0);
        if vk == 0 {
            continue;
        }

        if is_up {
            down_vks.remove(&vk);
        } else {
            down_vks.insert(vk);
        }

        // Compute which registered combos are fully satisfied right now.
        let now_active: HashSet<String> = {
            let regs = registrations.read().unwrap();
            regs.iter()
                .filter(|r| combo_down(&r.combo, &down_vks))
                .map(|r| r.action.clone())
                .collect()
        };

        // Fire actions that just became active (rising edge, key-down only).
        if !is_up {
            for action in now_active.difference(&active_actions) {
                let _ = tx.send(action.clone());
            }
        }

        active_actions = now_active;
    }

    unsafe { (api.destroy_context)(ctx) };
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns true when every key required by `combo` appears in `down_vks`.
fn combo_down(combo: &ParsedCombo, down_vks: &HashSet<u32>) -> bool {
    let ctrl  = down_vks.contains(&(VK_LCONTROL.0 as u32))
             || down_vks.contains(&(VK_RCONTROL.0 as u32));
    let alt   = down_vks.contains(&(VK_LMENU.0 as u32))
             || down_vks.contains(&(VK_RMENU.0 as u32));
    let shift = down_vks.contains(&(VK_LSHIFT.0 as u32))
             || down_vks.contains(&(VK_RSHIFT.0 as u32));
    let win   = down_vks.contains(&(VK_LWIN.0 as u32))
             || down_vks.contains(&(VK_RWIN.0 as u32));

    down_vks.contains(&combo.vk)
        && (!combo.ctrl  || ctrl)
        && (!combo.alt   || alt)
        && (!combo.shift || shift)
        && (!combo.win   || win)
}

/// Convert an Interception scan code + E0 flag to a Windows virtual-key code.
///
/// Modifier keys share scan codes between their left/right variants, with the
/// E0 flag distinguishing them — we handle those explicitly.  All other keys
/// (letters, digits, F-keys) are mapped via `MapVirtualKeyW`.
fn scancode_to_vk(code: u16, is_e0: bool) -> u32 {
    match (code, is_e0) {
        (0x1D, false) => VK_LCONTROL.0 as u32, // Left Ctrl
        (0x1D, true)  => VK_RCONTROL.0 as u32, // Right Ctrl
        (0x38, false) => VK_LMENU.0 as u32,    // Left Alt
        (0x38, true)  => VK_RMENU.0 as u32,    // Right Alt
        (0x2A, _)     => VK_LSHIFT.0 as u32,   // Left Shift
        (0x36, _)     => VK_RSHIFT.0 as u32,   // Right Shift
        (0x5B, _)     => VK_LWIN.0 as u32,     // Left Win
        (0x5C, _)     => VK_RWIN.0 as u32,     // Right Win
        (c, _)        => unsafe { MapVirtualKeyW(c as u32, MAPVK_VSC_TO_VK) },
    }
}

// ---------------------------------------------------------------------------
// Combo string parsing — unchanged from previous version
// ---------------------------------------------------------------------------

/// Validate a hotkey string; used by the settings UI for inline error display.
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
