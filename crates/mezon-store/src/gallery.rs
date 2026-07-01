//! Channel attachment domain model + gallery store — the native counterpart of
//! React's `gallery.slice` and `attachment.slice`. `ChannelAttachment` is the
//! richer media entity returned by `ListChannelAttachment` (distinct from the
//! inline `MessageAttachment`), carrying the uploader, originating message, and
//! creation timestamp used for date grouping and pagination cursors.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, SharedString};
use mezon_client::AppApi;
use mezon_client::transport::ApiChannelAttachment;

use crate::config::AppConfig;
use crate::ids::{ChannelId, ClanId, MessageId, UserId};

/// Gallery server cache TTL (React `GALLERY_CACHED_TIME` = 1 hour).
pub const GALLERY_CACHE_TTL: Duration = Duration::from_secs(60 * 60);
/// Default page size (React gallery slice `limit = 50`).
pub const GALLERY_PAGE_SIZE: i32 = 50;

/// Client-side media tab filter (React `MediaFilterType`). Switching tabs never
/// refetches — it filters the already-loaded list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaFilter {
    #[default]
    All,
    Image,
    Video,
}

impl MediaFilter {
    fn matches(self, att: &ChannelAttachment) -> bool {
        match self {
            MediaFilter::All => att.is_image || att.is_video,
            MediaFilter::Image => att.is_image,
            MediaFilter::Video => att.is_video,
        }
    }
}

/// A single media attachment in a channel — image or video.
#[derive(Debug, Clone)]
pub struct ChannelAttachment {
    pub id: i64,
    pub channel_id: ChannelId,
    pub clan_id: ClanId,
    pub message_id: MessageId,
    pub uploader_id: UserId,
    /// Original CDN url (used for copy-link, download, open-in-browser).
    pub url: String,
    pub filename: String,
    pub filetype: String,
    pub width: u32,
    pub height: u32,
    pub create_time_seconds: u32,
    pub is_image: bool,
    pub is_video: bool,
    /// Proxied square thumbnail for the gallery grid.
    pub thumb_src: SharedString,
    /// Proxied full-size url for the image viewer.
    pub viewer_src: SharedString,
    /// Calendar-day label ("January 01, 2021") for date grouping headers.
    pub day_label: SharedString,
    /// Days since the unix epoch (local), the date-group key.
    pub day_index: i64,
    /// Resolved uploader display name (filled by [`enrich_uploader`]).
    pub uploader_name: SharedString,
    /// Resolved uploader avatar url (filled by [`enrich_uploader`]).
    pub uploader_avatar: SharedString,
}

impl ChannelAttachment {
    pub fn from_api(
        api: ApiChannelAttachment,
        channel_id: ChannelId,
        clan_id: ClanId,
        cfg: &AppConfig,
    ) -> Self {
        let is_video = is_video_type(&api.filetype, &api.url);
        let is_image = !is_video && is_image_type(&api.filetype, &api.url);
        let (thumb_src, viewer_src) = if is_video {
            (SharedString::default(), SharedString::default())
        } else {
            (
                cfg.gallery_thumb_proxy(&api.url).into(),
                cfg.viewer_proxy(&api.url, api.width.max(0) as u32, api.height.max(0) as u32)
                    .into(),
            )
        };
        let day_label = format_day(api.create_time_seconds as i64).into();
        let day_index = day_index(api.create_time_seconds as i64);
        Self {
            id: api.id,
            channel_id,
            clan_id,
            message_id: MessageId(api.message_id),
            uploader_id: UserId(api.uploader),
            url: api.url,
            filename: api.filename,
            filetype: api.filetype,
            width: api.width.max(0) as u32,
            height: api.height.max(0) as u32,
            create_time_seconds: api.create_time_seconds,
            is_image,
            is_video,
            thumb_src,
            viewer_src,
            day_label,
            day_index,
            uploader_name: SharedString::default(),
            uploader_avatar: SharedString::default(),
        }
    }

    pub fn is_media(&self) -> bool {
        self.is_image || self.is_video
    }
}

/// Fetch and map a page of channel attachments to domain types. Keeps DTO
/// decoding inside the store so views (e.g. the image viewer) consume only
/// [`ChannelAttachment`]. `before`/`after` are unix-second cursors (0 = unset).
pub async fn fetch_channel_attachments(
    api: Arc<AppApi>,
    cfg: AppConfig,
    clan_id: ClanId,
    channel_id: ChannelId,
    before: u32,
    after: u32,
    limit: i32,
) -> anyhow::Result<Vec<ChannelAttachment>> {
    let list = api
        .list_channel_attachments(clan_id.0, channel_id.0, "", 0, limit, before, after)
        .await?;
    Ok(list
        .into_iter()
        .map(|a| ChannelAttachment::from_api(a, channel_id, clan_id, &cfg))
        .filter(ChannelAttachment::is_media)
        .collect())
}

/// Resolved uploader display info.
#[derive(Debug, Clone, Default)]
pub struct UploaderInfo {
    pub name: String,
    pub avatar: String,
}

/// Fill each attachment's uploader name/avatar via `resolve` (the native analog
/// of React `getAttachmentDataForWindow`). Falls back to "Anonymous" when the
/// uploader cannot be resolved.
pub fn enrich_uploader<F>(attachments: &mut [ChannelAttachment], resolve: F)
where
    F: Fn(UserId) -> Option<UploaderInfo>,
{
    for att in attachments.iter_mut() {
        match resolve(att.uploader_id) {
            Some(info) if !info.name.is_empty() => {
                att.uploader_name = info.name.into();
                att.uploader_avatar = info.avatar.into();
            }
            _ => {
                att.uploader_name = "Anonymous".into();
            }
        }
    }
}

fn is_video_type(filetype: &str, url: &str) -> bool {
    if filetype.starts_with("video/") {
        return true;
    }
    let lower = filetype.to_ascii_lowercase();
    if lower.contains("mp4") || lower.contains("mov") || lower.contains("webm") {
        return true;
    }
    matches!(
        extension(url).as_deref(),
        Some("mp4" | "mov" | "webm" | "mkv" | "avi")
    )
}

fn is_image_type(filetype: &str, url: &str) -> bool {
    if filetype.starts_with("image/") || filetype == "sticker" {
        return true;
    }
    matches!(
        extension(url).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "avif")
    )
}

fn extension(url: &str) -> Option<String> {
    url.split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit('.').next())
        .map(|ext| ext.to_ascii_lowercase())
}

fn format_day(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.format("%B %d, %Y").to_string())
        .unwrap_or_default()
}

fn day_index(ts: i64) -> i64 {
    ts.div_euclid(86_400)
}

/// Pagination/scroll direction for [`GalleryStore::fetch_page`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadDirection {
    /// Older attachments (scroll down past the oldest loaded item).
    Before,
    /// Newer attachments (scroll up past the newest loaded item).
    After,
}

#[derive(Default)]
struct GalleryChannel {
    attachments: Vec<ChannelAttachment>,
    has_more_before: bool,
    has_more_after: bool,
    is_loading: bool,
    fetched_at: Option<Instant>,
    date_range: Option<(u32, u32)>,
}

impl GalleryChannel {
    fn is_fresh(&self) -> bool {
        self.fetched_at
            .is_some_and(|t| t.elapsed() < GALLERY_CACHE_TTL)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum GalleryEvent {
    Changed(ChannelId),
}

/// Per-channel gallery attachment lists with bidirectional pagination. Unlike
/// realtime stores this is fetched on demand (when the gallery opens) and not
/// subscribed to the broadcast.
pub struct GalleryStore {
    by_channel: HashMap<ChannelId, GalleryChannel>,
    api: Arc<AppApi>,
}

struct GlobalGalleryStore(Entity<GalleryStore>);
impl Global for GlobalGalleryStore {}

impl EventEmitter<GalleryEvent> for GalleryStore {}

impl GalleryStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            by_channel: HashMap::new(),
            api,
        });
        cx.set_global(GlobalGalleryStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalGalleryStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalGalleryStore>().map(|g| g.0.clone())
    }

    /// Shared API handle for callers (e.g. the image viewer) that need to fetch
    /// channel attachments outside the gallery's per-channel list.
    pub fn api(&self) -> Arc<AppApi> {
        self.api.clone()
    }

    pub fn attachments(&self, channel_id: ChannelId) -> &[ChannelAttachment] {
        self.by_channel
            .get(&channel_id)
            .map(|c| c.attachments.as_slice())
            .unwrap_or(&[])
    }

    /// Attachments filtered by the given media tab.
    pub fn filtered(&self, channel_id: ChannelId, filter: MediaFilter) -> Vec<ChannelAttachment> {
        self.attachments(channel_id)
            .iter()
            .filter(|a| filter.matches(a))
            .cloned()
            .collect()
    }

    pub fn is_loading(&self, channel_id: ChannelId) -> bool {
        self.by_channel
            .get(&channel_id)
            .is_some_and(|c| c.is_loading)
    }

    pub fn has_more_before(&self, channel_id: ChannelId) -> bool {
        self.by_channel
            .get(&channel_id)
            .is_some_and(|c| c.has_more_before)
    }

    pub fn has_more_after(&self, channel_id: ChannelId) -> bool {
        self.by_channel
            .get(&channel_id)
            .is_some_and(|c| c.has_more_after)
    }

    pub fn date_range(&self, channel_id: ChannelId) -> Option<(u32, u32)> {
        self.by_channel.get(&channel_id).and_then(|c| c.date_range)
    }

    pub fn is_empty(&self, channel_id: ChannelId) -> bool {
        self.attachments(channel_id).is_empty()
    }

    /// Fetch the first page if the channel is not already loaded and fresh
    /// (React gallery button: fetch on first open).
    pub fn ensure_loaded(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let needs_fetch = match self.by_channel.get(&channel_id) {
            Some(c) => !c.is_loading && (c.attachments.is_empty() || !c.is_fresh()),
            None => true,
        };
        if needs_fetch {
            self.fetch(clan_id, channel_id, None, LoadDirection::Before, true, cx);
        }
    }

    pub fn refresh(&mut self, clan_id: ClanId, channel_id: ChannelId, cx: &mut Context<Self>) {
        let range = self.date_range(channel_id);
        self.fetch(clan_id, channel_id, range, LoadDirection::Before, true, cx);
    }

    /// Load the next page in `direction` for infinite scroll.
    pub fn fetch_page(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        direction: LoadDirection,
        cx: &mut Context<Self>,
    ) {
        let Some(channel) = self.by_channel.get(&channel_id) else {
            return;
        };
        if channel.is_loading {
            return;
        }
        match direction {
            LoadDirection::Before if !channel.has_more_before => return,
            LoadDirection::After if !channel.has_more_after => return,
            _ => {}
        }
        let range = channel.date_range;
        self.fetch(clan_id, channel_id, range, direction, false, cx);
    }

    /// Apply a from/to date filter (unix seconds) and refetch from scratch.
    pub fn apply_date_filter(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        after: Option<u32>,
        before: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        let range = match (after, before) {
            (None, None) => None,
            (a, b) => Some((a.unwrap_or(0), b.unwrap_or(0))),
        };
        self.fetch(
            clan_id,
            channel_id,
            range,
            LoadDirection::Before,
            true,
            cx,
        );
    }

    pub fn clear_date_filter(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.fetch(clan_id, channel_id, None, LoadDirection::Before, true, cx);
    }

    pub fn clear_attachments(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if self.reset_channel_attachments(channel_id) {
            cx.emit(GalleryEvent::Changed(channel_id));
            cx.notify();
        }
    }

    pub fn reset_channel_attachments(&mut self, channel_id: ChannelId) -> bool {
        if let Some(channel) = self.by_channel.get_mut(&channel_id) {
            channel.attachments.clear();
            channel.date_range = None;
            channel.fetched_at = None;
            channel.is_loading = false;
            channel.has_more_before = true;
            channel.has_more_after = true;
            true
        } else {
            false
        }
    }

    pub fn clear_channel(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if self.by_channel.remove(&channel_id).is_some() {
            cx.emit(GalleryEvent::Changed(channel_id));
            cx.notify();
        }
    }

    fn fetch(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        date_range: Option<(u32, u32)>,
        direction: LoadDirection,
        reset: bool,
        cx: &mut Context<Self>,
    ) {
        let (mut before, mut after) = date_range.map(|(a, b)| (b, a)).unwrap_or((0, 0));
        if !reset {
            let channel = self.by_channel.get(&channel_id);
            match direction {
                LoadDirection::Before => {
                    if let Some(oldest) = channel.and_then(|c| c.attachments.last()) {
                        before = oldest.create_time_seconds;
                    }
                }
                LoadDirection::After => {
                    if let Some(newest) = channel.and_then(|c| c.attachments.first()) {
                        after = newest.create_time_seconds;
                    }
                }
            }
        }

        let entry = self.by_channel.entry(channel_id).or_default();
        entry.is_loading = true;
        if reset {
            entry.date_range = date_range;
        }
        cx.emit(GalleryEvent::Changed(channel_id));
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_attachments(
                    clan_id.0,
                    channel_id.0,
                    "",
                    0,
                    GALLERY_PAGE_SIZE,
                    before,
                    after,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                let cfg = AppConfig::try_global(cx);
                let entry = this.by_channel.entry(channel_id).or_default();
                entry.is_loading = false;
                match result {
                    Ok(list) => {
                        let mapped: Vec<ChannelAttachment> = match &cfg {
                            Some(cfg) => list
                                .into_iter()
                                .map(|a| ChannelAttachment::from_api(a, channel_id, clan_id, cfg))
                                .filter(ChannelAttachment::is_media)
                                .collect(),
                            None => Vec::new(),
                        };
                        let fetched = mapped.len();
                        let added = merge_attachments(&mut entry.attachments, mapped, reset);
                        let full_page = fetched as i32 >= GALLERY_PAGE_SIZE;
                        let progressed = added > 0;
                        if reset {
                            entry.has_more_before = full_page;
                            entry.has_more_after = false;
                        } else {
                            match direction {
                                LoadDirection::Before => {
                                    entry.has_more_before = full_page && progressed;
                                }
                                LoadDirection::After => {
                                    entry.has_more_after = full_page && progressed;
                                }
                            }
                        }
                        entry.fetched_at = Some(Instant::now());
                    }
                    Err(e) => {
                        tracing::error!("list_channel_attachments failed: {e}");
                    }
                }
                cx.emit(GalleryEvent::Changed(channel_id));
                cx.notify();
            });
        })
        .detach();
    }
}

/// Merge `incoming` into `existing`, de-duplicating by attachment id and keeping
/// the list sorted newest-first. Returns the number of newly added items (0 =
/// duplicate page, used to stop pagination). `reset` replaces the list.
fn merge_attachments(
    existing: &mut Vec<ChannelAttachment>,
    incoming: Vec<ChannelAttachment>,
    reset: bool,
) -> usize {
    if reset {
        existing.clear();
    }
    let mut seen: std::collections::HashSet<i64> = existing.iter().map(|a| a.id).collect();
    let mut added = 0;
    for att in incoming {
        if seen.insert(att.id) {
            existing.push(att);
            added += 1;
        }
    }
    existing.sort_by_key(|a| std::cmp::Reverse(a.create_time_seconds));
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(id: i64, ts: u32, filetype: &str) -> ChannelAttachment {
        ChannelAttachment {
            id,
            channel_id: ChannelId(1),
            clan_id: ClanId(1),
            message_id: MessageId(id),
            uploader_id: UserId(7),
            url: format!("https://cdn.mezon.ai/{id}.png"),
            filename: format!("{id}.png"),
            filetype: filetype.to_string(),
            width: 100,
            height: 100,
            create_time_seconds: ts,
            is_image: filetype.starts_with("image/"),
            is_video: filetype.starts_with("video/"),
            thumb_src: SharedString::default(),
            viewer_src: SharedString::default(),
            day_label: SharedString::default(),
            day_index: 0,
            uploader_name: SharedString::default(),
            uploader_avatar: SharedString::default(),
        }
    }

    #[test]
    fn merge_reset_replaces_and_sorts_desc() {
        let mut existing = vec![att(1, 100, "image/png")];
        let added = merge_attachments(
            &mut existing,
            vec![att(2, 300, "image/png"), att(3, 200, "image/png")],
            true,
        );
        assert_eq!(added, 2);
        assert_eq!(
            existing.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn merge_appends_older_dedup() {
        let mut existing = vec![att(2, 300, "image/png"), att(3, 200, "image/png")];
        let added = merge_attachments(
            &mut existing,
            vec![att(3, 200, "image/png"), att(4, 100, "image/png")],
            false,
        );
        assert_eq!(added, 1);
        assert_eq!(
            existing.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
    }

    #[test]
    fn merge_duplicate_page_returns_zero() {
        let mut existing = vec![att(2, 300, "image/png"), att(3, 200, "image/png")];
        let added = merge_attachments(
            &mut existing,
            vec![att(2, 300, "image/png"), att(3, 200, "image/png")],
            false,
        );
        assert_eq!(added, 0);
    }

    #[test]
    fn media_filter_matches() {
        let img = att(1, 1, "image/png");
        let vid = att(2, 2, "video/mp4");
        assert!(MediaFilter::All.matches(&img));
        assert!(MediaFilter::All.matches(&vid));
        assert!(MediaFilter::Image.matches(&img));
        assert!(!MediaFilter::Image.matches(&vid));
        assert!(MediaFilter::Video.matches(&vid));
        assert!(!MediaFilter::Video.matches(&img));
    }

    #[test]
    fn detects_video_and_image_types() {
        assert!(is_video_type("video/mp4", ""));
        assert!(is_video_type("", "https://cdn.mezon.ai/clip.mov"));
        assert!(is_image_type("image/png", ""));
        assert!(is_image_type("", "https://cdn.mezon.ai/pic.JPG"));
        assert!(!is_image_type(
            "application/pdf",
            "https://cdn.mezon.ai/a.pdf"
        ));
    }

    #[test]
    fn enrich_falls_back_to_anonymous() {
        let mut atts = vec![att(1, 1, "image/png")];
        enrich_uploader(&mut atts, |_| None);
        assert_eq!(atts[0].uploader_name, SharedString::from("Anonymous"));
    }
}
