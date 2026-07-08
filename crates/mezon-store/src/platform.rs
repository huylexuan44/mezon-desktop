use gpui::{App, AppContext, Entity, Global, SharedString};
use std::sync::Arc;

pub fn download_url_with_dialog(url: SharedString, filename: SharedString, cx: &mut App) {
    let directory = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let suggested = filename.to_string();
    let receiver = cx.prompt_for_new_path(&directory, Some(suggested.as_str()));
    cx.spawn(async move |_cx| {
        if let Ok(Ok(Some(path))) = receiver.await
            && let Err(error) = mezon_client::transport_runtime::download_to(&url, path).await
        {
            tracing::error!("attachment download failed: {error}");
        }
    })
    .detach();
}

pub type OpenUrlFn = Arc<dyn Fn(&str) -> anyhow::Result<()> + Send + Sync>;
/// Download `url` and save it locally under the given suggested filename.
pub type SaveAttachmentFn = Arc<dyn Fn(&str, &str) -> anyhow::Result<()> + Send + Sync>;
pub type NotifyFn = Arc<dyn Fn(DesktopNotification) + Send + Sync>;

pub struct DesktopNotification {
    pub title: String,
    pub body: String,
    pub channel_id: Option<String>,
}

pub struct PlatformStore {
    open_url: Option<OpenUrlFn>,
    save_attachment: Option<SaveAttachmentFn>,
    notifier: Option<NotifyFn>,
}

impl PlatformStore {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            open_url: None,
            save_attachment: None,
            notifier: None,
        });
        cx.set_global(GlobalPlatformStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalPlatformStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalPlatformStore>().map(|g| g.0.clone())
    }

    pub fn set_open_url(entity: &Entity<Self>, f: OpenUrlFn, cx: &mut App) {
        entity.update(cx, |store, cx| {
            store.open_url = Some(f);
            cx.notify();
        });
    }

    pub fn set_save_attachment(entity: &Entity<Self>, f: SaveAttachmentFn, cx: &mut App) {
        entity.update(cx, |store, cx| {
            store.save_attachment = Some(f);
            cx.notify();
        });
    }

    pub fn open_url_external(&self, url: &str) -> anyhow::Result<()> {
        match &self.open_url {
            Some(f) => f(url),
            None => Err(anyhow::anyhow!("open_url not registered")),
        }
    }

    pub fn save_attachment(&self, url: &str, filename: &str) -> anyhow::Result<()> {
        match &self.save_attachment {
            Some(f) => f(url, filename),
            None => Err(anyhow::anyhow!("save_attachment not registered")),
        }
    }

    pub fn set_notifier(entity: &Entity<Self>, f: NotifyFn, cx: &mut App) {
        entity.update(cx, |store, cx| {
            store.notifier = Some(f);
            cx.notify();
        });
    }

    pub fn show_notification(&self, notification: DesktopNotification) {
        if let Some(f) = &self.notifier {
            f(notification);
        }
    }
}

struct GlobalPlatformStore(Entity<PlatformStore>);
impl Global for GlobalPlatformStore {}
