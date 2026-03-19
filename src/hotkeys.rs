use anyhow::{anyhow, Result};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

// ---------------------------------------------------------------------------
// Parsed combo
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct ParsedCombo {
    vk: u32,
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
}

#[derive(Clone)]
struct Registration {
    combo: ParsedCombo,
    action: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub struct HotkeyManager {
    registrations: Vec<Registration>,
    /// Tracks whether each combo was "fully active" on the previous poll.
    /// Used to detect rising edges and avoid repeated firing while held.
    prev_active: Vec<bool>,
}

impl HotkeyManager {
    pub fn new() -> Result<Self> {
        Ok(Self {
            registrations: Vec::new(),
            prev_active: Vec::new(),
        })
    }

    pub fn register(&mut self, combo: &str, action: &str) -> Result<()> {
        let parsed = parse_combo(combo)?;
        self.registrations.push(Registration { combo: parsed, action: action.to_string() });
        self.prev_active.push(false);
        Ok(())
    }

    pub fn unregister_all(&mut self) {
        self.registrations.clear();
        self.prev_active.clear();
    }

    /// Poll `GetAsyncKeyState` for every registered combo, detect rising edges,
    /// and return the action strings for combos that just became fully active.
    ///
    /// `GetAsyncKeyState` reads the kernel's async key-state table, which is
    /// written by the keyboard class driver *before* any user-mode hook or
    /// BlockInput call can suppress it.  This makes it visible even when
    /// Synergy (or a similar software KVM) has captured the keyboard.
    pub fn drain_hits(&mut self) -> Vec<String> {
        let mut fired = Vec::new();
        for (i, reg) in self.registrations.iter().enumerate() {
            let active = combo_active(&reg.combo);
            if active && !self.prev_active[i] {
                fired.push(reg.action.clone());
            }
            self.prev_active[i] = active;
        }
        fired
    }
}

fn combo_active(combo: &ParsedCombo) -> bool {
    (!combo.ctrl  || key_down(VK_CONTROL.0 as i32))
        && (!combo.alt   || key_down(VK_MENU.0 as i32))
        && (!combo.shift || key_down(VK_SHIFT.0 as i32))
        && (!combo.win   || key_down(VK_LWIN.0 as i32) || key_down(VK_RWIN.0 as i32))
        && key_down(combo.vk as i32)
}

#[inline]
fn key_down(vk: i32) -> bool {
    (unsafe { GetAsyncKeyState(vk) }) as u16 & 0x8000 != 0
}

// ---------------------------------------------------------------------------
// Combo string parsing
// ---------------------------------------------------------------------------

/// Validate a hotkey string — returns `Ok(())` if parseable.
/// Used by the settings UI for inline error display.
pub fn parse_hotkey(combo: &str) -> Result<()> {
    parse_combo(combo).map(|_| ())
}

fn parse_combo(combo: &str) -> Result<ParsedCombo> {
    let parts: Vec<&str> = combo.split('+').map(str::trim).collect();
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut win = false;
    let mut vk: Option<u32> = None;

    for part in &parts {
        match *part {
            "Ctrl" | "Control" => ctrl = true,
            "Alt"              => alt = true,
            "Shift"            => shift = true,
            "Win" | "Super"    => win = true,
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
