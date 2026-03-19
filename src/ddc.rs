use anyhow::{anyhow, Result};
use ddc::Ddc as RawDdc; // supertrait on ddc_hi::Ddc — gives us capabilities_string()
use ddc_hi::Display;
use log::{debug, info, warn};

/// VCP feature code for Input Source Select
pub const VCP_INPUT: u8 = 0x60;

pub struct DdcDisplay {
    #[allow(dead_code)]
    pub index: usize,
    /// Stable string key used in config (from ddc_hi::DisplayInfo::id)
    pub backend_id: String,
    /// Human-readable model name if the capabilities string reported one
    pub model_name: Option<String>,
    pub current_input: Option<u8>,
    /// Inputs actually listed in the monitor's DDC capabilities string.
    /// Empty if capabilities were unavailable — callers should fall back.
    pub available_inputs: Vec<(String, u8)>,
    pub display: Display,
}

pub fn enumerate_ddc_displays() -> Vec<DdcDisplay> {
    let displays = Display::enumerate();
    let mut result = Vec::new();

    for (i, mut display) in displays.into_iter().enumerate() {
        let backend_id = display.info.id.clone();

        // Skip internal/panel displays that don't support DDC input switching.
        // LVDS and eDP are laptop/embedded panels; they never expose VCP 0x60.
        if is_internal_display(&backend_id) {
            info!("Display {i}: skipping internal panel ({backend_id})");
            continue;
        }

        let current_input = match display.handle.get_vcp_feature(VCP_INPUT) {
            Ok(val) => {
                debug!("Display {i} current input VCP: {}", val.value());
                Some(val.value() as u8)
            }
            Err(e) => {
                warn!("Display {i}: cannot read VCP 0x60: {e}");
                None
            }
        };

        // Read and parse the capabilities string for this monitor
        let (available_inputs, model_name) = read_capabilities(&mut display, i);

        result.push(DdcDisplay {
            index: i,
            backend_id,
            model_name,
            current_input,
            available_inputs,
            display,
        });
    }

    result
}

/// Returns true for internal panel displays that don't support DDC/CI input switching.
fn is_internal_display(backend_id: &str) -> bool {
    let lower = backend_id.to_ascii_lowercase();
    lower.contains(":lvds") || lower.contains(":edp") || lower.contains(":lvds0")
}

/// Read the raw DDC capabilities string and extract input list + model name.
fn read_capabilities(display: &mut Display, idx: usize) -> (Vec<(String, u8)>, Option<String>) {
    let raw = match display.handle.capabilities_string() {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!("Display {idx}: capabilities_string() failed: {e}");
            return (vec![], None);
        }
    };

    let caps_str = match std::str::from_utf8(&raw) {
        Ok(s) => s,
        Err(_) => {
            warn!("Display {idx}: capabilities string is not valid UTF-8");
            return (vec![], None);
        }
    };

    debug!("Display {idx} capabilities: {caps_str}");

    let model = parse_model(caps_str);
    let inputs = parse_input_codes(caps_str);

    if inputs.is_empty() {
        info!("Display {idx}: no VCP 0x60 inputs in capabilities; will use fallback");
    } else {
        let labels: Vec<&str> = inputs.iter().map(|(l, _)| l.as_str()).collect();
        info!("Display {idx}: inputs from capabilities: {labels:?}");
    }

    (inputs, model)
}

/// Extract `model(NAME)` from a capabilities string.
fn parse_model(caps: &str) -> Option<String> {
    let lower = caps.to_ascii_lowercase();
    let start = lower.find("model(")? + 6;
    let end = caps[start..].find(')')?;
    let name = caps[start..start + end].trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// Parse the `vcp(...)` section and extract the allowed values for VCP 0x60.
///
/// Example capabilities fragment: `vcp(02 60(11 12 0F) AC)`
/// Returns a sorted Vec of (label, code) pairs.
fn parse_input_codes(caps: &str) -> Vec<(String, u8)> {
    // Find the vcp(...) section (case-insensitive)
    let lower = caps.to_ascii_lowercase();
    let vcp_pos = match lower.find("vcp(") {
        Some(p) => p + 4,
        None => return vec![],
    };

    // Extract everything inside the outer vcp() parens
    let vcp_section = extract_paren_block(&caps[vcp_pos..]);

    // Tokenise: walk through hex tokens and their optional value lists
    let mut codes = parse_vcp_inputs(&vcp_section);
    codes.sort_by_key(|(_, c)| *c);
    codes
}

/// Walk the vcp() content and return the allowed values for feature 0x60.
/// Grammar: HHHH | HHHH(HHHH HHHH ...) separated by whitespace
fn parse_vcp_inputs(vcp_section: &str) -> Vec<(String, u8)> {
    let mut iter = vcp_section.chars().peekable();

    loop {
        // skip whitespace
        while iter.peek().map(|c: &char| c.is_whitespace()).unwrap_or(false) {
            iter.next();
        }

        // read a hex token
        let mut token = String::new();
        while iter.peek().map(|c: &char| c.is_ascii_hexdigit()).unwrap_or(false) {
            token.push(iter.next().unwrap());
        }

        if token.is_empty() {
            // Could be '(' or ')' from nested structure — skip one char and continue
            match iter.next() {
                None => break,
                Some(')') => break,
                _ => continue,
            }
        }

        let code = match u8::from_str_radix(&token, 16) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Check for optional value list "(v1 v2 ...)"
        if iter.peek() == Some(&'(') {
            iter.next(); // consume '('
            let values_str: String = iter.by_ref().take_while(|c| *c != ')').collect();

            if code == VCP_INPUT {
                // These are the actual input codes this monitor supports
                return values_str
                    .split_whitespace()
                    .filter_map(|s| u8::from_str_radix(s, 16).ok())
                    .map(|c| (input_label(c), c))
                    .collect();
            }
        } else if code == VCP_INPUT {
            // 0x60 is listed but without specific values — monitor supports input
            // switching but didn't enumerate which inputs; return empty so caller falls back
            return vec![];
        }
    }

    vec![]
}

/// Extract the content inside the first `(...)` block (excluding the parens themselves).
/// Handles nesting correctly.
fn extract_paren_block(s: &str) -> String {
    let mut result = String::new();
    let mut depth = 1usize;
    for c in s.chars() {
        match c {
            '(' => { depth += 1; result.push(c); }
            ')' => {
                depth -= 1;
                if depth == 0 { break; }
                result.push(c);
            }
            _ => result.push(c),
        }
    }
    result
}

/// Read the current active input from a display via DDC/CI.
/// Returns None if the monitor doesn't respond (e.g. powered off or busy).
pub fn read_input(d: &mut DdcDisplay) -> Option<u8> {
    d.display.handle.get_vcp_feature(VCP_INPUT)
        .ok()
        .map(|v| v.value() as u8)
}

pub fn set_input(display: &mut Display, input_code: u8) -> Result<()> {
    display
        .handle
        .set_vcp_feature(VCP_INPUT, input_code as u16)
        .map_err(|e| anyhow!("DDC set_vcp_feature failed: {e}"))
}

/// Common VCP 0x60 input codes (MCCS standard) — used as a label lookup table
pub fn standard_inputs() -> Vec<(&'static str, u8)> {
    vec![
        // ── MCCS standard codes ─────────────────────────────────────────
        ("VGA-1",       0x01),
        ("VGA-2",       0x02),
        ("DVI-1",       0x03),
        ("DVI-2",       0x04),
        ("Composite-1", 0x05),
        ("Composite-2", 0x06),
        ("S-Video-1",   0x07),
        ("S-Video-2",   0x08),
        ("Tuner-1",     0x09),
        ("Tuner-2",     0x0A),
        ("Tuner-3",     0x0B),
        ("Component-1", 0x0C),
        ("Component-2", 0x0D),
        ("Component-3", 0x0E),
        ("DP-1",        0x0F),
        ("DP-2",        0x10),
        ("HDMI-1",      0x11),
        ("HDMI-2",      0x12),
        ("HDMI-3",      0x13),
        ("HDMI-4",      0x14),
        // ── MCCS reserved range — widely used by manufacturers ──────────
        // HDMI-5 through HDMI-9: monitors with many HDMI ports (TVs, hubs)
        ("HDMI-5",      0x15),
        ("HDMI-6",      0x16),
        ("HDMI-7",      0x17),
        ("HDMI-8",      0x18),
        ("HDMI-9",      0x19),
        // DP-3: used by Dell UltraSharp and HP Z-series with three DP ports
        ("DP-3",        0x1A),
        // USB-C: Dell (S/U/P-series), HP, Lenovo — the most common code for
        // the first USB-C / USB Type-C input.  0x1B confirmed by user on Dell S2725QC.
        ("USB-C",       0x1B),
        // USB-C-2: secondary USB-C port on monitors with two (e.g. some Dell U-series)
        ("USB-C-2",     0x1C),
        // Thunderbolt: Intel-spec Thunderbolt 3/4 input, used by some HP and Dell monitors
        ("Thunderbolt", 0x1D),
        // ── Vendor-specific ranges seen in the wild ──────────────────────
        // 0x20–0xFF: no reliable cross-vendor mapping; falls through to "Input 0xXX"
    ]
}

/// Human-readable label for a VCP input code
pub fn input_label(code: u8) -> String {
    for (label, c) in standard_inputs() {
        if c == code {
            return label.to_string();
        }
    }
    format!("Input 0x{:02X}", code)
}
