use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use anyhow::Result;
use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};

/// High-level description of a connected monitor
#[derive(Debug, Clone)]
pub struct MonitorInfo {
    /// Windows device name, e.g. `\\.\DISPLAY1`
    pub device_name: String,
    /// Unique device-path id (used as config key)
    pub device_id: String,
    /// Bounding rect in virtual screen coords
    pub rect: (i32, i32, i32, i32),
    /// Is this the primary monitor?
    pub is_primary: bool,
}

pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>> {
    let mut monitors: Vec<MonitorInfo> = Vec::new();
    let ptr = &mut monitors as *mut Vec<MonitorInfo> as isize;

    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(enum_monitors_callback),
            LPARAM(ptr),
        );
    }

    Ok(monitors)
}

unsafe extern "system" fn enum_monitors_callback(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let monitors = &mut *(lparam.0 as *mut Vec<MonitorInfo>);

    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;

    if GetMonitorInfoW(hmonitor, &mut info.monitorInfo).as_bool() {
        let name_slice = &info.szDevice;
        let name_len = name_slice.iter().position(|&c| c == 0).unwrap_or(name_slice.len());
        let device_name = OsString::from_wide(&name_slice[..name_len])
            .to_string_lossy()
            .to_string();

        let r = info.monitorInfo.rcMonitor;
        let is_primary = (info.monitorInfo.dwFlags & 1) != 0; // MONITORINFOF_PRIMARY = 1

        // Use device_name as device_id (stable enough for config keying)
        let device_id = device_name.trim_start_matches("\\\\.\\").to_string();

        monitors.push(MonitorInfo {
            device_name: device_name.clone(),
            device_id,
            rect: (r.left, r.top, r.right, r.bottom),
            is_primary,
        });
    }

    BOOL(1) // continue enumeration
}
