use std::future::Future;
use std::sync::Arc;

use futures::future::{AbortHandle, Abortable};
use futures::{AsyncReadExt as _, FutureExt};
use gpui::{
    App, Asset, AssetLogger, Context, ImageAssetLoader, ImageCache, ImageCacheError,
    ImageCacheItem, RenderImage, Resource, Window, hash,
};
use indexmap::IndexMap;

pub const MESSAGE_IMAGE_CACHE_CAPACITY: usize = 48;
pub const MESSAGE_IMAGE_CACHE_BYTES: u64 = 48 * 1024 * 1024;
pub const AVATAR_IMAGE_CACHE_CAPACITY: usize = 256;
pub const AVATAR_IMAGE_CACHE_BYTES: u64 = 16 * 1024 * 1024;

/// App-wide fallback cache attached at the root, so any `img`/avatar that does
/// not declare its own cache uses this bounded LRU instead of GPUI's unbounded
/// global asset cache (which never evicts and leaks RAM for every URL seen).
pub const SHARED_IMAGE_CACHE_CAPACITY: usize = 384;
pub const SHARED_IMAGE_CACHE_BYTES: u64 = 64 * 1024 * 1024;

/// Per-image decoded-size caps. A compressed file is tiny on the wire but is
/// stored uncompressed in RAM as `width * height * 4` bytes *per frame*. An
/// animated GIF/WebP therefore explodes: a ~400 KB animated avatar can decode
/// to hundreds of MB once every frame is expanded. When the resizing image
/// proxy is unavailable (dev, or a prod outage) we fall back to the raw,
/// full-resolution file, so we guard against a single pathological image
/// blowing up RAM by refusing to retain anything decoded larger than this and
/// negatively caching it (shown as the initials fallback instead).
pub const AVATAR_ENTRY_MAX_BYTES: u64 = 8 * 1024 * 1024;
pub const MESSAGE_ENTRY_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const SHARED_ENTRY_MAX_BYTES: u64 = 16 * 1024 * 1024;

struct CacheEntry {
    item: ImageCacheItem,
    abort: AbortHandle,
    /// Decoded size in bytes, once the image has finished loading.
    bytes: Option<u64>,
    /// The sweep epoch in which this entry was last requested.
    touched_epoch: u64,
}

/// Sum of the decoded byte size across all frames of an image.
fn image_bytes(image: &RenderImage) -> u64 {
    (0..image.frame_count())
        .filter_map(|frame| image.as_bytes(frame))
        .map(|buf| buf.len() as u64)
        .sum()
}

/// An LRU image cache bounded by both an item count and a decoded-byte budget.
///
/// The byte budget is what actually keeps RAM in check: large attachments are
/// evicted as soon as the total decoded size exceeds `max_bytes`, instead of
/// lingering until the (much larger) item count or a channel switch clears them.
static CACHE_INSTANCE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Which decoder a cache uses to turn a resource into a `RenderImage`.
#[derive(Clone, Copy)]
enum LoaderKind {
    /// GPUI's stock loader: decodes the image at full resolution and keeps every
    /// frame of animated GIF/WebP. Used for message attachments that must render
    /// full-size and animated.
    Full,
    /// Decodes only the first frame and downscales to avatar size, so even an
    /// animated full-resolution source costs ~100 KB of RAM. Used for avatars.
    AvatarThumbnail,
}

pub struct LruImageCache {
    label: &'static str,
    instance: u64,
    loader: LoaderKind,
    max_items: usize,
    max_bytes: u64,
    /// Largest decoded size (bytes, summed across frames) a single entry may
    /// have before it is dropped and negatively cached. Protects against a
    /// single huge/animated image consuming hundreds of MB.
    max_entry_bytes: u64,
    total_bytes: u64,
    epoch: u64,
    cache: IndexMap<u64, CacheEntry>,
}

impl LruImageCache {
    pub fn new(max_items: usize, max_bytes: u64, cx: &mut Context<Self>) -> Self {
        Self::labeled("image", max_items, max_bytes, u64::MAX, cx)
    }

    pub fn labeled(
        label: &'static str,
        max_items: usize,
        max_bytes: u64,
        max_entry_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_loader(
            label,
            LoaderKind::Full,
            max_items,
            max_bytes,
            max_entry_bytes,
            cx,
        )
    }

    /// A cache for avatars: decodes only the first frame and downscales to
    /// avatar size, so animated or oversized sources can never blow up RAM.
    pub fn avatar_thumbnail(
        label: &'static str,
        max_items: usize,
        max_bytes: u64,
        max_entry_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_loader(
            label,
            LoaderKind::AvatarThumbnail,
            max_items,
            max_bytes,
            max_entry_bytes,
            cx,
        )
    }

    fn with_loader(
        label: &'static str,
        loader: LoaderKind,
        max_items: usize,
        max_bytes: u64,
        max_entry_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|cache, cx| {
            for (_, mut entry) in std::mem::take(&mut cache.cache) {
                entry.abort.abort();
                if let Some(Ok(image)) = entry.item.get() {
                    cx.drop_image(image, None);
                }
            }
        })
        .detach();

        let instance = CACHE_INSTANCE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            label,
            instance,
            loader,
            max_items,
            max_bytes,
            max_entry_bytes,
            total_bytes: 0,
            epoch: 0,
            cache: IndexMap::with_capacity(max_items),
        }
    }

    pub fn clear(&mut self, window: &mut Window, cx: &mut App) {
        for (_, mut entry) in std::mem::take(&mut self.cache) {
            entry.abort.abort();
            if let Some(Ok(image)) = entry.item.get() {
                cx.drop_image(image, Some(window));
            }
        }
        self.total_bytes = 0;
    }

    /// Drop every image that was not requested during the most recent frame,
    /// then advance the epoch. Call this once per render: anything that has
    /// scrolled out of the viewport stops being requested and is freed on the
    /// next sweep, so only the currently-visible images stay in RAM.
    pub fn sweep(&mut self, window: &mut Window, cx: &mut App) {
        let epoch = self.epoch;
        let stale: Vec<u64> = self
            .cache
            .iter()
            .filter(|(_, entry)| entry.touched_epoch != epoch)
            .map(|(key, _)| *key)
            .collect();
        for key in stale {
            if let Some(mut entry) = self.cache.shift_remove(&key) {
                entry.abort.abort();
                if let Some(bytes) = entry.bytes {
                    self.total_bytes = self.total_bytes.saturating_sub(bytes);
                }
                if let Some(Ok(image)) = entry.item.get() {
                    cx.drop_image(image, Some(window));
                }
            }
        }
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Evict least-recently-used entries until both the item-count and
    /// byte budgets are satisfied. The most-recently-used entry (back of the
    /// map) is never evicted, so the image requested this frame stays resident.
    fn evict_to_budget(&mut self, window: &mut Window, cx: &mut App) {
        while self.cache.len() > self.max_items
            || (self.total_bytes > self.max_bytes && self.cache.len() > 1)
        {
            let Some((_, mut evicted)) = self.cache.shift_remove_index(0) else {
                break;
            };
            evicted.abort.abort();
            if let Some(bytes) = evicted.bytes {
                self.total_bytes = self.total_bytes.saturating_sub(bytes);
            }
            if let Some(Ok(image)) = evicted.item.get() {
                cx.drop_image(image, Some(window));
            }
        }
    }

    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let hash = hash(resource);

        if let Some(entry) = self.cache.shift_remove(&hash) {
            self.cache.insert(hash, entry);

            enum Measured {
                /// Nothing new to account for (already measured, or still loading).
                None,
                /// Newly decoded image of the given size, kept in the cache.
                Kept(u64),
                /// Newly decoded image exceeded the per-entry cap: dropped and
                /// negatively cached. Carries the image to free + the error.
                TooLarge(Arc<RenderImage>, ImageCacheError),
            }

            let (res, measured) = {
                let entry = self.cache.get_mut(&hash).expect("just re-inserted");
                entry.touched_epoch = self.epoch;
                let res = entry.item.get();
                let measured = if entry.bytes.is_none()
                    && let Some(Ok(image)) = res.as_ref()
                {
                    let bytes = image_bytes(image);
                    if bytes > self.max_entry_bytes {
                        let err = ImageCacheError::Other(Arc::new(anyhow::anyhow!(
                            "image decoded to {bytes} bytes, exceeds per-entry cap of {} bytes",
                            self.max_entry_bytes
                        )));
                        entry.item = ImageCacheItem::Loaded(Err(err.clone()));
                        entry.bytes = Some(0);
                        Measured::TooLarge(image.clone(), err)
                    } else {
                        entry.bytes = Some(bytes);
                        Measured::Kept(bytes)
                    }
                } else {
                    Measured::None
                };
                (res, measured)
            };
            match measured {
                Measured::Kept(bytes) => {
                    self.total_bytes = self.total_bytes.saturating_add(bytes);
                    self.evict_to_budget(window, cx);
                    return res;
                }
                Measured::TooLarge(image, err) => {
                    tracing::warn!(
                        "[imgcache:{}#{}] dropping oversized image: {}",
                        self.label,
                        self.instance,
                        err
                    );
                    cx.drop_image(image, Some(window));
                    return Some(Err(err));
                }
                Measured::None => return res,
            }
        }

        let fut = match self.loader {
            LoaderKind::Full => AssetLogger::<ImageAssetLoader>::load(resource.clone(), cx).boxed(),
            LoaderKind::AvatarThumbnail => {
                AssetLogger::<AvatarImageLoader>::load(resource.clone(), cx).boxed()
            }
        };
        let task = cx.background_executor().spawn(fut).shared();
        let (abort_handle, abort_reg) = AbortHandle::new_pair();

        self.cache.insert(
            hash,
            CacheEntry {
                item: ImageCacheItem::Loading(task.clone()),
                abort: abort_handle,
                bytes: None,
                touched_epoch: self.epoch,
            },
        );
        self.evict_to_budget(window, cx);

        let entity = window.current_view();
        let notify_task = task.clone();
        window
            .spawn(cx, async move |cx| {
                let _ = Abortable::new(notify_task, abort_reg).await;
                cx.on_next_frame(move |_, cx| {
                    cx.notify(entity);
                });
            })
            .detach();

        None
    }
}

impl ImageCache for LruImageCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        LruImageCache::load(self, resource, window, cx)
    }
}

/// Largest dimension (device pixels) an avatar is ever drawn at: the biggest
/// avatar is 80px logical, which is 160px on a 2x display. Decoding to this size
/// keeps a single avatar at ~100 KB regardless of the source file.
const AVATAR_DECODE_MAX_PX: u32 = 160;

/// An [`Asset`] loader for avatars that, unlike GPUI's stock [`ImageAssetLoader`],
/// decodes **only the first frame** and **downscales** to avatar size before
/// building the `RenderImage`.
///
/// GPUI's loader expands every frame of an animated GIF/WebP to
/// `width * height * 4` uncompressed bytes and keeps them all, so a ~400 KB
/// animated avatar can decode to hundreds of MB. Avatars never need animation
/// or full resolution, so we sidestep that entirely: `image::load_from_memory`
/// reads a single frame even for animated formats, and we shrink it to at most
/// [`AVATAR_DECODE_MAX_PX`]. The result is a tiny, static image that cannot blow
/// up RAM even when the resizing image proxy is unavailable and we fall back to
/// the raw source file.
pub enum AvatarImageLoader {}

impl Asset for AvatarImageLoader {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        let svg_renderer = cx.svg_renderer();
        let asset_source = cx.asset_source().clone();
        async move {
            let bytes = match source.clone() {
                Resource::Path(uri) => std::fs::read(uri.as_ref())?,
                Resource::Uri(uri) => {
                    use anyhow::Context as _;

                    let mut response = client
                        .get(uri.as_ref(), ().into(), true)
                        .await
                        .with_context(|| format!("loading avatar from {uri:?}"))?;
                    let mut body = Vec::new();
                    response.body_mut().read_to_end(&mut body).await?;
                    if !response.status().is_success() {
                        let mut body = String::from_utf8_lossy(&body).into_owned();
                        let first_line = body.lines().next().unwrap_or("").trim_end();
                        body.truncate(first_line.len());
                        return Err(ImageCacheError::BadStatus {
                            uri,
                            status: response.status(),
                            body,
                        });
                    }
                    body
                }
                Resource::Embedded(path) => match asset_source.load(&path).ok().flatten() {
                    Some(data) => data.to_vec(),
                    None => {
                        return Err(ImageCacheError::Asset(
                            format!("Embedded resource not found: {path}").into(),
                        ));
                    }
                },
            };

            if image::guess_format(&bytes).is_ok() {
                // `load_from_memory` decodes a single frame even for animated
                // GIF/WebP, so this never expands the whole animation.
                let decoded = image::load_from_memory(&bytes)?;
                // Center-crop to a square (cover), so non-square sources render
                // as a clean circle/rounded box instead of overflowing the
                // avatar bounds. Cap the side so we only ever downscale.
                let side = decoded
                    .width()
                    .min(decoded.height())
                    .min(AVATAR_DECODE_MAX_PX)
                    .max(1);
                let mut data = decoded
                    .resize_to_fill(side, side, image::imageops::FilterType::Triangle)
                    .into_rgba8();
                // GPUI expects BGRA, so swap the red and blue channels.
                for pixel in data.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
                Ok(Arc::new(RenderImage::new(vec![image::Frame::new(data)])))
            } else {
                svg_renderer
                    .render_single_frame(&bytes, 1.0)
                    .map_err(Into::into)
            }
        }
    }
}
