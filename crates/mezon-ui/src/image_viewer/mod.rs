use std::borrow::{Borrow, BorrowMut};
use std::sync::Arc;

use futures::AsyncReadExt as _;
use gpui::{
    App, AppContext, Bounds, Context, Corners, Entity, FocusHandle, Focusable, ImageCache,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, RenderImage,
    Resource, ScrollDelta, ScrollWheelEvent, SharedString, SharedUri,
    UniformListScrollHandle, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, canvas,
    div, img, point, prelude::*, px, relative, size, uniform_list,
};
use gpui::http_client::HttpClient;
use gpui::Size as GpuiSize;
use mezon_store::{
    AppConfig, ChannelAttachment, ChannelId, ClanId, ClanMembersStore, GalleryStore, PlatformStore,
    Settings, UserId, fetch_channel_attachments,
};

use crate::app::main_window::activate_main_window;
use crate::components::primitives::{Avatar, Icon, IconName, Sizable, Size, Spinner};
use crate::components::primitives::{ContextMenu, context_menu_at};
use crate::image_cache::LruImageCache;
use crate::theme::{ActiveTheme, Theme};

const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 5.0;
const ZOOM_STEP: f32 = 0.25;
const THUMB_ROW_HEIGHT: f32 = 72.0;
const SIDEBAR_WIDTH: f32 = 96.0;
const VIEWER_FETCH_LIMIT: i32 = 50;
const LOAD_MORE_THRESHOLD: usize = 3;

/// Request to open (or update) the image viewer. Built by entry points
/// (gallery tile click, message attachment click).
pub struct OpenViewerRequest {
    pub clan_id: ClanId,
    pub channel_id: ChannelId,
    pub channel_label: SharedString,
    pub settings: Entity<Settings>,
    /// Pre-loaded playlist (gallery path). Empty for message-click, which fetches.
    pub attachments: Vec<ChannelAttachment>,
    pub selected_index: usize,
    /// Select this url once the playlist is loaded (message-click path).
    pub selected_url: Option<SharedString>,
    /// Fetch anchor (unix seconds, message create_time + 1 day) for message-click.
    pub anchor_before: Option<u32>,
}

struct GlobalImageViewer(WindowHandle<ImageViewer>);
impl gpui::Global for GlobalImageViewer {}

fn clear_image_viewer_global(cx: &mut App) {
    if cx.try_global::<GlobalImageViewer>().is_some() {
        cx.remove_global::<GlobalImageViewer>();
    }
}

/// Open the image viewer, reusing the existing window if one is already open
pub fn open_image_viewer(request: OpenViewerRequest, cx: &mut App) {
    let mut pending = Some(request);
    if let Some(handle) = cx.try_global::<GlobalImageViewer>().map(|g| g.0) {
        let updated = handle
            .update(cx, |viewer, window, cx| {
                if let Some(req) = pending.take() {
                    viewer.set_request(req, window, cx);
                }
                window.activate_window();
            })
            .is_ok();
        if updated {
            return;
        }
        clear_image_viewer_global(cx);
    }
    let Some(request) = pending else {
        return;
    };

    let bounds = Bounds::centered(None, size(px(1100.0), px(740.0)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(640.0), px(480.0))),
        kind: WindowKind::Normal,
        focus: true,
        show: true,
        ..Default::default()
    };

    match cx.open_window(options, |window, cx| {
        cx.new(|cx| ImageViewer::new(request, window, cx))
    }) {
        Ok(handle) => cx.set_global(GlobalImageViewer(handle)),
        Err(e) => tracing::error!("failed to open image viewer window: {e}"),
    }
}

pub struct ImageViewer {
    focus_handle: FocusHandle,
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    attachments: Vec<ChannelAttachment>,
    index: usize,
    zoom: f32,
    pan: Point<Pixels>,
    drag_from: Option<Point<Pixels>>,
    show_thumbnails: bool,
    context_menu: Option<Point<Pixels>>,
    has_more_before: bool,
    loading: bool,
    rotation_deg: i32,
    rotated_image: Option<Arc<RenderImage>>,
    rotation_loading: bool,
    image_cache: Entity<LruImageCache>,
    list_scroll: UniformListScrollHandle,
}

impl ImageViewer {
    fn new(request: OpenViewerRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        window.on_window_should_close((&*cx).borrow(), |_, app| {
            clear_image_viewer_global(app);
            activate_main_window(app);
            true
        });
        let image_cache = cx.new(|cx| LruImageCache::new(64, 384 * 1024 * 1024, cx));
        let mut this = Self {
            focus_handle,
            clan_id: request.clan_id,
            channel_id: request.channel_id,
            settings: request.settings.clone(),
            attachments: Vec::new(),
            index: 0,
            zoom: MIN_ZOOM,
            pan: point(px(0.), px(0.)),
            drag_from: None,
            show_thumbnails: true,
            context_menu: None,
            has_more_before: true,
            loading: false,
            rotation_deg: 0,
            rotated_image: None,
            rotation_loading: false,
            image_cache,
            list_scroll: UniformListScrollHandle::new(),
        };
        this.set_request(request, window, cx);
        this
    }

    fn set_request(
        &mut self,
        request: OpenViewerRequest,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clan_id = request.clan_id;
        self.channel_id = request.channel_id;
        self.settings = request.settings;
        self.zoom = MIN_ZOOM;
        self.pan = point(px(0.), px(0.));
        self.rotation_deg = 0;
        self.rotated_image = None;
        self.rotation_loading = false;
        self.context_menu = None;
        self.attachments = request.attachments;
        self.has_more_before = true;

        if let Some(url) = &request.selected_url {
            self.index = self
                .attachments
                .iter()
                .position(|a| a.viewer_src == *url || a.url.as_str() == url.as_ref())
                .unwrap_or(0);
        } else {
            self.index = request
                .selected_index
                .min(self.attachments.len().saturating_sub(1));
        }

        if self.attachments.is_empty() {
            self.loading = true;
            self.fetch_initial(request.anchor_before, request.selected_url, cx);
        }
        cx.notify();
    }

    fn clan(&self) -> ClanId {
        self.clan_id
    }

    fn channel(&self) -> ChannelId {
        self.channel_id
    }

    fn current(&self) -> Option<&ChannelAttachment> {
        self.attachments.get(self.index)
    }

    fn fetch_initial(
        &mut self,
        anchor_before: Option<u32>,
        select_url: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let (Some(gallery), Some(cfg)) = (
            GalleryStore::try_global(cx),
            AppConfig::try_global(cx).cloned(),
        ) else {
            self.loading = false;
            return;
        };
        let api = gallery.read(cx).api();
        let clan = self.clan();
        let channel = self.channel();
        let before = anchor_before.unwrap_or(0);
        cx.spawn(async move |this, cx| {
            let result =
                fetch_channel_attachments(api, cfg, clan, channel, before, 0, VIEWER_FETCH_LIMIT)
                    .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(mut mapped) => {
                        resolve_uploaders(&mut mapped, clan, cx);
                        this.attachments = mapped;
                        this.has_more_before = this.attachments.len() as i32 >= VIEWER_FETCH_LIMIT;
                        if let Some(url) = &select_url {
                            this.index = this
                                .attachments
                                .iter()
                                .position(|a| a.url.as_str() == url.as_ref())
                                .unwrap_or(0);
                        }
                    }
                    Err(e) => tracing::error!("image viewer fetch failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn fetch_older(&mut self, cx: &mut Context<Self>) {
        if self.loading || !self.has_more_before {
            return;
        }
        let Some(oldest) = self.attachments.last() else {
            return;
        };
        let before = oldest.create_time_seconds;
        let (Some(gallery), Some(cfg)) = (
            GalleryStore::try_global(cx),
            AppConfig::try_global(cx).cloned(),
        ) else {
            return;
        };
        let api = gallery.read(cx).api();
        let clan = self.clan();
        let channel = self.channel();
        self.loading = true;
        cx.spawn(async move |this, cx| {
            let result =
                fetch_channel_attachments(api, cfg, clan, channel, before, 0, VIEWER_FETCH_LIMIT)
                    .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(mut mapped) => {
                        resolve_uploaders(&mut mapped, clan, cx);
                        let existing: std::collections::HashSet<i64> =
                            this.attachments.iter().map(|a| a.id).collect();
                        let before_len = this.attachments.len();
                        for att in mapped {
                            if !existing.contains(&att.id) {
                                this.attachments.push(att);
                            }
                        }
                        let added = this.attachments.len() - before_len;
                        this.has_more_before = added > 0;
                        cx.notify();
                    }
                    Err(e) => tracing::error!("image viewer page fetch failed: {e}"),
                }
            });
        })
        .detach();
    }

    fn go_to(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.attachments.len() {
            return;
        }
        self.index = index;
        self.zoom = MIN_ZOOM;
        self.pan = point(px(0.), px(0.));
        self.rotation_deg = 0;
        self.rotated_image = None;
        self.rotation_loading = false;
        self.context_menu = None;
        if self.index + LOAD_MORE_THRESHOLD >= self.attachments.len() {
            self.fetch_older(cx);
        }
        self.list_scroll
            .scroll_to_item(self.index, gpui::ScrollStrategy::Center);
        cx.notify();
    }

    fn next(&mut self, cx: &mut Context<Self>) {
        if self.index + 1 < self.attachments.len() {
            self.go_to(self.index + 1, cx);
        }
    }

    fn prev(&mut self, cx: &mut Context<Self>) {
        if self.index > 0 {
            self.go_to(self.index - 1, cx);
        }
    }

    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if (self.zoom - MIN_ZOOM).abs() < f32::EPSILON {
            self.pan = point(px(0.), px(0.));
        }
        cx.notify();
    }

    fn reset_zoom(&mut self, cx: &mut Context<Self>) {
        self.set_zoom(MIN_ZOOM, cx);
    }

    fn rotate_left(&mut self, cx: &mut Context<Self>) {
        self.rotation_deg = (self.rotation_deg - 90).rem_euclid(360);
        self.pan = point(px(0.), px(0.));
        self.refresh_rotation(cx);
    }

    fn rotate_right(&mut self, cx: &mut Context<Self>) {
        self.rotation_deg = (self.rotation_deg + 90).rem_euclid(360);
        self.pan = point(px(0.), px(0.));
        self.refresh_rotation(cx);
    }

    fn refresh_rotation(&mut self, cx: &mut Context<Self>) {
        if self.rotation_deg == 0 {
            self.rotated_image = None;
            self.rotation_loading = false;
            cx.notify();
            return;
        }
        let Some(att) = self.current().filter(|a| a.is_image) else {
            self.rotation_deg = 0;
            self.rotated_image = None;
            self.rotation_loading = false;
            cx.notify();
            return;
        };
        let url = att.viewer_src.to_string();
        let degrees = self.rotation_deg;
        self.rotation_loading = true;
        let client = cx.http_client();
        cx.spawn(async move |this, cx| {
            let result = fetch_rotated_image(&url, degrees, client).await;
            let _ = this.update(cx, |this, cx| {
                this.rotation_loading = false;
                match result {
                    Ok(image) => this.rotated_image = Some(image),
                    Err(e) => {
                        tracing::warn!("image rotation failed: {e}");
                        this.rotation_deg = 0;
                        this.rotated_image = None;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn locale(&self, cx: &App) -> String {
        self.settings.read(cx).language.clone()
    }

    fn close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.context_menu = None;
        clear_image_viewer_global(cx.borrow_mut());
        activate_main_window(cx.borrow_mut());
        window.remove_window();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => {
                if self.context_menu.take().is_some() {
                    cx.notify();
                } else {
                    self.close_window(window, cx);
                }
            }
            "left" | "up" => self.prev(cx),
            "right" | "down" => self.next(cx),
            "+" | "=" => self.set_zoom(self.zoom + ZOOM_STEP, cx),
            "-" | "_" => self.set_zoom(self.zoom - ZOOM_STEP, cx),
            "0" => self.reset_zoom(cx),
            _ => {}
        }
    }

    fn on_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dy = match event.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => f32::from(p.y) / 40.0,
        };
        if dy.abs() > f32::EPSILON {
            self.set_zoom(self.zoom + dy * ZOOM_STEP, cx);
        }
    }

    fn copy_link(&self, cx: &mut App) {
        if let Some(att) = self.current() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(att.url.clone()));
        }
    }

    fn open_in_browser(&self, cx: &mut App) {
        if let (Some(att), Some(platform)) = (self.current(), PlatformStore::try_global(cx)) {
            let url = att.url.clone();
            let _ = platform.read(cx).open_url_external(&url);
        }
    }

    fn save_image(&mut self, cx: &mut Context<Self>) {
        let Some(att) = self.current() else {
            return;
        };
        let url = att.url.clone();
        let filename = att.filename.clone();
        cx.spawn(async move |_this, cx| {
            match mezon_store::download_url_to_downloads(&url, &filename).await {
                Ok(path) => {
                    let _ = cx.update(|cx| cx.reveal_path(&path));
                }
                Err(e) => tracing::warn!("save image failed: {e}"),
            }
        })
        .detach();
    }
}

fn resolve_uploaders(attachments: &mut [ChannelAttachment], clan: ClanId, cx: &App) {
    let Some(members) = ClanMembersStore::try_global(cx) else {
        return;
    };
    let cfg = AppConfig::try_global(cx);
    let members = members.read(cx);
    for att in attachments.iter_mut() {
        match member_display(members, clan, att.uploader_id, cfg) {
            Some((name, avatar)) => {
                att.uploader_name = name.into();
                att.uploader_avatar = avatar.into();
            }
            None => att.uploader_name = "Anonymous".into(),
        }
    }
}

fn member_display(
    members: &ClanMembersStore,
    clan: ClanId,
    uid: UserId,
    cfg: Option<&AppConfig>,
) -> Option<(String, String)> {
    let member = members.member(clan, uid)?;
    let name = member.name().to_string();
    if name.is_empty() {
        return None;
    }
    let avatar = match cfg {
        Some(cfg) => cfg.avatar_proxy(member.avatar()),
        None => member.avatar().to_string(),
    };
    Some((name, avatar))
}

impl Focusable for ImageViewer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ImageViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.locale(cx);

        div()
            .track_focus(&self.focus_handle)
            .key_context("ImageViewer")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg_tertiary)
            .text_color(theme.text_primary)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .min_h_0()
                    .child(self.render_main_area(&theme, &locale, cx))
                    .when(self.show_thumbnails, |el| {
                        el.child(self.render_sidebar(&theme, cx))
                    }),
            )
            .child(self.render_bottom_bar(&theme, &locale, window, cx))
            .when_some(self.context_menu, |el, pos| {
                el.child(context_menu_at(pos, self.build_context_menu(&locale, cx)))
            })
    }
}

impl ImageViewer {
    fn render_main_area(
        &self,
        theme: &Theme,
        _locale: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let content = if self.loading && self.attachments.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element()
        } else if let Some(att) = self.current() {
            if att.is_video {
                self.render_video_placeholder(theme, att, cx)
            } else {
                self.render_image(att, cx)
            }
        } else {
            div().size_full().into_any_element()
        };

        let can_prev = self.index > 0;
        let can_next = self.index + 1 < self.attachments.len();

        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .image_cache(self.image_cache.clone())
            .child(content)
            .when(can_prev, |el| {
                el.child(nav_button(IconName::ArrowLeft, theme, true).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| this.prev(cx)),
                ))
            })
            .when(can_next, |el| {
                el.child(
                    nav_button(IconName::ChevronRight, theme, false).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| this.next(cx)),
                    ),
                )
            })
    }

    fn render_image(&self, att: &ChannelAttachment, cx: &mut Context<Self>) -> gpui::AnyElement {
        let zoom = self.zoom;
        let pan = self.pan;
        let rotated_image = self.rotated_image.clone();
        let rotation_loading = self.rotation_loading;
        let viewer_src = att.viewer_src.clone();
        let image_cache = self.image_cache.clone();

        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .when(rotation_loading, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Spinner::new()),
                )
            })
            .child(
                canvas(
                    move |_bounds, window, cx| {
                        if rotation_loading {
                            return None;
                        }
                        if let Some(image) = rotated_image {
                            return Some(image);
                        }
                        let resource =
                            Resource::Uri(SharedUri::from(viewer_src.as_ref()));
                        image_cache.update(cx, |cache, cx| {
                            ImageCache::load(cache, &resource, window, cx).and_then(|r| r.ok())
                        })
                    },
                    move |bounds, image, window, _cx| {
                        let Some(image) = image else {
                            return;
                        };
                        let raw = image.size(0);
                        let content = size(
                            px(raw.width.0 as f32),
                            px(raw.height.0 as f32),
                        );
                        let fitted = fit_contain(bounds.size, content);
                        if fitted.width <= px(0.) || fitted.height <= px(0.) {
                            return;
                        }
                        let w = px(f32::from(fitted.width) * zoom);
                        let h = px(f32::from(fitted.height) * zoom);
                        let x = bounds.origin.x + (bounds.size.width - w) / 2.0 + pan.x;
                        let y = bounds.origin.y + (bounds.size.height - h) / 2.0 + pan.y;
                        let _ = window.paint_image(
                            Bounds::from_corners(point(x, y), point(x + w, y + h)),
                            Corners::default(),
                            image,
                            0,
                            false,
                        );
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    if ev.click_count == 2 {
                        if this.zoom > MIN_ZOOM {
                            this.reset_zoom(cx);
                        } else {
                            this.set_zoom(2.0, cx);
                        }
                    } else if this.zoom > MIN_ZOOM {
                        this.drag_from = Some(ev.position);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some(from) = this.drag_from {
                    this.pan.x += ev.position.x - from.x;
                    this.pan.y += ev.position.y - from.y;
                    this.drag_from = Some(ev.position);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, _| {
                    this.drag_from = None;
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.context_menu = Some(ev.position);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .when(zoom > MIN_ZOOM, |el| el.cursor(gpui::CursorStyle::OpenHand))
            .into_any_element()
    }

    fn render_video_placeholder(
        &self,
        theme: &Theme,
        att: &ChannelAttachment,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let url = att.url.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                Icon::new(IconName::PlayButton)
                    .size(px(64.))
                    .text_color(theme.text_secondary),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .rounded(px(6.))
                    .bg(theme.brand)
                    .text_color(gpui::white())
                    .cursor_pointer()
                    .child("Open video")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, _: &MouseDownEvent, _, cx| {
                            if let Some(platform) = PlatformStore::try_global(cx) {
                                let _ = platform.read(cx).open_url_external(&url);
                            }
                        }),
                    ),
            )
            .into_any_element()
    }

    fn render_sidebar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.attachments.len();
        let entity = cx.entity();
        let active = self.index;
        let cache = self.image_cache.clone();
        let border = theme.border;
        let brand = theme.brand;

        let list = uniform_list("viewer-thumbnails", count, move |range, _window, cx| {
            let this = entity.read(cx);
            range
                .map(|ix| {
                    let Some(att) = this.attachments.get(ix) else {
                        return div().into_any_element();
                    };
                    let is_active = ix == active;
                    let src = if att.is_video {
                        att.url.clone().into()
                    } else {
                        att.thumb_src.clone()
                    };
                    div()
                        .id(("viewer-thumb", ix))
                        .h(px(THUMB_ROW_HEIGHT))
                        .w_full()
                        .p_1()
                        .child(
                            div()
                                .size_full()
                                .rounded(px(6.))
                                .overflow_hidden()
                                .border_2()
                                .border_color(if is_active { brand } else { gpui::rgba(0) })
                                .child(img(src).size_full().object_fit(gpui::ObjectFit::Cover)),
                        )
                        .on_mouse_down(MouseButton::Left, {
                            let entity = entity.clone();
                            move |_: &MouseDownEvent, _window, cx: &mut App| {
                                entity.update(cx, |this, cx| this.go_to(ix, cx));
                            }
                        })
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.list_scroll)
        .flex_1()
        .min_h_0();

        div()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(theme.bg_secondary)
            .border_l_1()
            .border_color(border)
            .image_cache(cache)
            .child(list)
    }

    fn render_bottom_bar(
        &self,
        theme: &Theme,
        locale: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let att = self.current();
        let uploader = att.map(|a| a.uploader_name.clone()).unwrap_or_default();
        let avatar = att.map(|a| a.uploader_avatar.clone()).unwrap_or_default();
        let day = att.map(|a| a.day_label.clone()).unwrap_or_default();
        let counter = if self.attachments.is_empty() {
            SharedString::default()
        } else {
            format!("{} / {}", self.index + 1, self.attachments.len()).into()
        };
        let _ = locale;

        let is_image = self.current().is_some_and(|a| a.is_image);

        let user_block = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(
                Avatar::new()
                    .name(uploader.clone())
                    .src(avatar)
                    .with_size(Size::Small)
                    .image_cache(self.image_cache.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(div().text_size(px(13.)).child(uploader))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child(day),
                    ),
            );

        div()
            .flex_shrink_0()
            .h(px(56.))
            .w_full()
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .bg(theme.bg_secondary)
            .border_t_1()
            .border_color(theme.border)
            .child(user_block)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.text_muted)
                    .child(counter),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(tool_button(IconName::Download, theme).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| this.save_image(cx)),
                    ))
                    .when(is_image, |el| {
                        el.child(tool_divider(theme))
                            .child(tool_button(IconName::RotateLeftIcon, theme).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, _, cx| this.rotate_left(cx)),
                            ))
                            .child(tool_button(IconName::RotateRightIcon, theme).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, _, cx| this.rotate_right(cx)),
                            ))
                    })
                    .child(tool_divider(theme))
                    .child(tool_button(IconName::MinusCircleIcon, theme).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.set_zoom(this.zoom - ZOOM_STEP, cx)
                        }),
                    ))
                    .child(tool_button(IconName::Plus, theme).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.set_zoom(this.zoom + ZOOM_STEP, cx)
                        }),
                    ))
                    .child(tool_button(IconName::ImageThumbnail, theme).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.show_thumbnails = !this.show_thumbnails;
                            cx.notify();
                        }),
                    )),
            )
    }

    fn build_context_menu(&self, locale: &str, cx: &Context<Self>) -> ContextMenu {
        let entity = cx.entity();
        let t = |key: &'static str| mezon_i18n::t(locale, key).to_string();
        let dismiss = {
            let entity = entity.downgrade();
            move |_window: &mut Window, cx: &mut App| {
                if let Some(this) = entity.upgrade() {
                    this.update(cx, |this, cx| {
                        this.context_menu = None;
                        cx.notify();
                    });
                }
            }
        };
        ContextMenu::new()
            .on_dismiss(dismiss)
            .item_icon(t("contextMenu.copyLink"), IconName::CopyIcon, {
                let entity = entity.downgrade();
                move |_w, cx| {
                    if let Some(this) = entity.upgrade() {
                        this.update(cx, |this, cx| {
                            this.copy_link(cx);
                            this.context_menu = None;
                            cx.notify();
                        });
                    }
                }
            })
            .item_icon(t("contextMenu.saveImage"), IconName::Download, {
                let entity = entity.downgrade();
                move |_w, cx| {
                    if let Some(this) = entity.upgrade() {
                        this.update(cx, |this, cx| {
                            this.save_image(cx);
                            this.context_menu = None;
                            cx.notify();
                        });
                    }
                }
            })
            .separator()
            .item(t("contextMenu.openLink"), {
                let entity = entity.downgrade();
                move |_w, cx| {
                    if let Some(this) = entity.upgrade() {
                        this.update(cx, |this, cx| {
                            this.open_in_browser(cx);
                            this.context_menu = None;
                            cx.notify();
                        });
                    }
                }
            })
    }
}

fn nav_button(icon: IconName, theme: &Theme, left: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(if left { "viewer-prev" } else { "viewer-next" })
        .absolute()
        .top(relative(0.5))
        .when(left, |el| el.left(px(16.)))
        .when(!left, |el| el.right(px(16.)))
        .size(px(40.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(gpui::hsla(0., 0., 0., 0.5))
        .cursor_pointer()
        .hover(|el| el.bg(gpui::hsla(0., 0., 0., 0.7)))
        .child(Icon::new(icon).size(px(20.)).text_color(theme.text_primary))
}

fn tool_button(icon: IconName, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(("viewer-tool", icon as usize))
        .size(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|el| el.bg(theme.bg_hover))
        .child(
            Icon::new(icon)
                .size(px(18.))
                .text_color(theme.text_secondary),
        )
}

fn tool_divider(theme: &Theme) -> impl IntoElement {
    div()
        .w(px(1.))
        .h(px(20.))
        .mx_1()
        .bg(theme.border)
}

async fn fetch_rotated_image(
    url: &str,
    degrees: i32,
    client: Arc<dyn HttpClient>,
) -> anyhow::Result<Arc<RenderImage>> {
    if !url.starts_with("https://") {
        anyhow::bail!("rotation fetch rejected: only https scheme is allowed");
    }
    let mut response = client.get(url, ().into(), true).await?;
    if !response.status().is_success() {
        anyhow::bail!("rotation fetch failed with status {}", response.status());
    }
    let mut bytes = Vec::new();
    response.body_mut().read_to_end(&mut bytes).await?;
    let decoded = image::load_from_memory(&bytes)?;
    let rotated = match degrees.rem_euclid(360) {
        90 => decoded.rotate90(),
        180 => decoded.rotate180(),
        270 => decoded.rotate270(),
        _ => decoded,
    };
    let mut rgba = rotated.to_rgba8();
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
}

fn fit_contain(container: GpuiSize<Pixels>, content: GpuiSize<Pixels>) -> GpuiSize<Pixels> {
    let cw = f32::from(container.width);
    let ch = f32::from(container.height);
    let iw = f32::from(content.width);
    let ih = f32::from(content.height);
    if iw <= f32::EPSILON || ih <= f32::EPSILON {
        return size(px(0.), px(0.));
    }
    let scale = (cw / iw).min(ch / ih);
    size(px(iw * scale), px(ih * scale))
}
