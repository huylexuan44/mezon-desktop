use anyhow::Result;
use std::sync::{Arc, Mutex};
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

const SHOW_ID: &str = "show";
const UPDATE_ID: &str = "update";
const QUIT_ID: &str = "quit";

#[cfg(target_os = "linux")]
const TRAY_ICON_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/icons/trayicon-linux.png"
));

#[cfg(not(target_os = "linux"))]
const TRAY_ICON_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/icons/trayicon.png"
));

pub struct MezonTray {
    #[cfg(not(target_os = "linux"))]
    _icon: TrayIcon,
    update_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    stop_tx: crossbeam_channel::Sender<()>,
}

impl MezonTray {
    pub fn new(
        on_show: impl Fn() + Send + Sync + 'static,
        on_quit: impl Fn() + Send + Sync + 'static,
        rt_handle: Arc<tokio::runtime::Handle>,
    ) -> Result<Self> {
        let on_show = Arc::new(on_show);
        let on_quit = Arc::new(on_quit);
        let receiver = MenuEvent::receiver().clone();

        let update_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>> =
            Arc::new(Mutex::new(None));
        let update_task_thread = Arc::clone(&update_task);

        let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(1);

        std::thread::spawn(move || {
            loop {
                crossbeam_channel::select! {
                    recv(receiver) -> msg => match msg {
                        Ok(event) => match event.id().0.as_str() {
                            SHOW_ID => (on_show)(),
                            UPDATE_ID => {
                                let handle = rt_handle.clone();
                                let join = handle.spawn(async {
                                    match mezon_updater::check_for_updates(env!("CARGO_PKG_VERSION"))
                                        .await
                                    {
                                        Ok(Some(version)) => {
                                            let download_url = "https://mezon.ai/download";
                                            tracing::info!(
                                                "Update available: v{version} — download from {download_url}"
                                            );
                                            match mezon_updater::validate_update_url(download_url) {
                                                Ok(()) => {
                                                    let _ = open::that_detached(download_url);
                                                }
                                                Err(e) => {
                                                    tracing::warn!("Blocked open of update URL: {e}");
                                                }
                                            }
                                        }
                                        Ok(None) => {
                                            tracing::info!("Mezon is up to date");
                                        }
                                        Err(e) => {
                                            tracing::warn!("Update check failed: {e}");
                                        }
                                    }
                                });
                                if let Ok(mut guard) = update_task_thread.lock()
                                    && let Some(prior) = guard.replace(join)
                                {
                                    prior.abort();
                                }
                            }
                            QUIT_ID => (on_quit)(),
                            _ => {}
                        },
                        Err(_) => break,
                    },
                    recv(stop_rx) -> _ => break,
                }
            }
            tracing::debug!("Tray event-loop thread exiting");
        });

        #[cfg(not(target_os = "linux"))]
        let icon = build_menu_and_tray()?;

        #[cfg(target_os = "linux")]
        {
            let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<(), String>>(1);
            std::thread::Builder::new()
                .name("mezon-tray-gtk".into())
                .spawn(move || {
                    if let Err(e) = gtk::init() {
                        let _ = ready_tx.send(Err(format!("gtk init failed: {e}")));
                        return;
                    }
                    let _tray = match build_menu_and_tray() {
                        Ok(tray) => tray,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.to_string()));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));
                    gtk::main();
                })
                .map_err(|e| anyhow::anyhow!("Failed to spawn GTK tray thread: {e}"))?;

            match ready_rx.recv() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(anyhow::anyhow!(e)),
                Err(e) => return Err(anyhow::anyhow!("GTK tray thread exited before init: {e}")),
            }
        }

        tracing::debug!("System tray created");
        Ok(Self {
            #[cfg(not(target_os = "linux"))]
            _icon: icon,
            update_task,
            stop_tx,
        })
    }
}

fn build_menu_and_tray() -> Result<TrayIcon> {
    let menu = Menu::new();

    let item_show = MenuItem::with_id(SHOW_ID, "Show Mezon", true, None);
    let item_update = MenuItem::with_id(UPDATE_ID, "Check for Updates", true, None);
    let item_quit = MenuItem::with_id(QUIT_ID, "Quit Mezon", true, None);

    menu.append(&item_show)?;
    menu.append(&item_update)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&item_quit)?;

    let icon = build_tray_icon();

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Mezon")
        .with_icon(icon)
        .build()?;

    Ok(tray)
}

impl Drop for MezonTray {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
        if let Ok(mut guard) = self.update_task.lock()
            && let Some(handle) = guard.take()
        {
            handle.abort();
            tracing::debug!("Aborted tray update-check task on drop");
        }
    }
}

fn load_icon_from_bytes(data: &[u8]) -> Result<tray_icon::Icon> {
    let img = image::load_from_memory(data)?.into_rgba8();
    let (w, h) = img.dimensions();
    let rgba = img.into_raw();
    Ok(tray_icon::Icon::from_rgba(rgba, w, h)?)
}

fn build_tray_icon() -> tray_icon::Icon {
    for path in &icon_search_paths() {
        if path.exists() {
            match load_icon_from_path(path) {
                Ok(icon) => {
                    tracing::debug!("Loaded tray icon from {}", path.display());
                    return icon;
                }
                Err(e) => {
                    tracing::warn!("Failed to load tray icon from {}: {e}", path.display());
                }
            }
        }
    }
    match load_icon_from_bytes(TRAY_ICON_BYTES) {
        Ok(icon) => {
            tracing::debug!("Loaded embedded tray icon");
            return icon;
        }
        Err(e) => tracing::warn!("Failed to load embedded tray icon: {e}"),
    }
    tracing::debug!("Using generated fallback tray icon");
    build_fallback_icon()
}

fn icon_search_paths() -> Vec<std::path::PathBuf> {
    let mut paths = vec![];
    if cfg!(target_os = "linux") {
        paths.push(std::path::PathBuf::from("assets/icons/trayicon-linux.png"));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        paths.push(parent.join("assets/icons/trayicon-linux.png"));
        paths.push(parent.join("assets/icons/trayicon.png"));
        paths.push(parent.join("../../../assets/icons/trayicon.png"));
        paths.push(parent.join("../../../../assets/icons/trayicon.png"));
    }
    paths.push(std::path::PathBuf::from("assets/icons/trayicon.png"));
    paths
}

fn load_icon_from_path(path: &std::path::Path) -> Result<tray_icon::Icon> {
    let img = image::open(path)?.into_rgba8();
    let (w, h) = img.dimensions();
    let rgba = img.into_raw();
    Ok(tray_icon::Icon::from_rgba(rgba, w, h)?)
}

fn build_fallback_icon() -> tray_icon::Icon {
    const SIZE: u32 = 22;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..SIZE * SIZE {
        rgba.extend_from_slice(&[0x58, 0x65, 0xF2, 0xFF]);
    }
    tray_icon::Icon::from_rgba(rgba, SIZE, SIZE).expect("Failed to build fallback tray icon")
}
