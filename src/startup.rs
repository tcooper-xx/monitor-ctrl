use anyhow::Result;
use auto_launch::AutoLaunchBuilder;

fn make_auto_launch() -> Result<auto_launch::AutoLaunch> {
    let exe = std::env::current_exe()?;
    let app = AutoLaunchBuilder::new()
        .set_app_name("MonitorCtrl")
        .set_app_path(exe.to_str().unwrap_or("monitor-ctrl.exe"))
        .build()?;
    Ok(app)
}

pub fn is_enabled() -> bool {
    match make_auto_launch() {
        Ok(a) => a.is_enabled().unwrap_or(false),
        Err(_) => false,
    }
}

pub fn set_enabled(enable: bool) -> Result<()> {
    let app = make_auto_launch()?;
    if enable {
        app.enable()?;
    } else {
        app.disable()?;
    }
    Ok(())
}
