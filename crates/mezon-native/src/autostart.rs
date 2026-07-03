/// Auto-start on login using the `auto-launch` crate.
///
/// On Windows: writes to `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
/// On Linux: creates/removes `~/.config/autostart/mezon.desktop`.
use anyhow::Result;
use auto_launch::AutoLaunchBuilder;

/// Sync the auto-start state with what is stored in Settings.
/// Call once at app startup after settings are loaded.
pub fn sync_auto_start(enabled: bool) {
    if let Err(e) = set_auto_start(enabled) {
        tracing::warn!("Failed to sync auto-start (enabled={enabled}): {e}");
    } else {
        tracing::debug!("Auto-start synced: enabled={enabled}");
    }
}

/// Enable or disable the login item.
pub fn set_auto_start(enabled: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    let exe_str = exe
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("executable path is not valid UTF-8"))?;

    #[cfg(target_os = "macos")]
    disable_legacy_login_item(exe_str);

    let mut builder = AutoLaunchBuilder::new();
    builder.set_app_name("Mezon").set_app_path(exe_str);
    #[cfg(target_os = "macos")]
    builder.set_use_launch_agent(true);
    let auto = builder.build()?;

    if enabled {
        auto.enable()?;
    } else {
        // Only disable if currently enabled to avoid spurious errors.
        if auto.is_enabled()? {
            auto.disable()?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn disable_legacy_login_item(exe_str: &str) {
    let mut builder = AutoLaunchBuilder::new();
    builder.set_app_name("Mezon").set_app_path(exe_str);
    builder.set_use_launch_agent(false);
    if let Ok(legacy) = builder.build() {
        let _ = legacy.disable();
    }
}
