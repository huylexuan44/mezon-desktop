use chrono::NaiveDate;
use gpui::{
    App, AppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    ListAlignment, ListState, MouseButton, MouseDownEvent, Render, SharedString, Subscription,
    Window, div, img, list, prelude::*, px,
};
use mezon_store::{
    AppConfig, ChannelAttachment, ChannelId, ClanId, ClanMembersStore, GalleryStore, LoadDirection,
    MediaFilter, Settings, UploaderInfo, UserId, enrich_uploader,
};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use crate::components::primitives::input::{Input, InputState};
use crate::components::primitives::{Icon, IconName};
use crate::image_cache::LruImageCache;
use crate::image_viewer::{OpenViewerRequest, open_image_viewer};
use crate::theme::{ActiveTheme, Theme};

const TILE: f32 = 144.0;
const COLUMNS: usize = 3;
const LOAD_MORE_THRESHOLD: usize = 4;
const DATE_FMT: &str = "%d/%m/%Y";
const DATE_FILTER_TOP: f32 = 92.0;

enum GalleryRow {
    Header(SharedString),
    Images(Vec<ChannelAttachment>),
}

pub struct GalleryModal {
    focus_handle: FocusHandle,
    clan_id: ClanId,
    channel_id: ChannelId,
    channel_label: SharedString,
    settings: Entity<Settings>,
    active_filter: MediaFilter,
    from_date_input: Entity<InputState>,
    to_date_input: Entity<InputState>,
    date_validation_error: Option<String>,
    applied_from_date: Option<NaiveDate>,
    applied_to_date: Option<NaiveDate>,
    date_filter_open: bool,
    rows: Vec<GalleryRow>,
    list_state: ListState,
    image_cache: Entity<LruImageCache>,
    _subscription: Subscription,
    _release: Subscription,
}

impl GalleryModal {
    pub(crate) fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        channel_label: SharedString,
        settings: Entity<Settings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let image_cache = cx.new(|cx| LruImageCache::new(256, 128 * 1024 * 1024, cx));
        let gallery = GalleryStore::global(cx);
        let subscription = cx.observe(&gallery, |this, _, cx| {
            this.rebuild_rows(cx);
        });
        let release = cx.on_release(move |_, cx| {
            if let Some(store) = GalleryStore::try_global(cx) {
                store.update(cx, |store, _| {
                    store.reset_channel_attachments(channel_id);
                });
            }
        });
        let from_date_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("dd/MM/yyyy")
                .height(px(36.))
                .radius(px(6.))
                .bg(cx.theme().bg_secondary)
                .borderless()
        });
        let to_date_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("dd/MM/yyyy")
                .height(px(36.))
                .radius(px(6.))
                .bg(cx.theme().bg_secondary)
                .borderless()
        });
        let mut this = Self {
            focus_handle,
            clan_id,
            channel_id,
            channel_label,
            settings,
            active_filter: MediaFilter::All,
            from_date_input,
            to_date_input,
            date_validation_error: None,
            applied_from_date: None,
            applied_to_date: None,
            date_filter_open: false,
            rows: Vec::new(),
            list_state: ListState::new(0, ListAlignment::Top, px(400.)),
            image_cache,
            _subscription: subscription,
            _release: release,
        };
        this.install_scroll_handler(cx);
        this.rebuild_rows(cx);
        gallery.update(cx, |store, cx| {
            store.ensure_loaded(clan_id, channel_id, cx);
        });
        this
    }

    fn install_scroll_handler(&self, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        let clan = self.clan_id;
        let channel = self.channel_id;
        self.list_state
            .set_scroll_handler(move |event, _window, cx| {
                let near_bottom =
                    event.visible_range.end + LOAD_MORE_THRESHOLD >= event.count && event.count > 0;
                if near_bottom && let Some(this) = weak.upgrade() {
                    this.update(cx, |_, cx| {
                        GalleryStore::global(cx).update(cx, |store, cx| {
                            store.fetch_page(clan, channel, LoadDirection::Before, cx);
                        });
                    });
                }
            });
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let store = GalleryStore::global(cx);
        let filtered = store.read(cx).filtered(self.channel_id, self.active_filter);
        let mut rows: Vec<GalleryRow> = Vec::new();
        let mut current_day: Option<i64> = None;
        let mut bucket: Vec<ChannelAttachment> = Vec::new();
        for att in filtered {
            if current_day != Some(att.day_index) {
                flush_bucket(&mut rows, &mut bucket);
                rows.push(GalleryRow::Header(att.day_label.clone()));
                current_day = Some(att.day_index);
            }
            bucket.push(att);
            if bucket.len() == COLUMNS {
                rows.push(GalleryRow::Images(std::mem::take(&mut bucket)));
            }
        }
        flush_bucket(&mut rows, &mut bucket);
        self.rows = rows;
        self.list_state.reset(self.rows.len());
        cx.notify();
    }

    fn set_filter(&mut self, filter: MediaFilter, cx: &mut Context<Self>) {
        self.close_date_filter_panel(cx);
        if self.active_filter != filter {
            self.active_filter = filter;
            self.rebuild_rows(cx);
        }
    }

    fn parse_draft_from(&self, cx: &App) -> Option<NaiveDate> {
        parse_date_text(self.from_date_input.read(cx).value())
    }

    fn parse_draft_to(&self, cx: &App) -> Option<NaiveDate> {
        parse_date_text(self.to_date_input.read(cx).value())
    }

    fn validate_dates(&mut self, cx: &mut Context<Self>) -> bool {
        let from = self.parse_draft_from(cx);
        let to = self.parse_draft_to(cx);
        self.date_validation_error = match (from, to) {
            (Some(start), Some(end)) if start > end => Some(
                mezon_i18n::t(
                    &self.locale(cx),
                    "channelTopbar.gallery.validation.startDateBeforeEnd",
                )
                .to_string(),
            ),
            _ => None,
        };
        self.date_validation_error.is_none()
    }

    fn apply_date_filter(&mut self, cx: &mut Context<Self>) {
        if !self.validate_dates(cx) {
            cx.notify();
            return;
        }
        let from = self.parse_draft_from(cx);
        let to = self.parse_draft_to(cx);
        if from.is_none() && to.is_none() {
            return;
        }
        let (after, before) = calculate_timestamps(from, to);
        self.applied_from_date = from;
        self.applied_to_date = to;
        GalleryStore::global(cx).update(cx, |store, cx| {
            store.apply_date_filter(self.clan_id, self.channel_id, after, before, cx);
        });
        self.date_filter_open = false;
        cx.notify();
    }

    fn clear_date_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.applied_from_date = None;
        self.applied_to_date = None;
        self.date_validation_error = None;
        self.date_filter_open = false;
        self.from_date_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.to_date_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        GalleryStore::global(cx).update(cx, |store, cx| {
            store.clear_date_filter(self.clan_id, self.channel_id, cx);
        });
        cx.notify();
    }

    fn toggle_date_filter(&mut self, cx: &mut Context<Self>) {
        self.date_filter_open = !self.date_filter_open;
        cx.notify();
    }

    fn close_date_filter_panel(&mut self, cx: &mut Context<Self>) {
        if self.date_filter_open {
            self.date_filter_open = false;
            cx.notify();
        }
    }

    fn date_range_label(&self, locale: &str) -> String {
        match (self.applied_from_date, self.applied_to_date) {
            (None, None) => mezon_i18n::t(locale, "channelTopbar.gallery.sentDate").to_string(),
            (Some(start), None) => mezon_i18n::t(
                locale,
                "channelTopbar.gallery.dateRange.from",
            )
            .replace("{{date}}", &format_date_label(start))
            .to_string(),
            (None, Some(end)) => mezon_i18n::t(locale, "channelTopbar.gallery.dateRange.to")
                .replace("{{date}}", &format_date_label(end))
                .to_string(),
            (Some(start), Some(end)) if start == end => format_date_label(start),
            (Some(start), Some(end)) => mezon_i18n::t(
                locale,
                "channelTopbar.gallery.dateRange.range",
            )
            .replace("{{startDate}}", &format_date_label(start))
            .replace("{{endDate}}", &format_date_label(end))
            .to_string(),
        }
    }

    fn has_date_filter(&self) -> bool {
        self.applied_from_date.is_some() || self.applied_to_date.is_some()
    }

    fn open_attachment(&mut self, attachment_id: i64, cx: &mut Context<Self>) {
        let store = GalleryStore::global(cx);
        let mut playlist = store.read(cx).filtered(self.channel_id, self.active_filter);
        let Some(index) = playlist.iter().position(|a| a.id == attachment_id) else {
            return;
        };
        enrich_playlist(&mut playlist, self.clan_id, cx);
        open_image_viewer(
            OpenViewerRequest {
                clan_id: self.clan_id,
                channel_id: self.channel_id,
                channel_label: self.channel_label.clone(),
                settings: self.settings.clone(),
                attachments: playlist,
                selected_index: index,
                selected_url: None,
                anchor_before: None,
            },
            cx,
        );
        self.dismiss(cx);
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        GalleryStore::global(cx).update(cx, |store, _| {
            store.reset_channel_attachments(self.channel_id);
        });
        cx.emit(DismissEvent);
    }

    fn locale(&self, cx: &App) -> String {
        self.settings.read(cx).language.clone()
    }
}

fn flush_bucket(rows: &mut Vec<GalleryRow>, bucket: &mut Vec<ChannelAttachment>) {
    if !bucket.is_empty() {
        rows.push(GalleryRow::Images(std::mem::take(bucket)));
    }
}

fn enrich_playlist(playlist: &mut [ChannelAttachment], clan: ClanId, cx: &App) {
    let Some(members) = ClanMembersStore::try_global(cx) else {
        return;
    };
    let cfg = AppConfig::try_global(cx);
    let members = members.read(cx);
    enrich_uploader(playlist, |uid: UserId| {
        members.member(clan, uid).map(|m| UploaderInfo {
            name: m.name().to_string(),
            avatar: match cfg {
                Some(cfg) => cfg.avatar_proxy(m.avatar()),
                None => m.avatar().to_string(),
            },
        })
    });
}

impl Focusable for GalleryModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for GalleryModal {}

impl Render for GalleryModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.locale(cx);
        let t = |key: &'static str| mezon_i18n::t(&locale, key).to_string();

        let viewport_h = f32::from(window.viewport_size().height);
        let panel_h = px((viewport_h * 0.8).clamp(400.0, (viewport_h - 96.0).max(400.0)));

        let entity = cx.entity();
        let entity_body = entity.clone();
        let image_cache = self.image_cache.clone();
        let theme_for_list = theme.clone();
        let body = if self.rows.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.text_muted)
                .child(empty_label(
                    self.active_filter,
                    self.has_date_filter(),
                    &locale,
                ))
                .into_any_element()
        } else {
            div()
                .size_full()
                .relative()
                .overflow_hidden()
                .image_cache(image_cache)
                .child(
                    list(self.list_state.clone(), move |ix, _window, cx| {
                        let this = entity.read(cx);
                        match this.rows.get(ix) {
                            Some(GalleryRow::Header(label)) => {
                                render_header(label, &theme_for_list)
                            }
                            Some(GalleryRow::Images(atts)) => {
                                render_image_row(atts, &theme_for_list, &entity, cx)
                            }
                            None => div().into_any_element(),
                        }
                    })
                    .flex_1()
                    .size_full(),
                )
                .custom_scrollbars(
                    Scrollbars::new(ScrollAxes::Vertical)
                        .tracked_scroll_handle(&self.list_state),
                    window,
                    cx,
                )
                .into_any_element()
        };

        div()
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| {
                if this.date_filter_open {
                    this.close_date_filter_panel(cx);
                } else {
                    this.dismiss(cx);
                }
            }))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                this.dismiss(cx);
            }))
            .occlude()
            .relative()
            .w(px(480.))
            .h(panel_h)
            .flex()
            .flex_col()
            .shadow_lg()
            .rounded(px(8.))
            .bg(theme.bg_primary)
            .border_1()
            .border_color(theme.border)
            .child(render_modal_header(
                &t("channelTopbar.gallery.title"),
                &theme,
                &cx.entity(),
            ))
            .child(render_filter_tabs(
                self.active_filter,
                &theme,
                &locale,
                self.date_range_label(&locale),
                self.date_filter_open,
                &cx.entity(),
            ))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .px_3()
                    .pb_3()
                    .on_mouse_down(
                        MouseButton::Left,
                        move |_: &MouseDownEvent, _window, cx: &mut App| {
                            entity_body.update(cx, |this, cx| this.close_date_filter_panel(cx));
                        },
                    )
                    .child(body),
            )
            .when(self.date_filter_open, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(DATE_FILTER_TOP))
                        .right(px(16.))
                        .child(render_date_filter_panel(
                            &theme,
                            &locale,
                            self.from_date_input.clone(),
                            self.to_date_input.clone(),
                            self.date_validation_error.clone(),
                            &cx.entity(),
                        )),
                )
            })
    }
}

fn render_modal_header(
    title: &str,
    theme: &Theme,
    entity: &Entity<GalleryModal>,
) -> impl IntoElement {
    let entity = entity.clone();
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px_4()
        .py_3()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_4()
                .child(
                    Icon::new(IconName::ImageThumbnail)
                        .size(px(20.))
                        .text_color(theme.text_primary),
                )
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(title.to_string()),
                ),
        )
        .child(
            div()
                .id("gallery-close")
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|el| el.bg(theme.bg_hover))
                .child(
                    Icon::new(IconName::Close)
                        .size(px(16.))
                        .text_color(theme.text_secondary),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    move |_: &MouseDownEvent, _window, cx: &mut App| {
                        entity.update(cx, |this, cx| this.dismiss(cx));
                    },
                ),
        )
}

fn render_filter_tabs(
    active: MediaFilter,
    theme: &Theme,
    locale: &str,
    date_label: String,
    date_filter_open: bool,
    entity: &Entity<GalleryModal>,
) -> impl IntoElement {
    let entity_toggle = entity.clone();
    let text_color = if date_filter_open {
        theme.text_primary
    } else {
        theme.text_secondary
    };
    let mut chevron = Icon::new(IconName::ChevronDown)
        .size(px(12.))
        .text_color(text_color);
    if date_filter_open {
        chevron = chevron.with_transformation(gpui::Transformation::rotate(gpui::radians(
            std::f32::consts::PI,
        )));
    }
    let date_trigger = div()
        .id("gallery-date-filter-trigger")
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py_1()
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(13.))
        .text_color(text_color)
        .hover(move |s| s.bg(theme.bg_hover))
        .when(date_filter_open, |el| el.bg(theme.bg_secondary))
        .child(date_label)
        .child(chevron)
        .on_mouse_down(
            MouseButton::Left,
            move |_: &MouseDownEvent, _window, cx: &mut App| {
                entity_toggle.update(cx, |this, cx| this.toggle_date_filter(cx));
            },
        );

    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px_4()
        .py_2()
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(filter_tab(
                    MediaFilter::All,
                    active,
                    mezon_i18n::t(locale, "channelTopbar.gallery.filters.all").to_string(),
                    theme,
                    entity,
                ))
                .child(filter_tab(
                    MediaFilter::Image,
                    active,
                    mezon_i18n::t(locale, "channelTopbar.gallery.filters.images").to_string(),
                    theme,
                    entity,
                ))
                .child(filter_tab(
                    MediaFilter::Video,
                    active,
                    mezon_i18n::t(locale, "channelTopbar.gallery.filters.videos").to_string(),
                    theme,
                    entity,
                )),
        )
        .child(
            div()
                .relative()
                .child(date_trigger),
        )
}

fn render_date_filter_panel(
    theme: &Theme,
    locale: &str,
    from_input: Entity<InputState>,
    to_input: Entity<InputState>,
    validation_error: Option<String>,
    entity: &Entity<GalleryModal>,
) -> impl IntoElement {
    let has_error = validation_error.is_some();
    let entity_clear = entity.clone();
    let entity_apply = entity.clone();

    div()
        .id("gallery-date-filter-panel")
        .occlude()
        .w(px(300.))
        .p_4()
        .flex()
        .flex_col()
        .gap_4()
        .rounded(px(8.))
        .bg(theme.bg_primary)
        .border_1()
        .border_color(theme.border)
        .shadow_lg()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.text_muted)
                        .child(mezon_i18n::t(locale, "channelTopbar.gallery.fromDate")),
                )
                .child(div().w_full().child(Input::new(&from_input))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.text_muted)
                        .child(mezon_i18n::t(locale, "channelTopbar.gallery.toDate")),
                )
                .child(div().w_full().child(Input::new(&to_input))),
        )
        .when_some(validation_error, |el, error| {
            el.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.status_dnd)
                    .child(error),
            )
        })
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|el| el.text_color(theme.text_primary))
                        .child(mezon_i18n::t(
                            locale,
                            "channelTopbar.gallery.buttons.clearAll",
                        ))
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_: &MouseDownEvent, window, cx: &mut App| {
                                entity_clear.update(cx, |this, cx| {
                                    this.clear_date_filter(window, cx);
                                });
                            },
                        ),
                )
                .child(
                    div()
                        .px_3()
                        .py_1()
                        .rounded(px(6.))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .when(!has_error, |el| {
                            el.bg(theme.brand).text_color(gpui::white())
                        })
                        .when(has_error, |el| {
                            el.bg(theme.bg_tertiary).text_color(theme.text_muted)
                        })
                        .child(mezon_i18n::t(
                            locale,
                            "channelTopbar.gallery.buttons.apply",
                        ))
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_: &MouseDownEvent, _window, cx: &mut App| {
                                entity_apply.update(cx, |this, cx| {
                                    this.apply_date_filter(cx);
                                });
                            },
                        ),
                ),
        )
}

fn filter_tab(
    filter: MediaFilter,
    active: MediaFilter,
    label: String,
    theme: &Theme,
    entity: &Entity<GalleryModal>,
) -> impl IntoElement {
    let is_active = filter == active;
    let entity = entity.clone();
    div()
        .id(SharedString::from(format!("gallery-tab-{label}")))
        .px_3()
        .py_1()
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(13.))
        .when(is_active, |el| el.bg(theme.brand).text_color(gpui::white()))
        .when(!is_active, |el| {
            el.bg(theme.bg_secondary).text_color(theme.text_secondary)
        })
        .child(label)
        .on_mouse_down(
            MouseButton::Left,
            move |_: &MouseDownEvent, _window, cx: &mut App| {
                entity.update(cx, |this, cx| this.set_filter(filter, cx));
            },
        )
}

fn render_header(label: &SharedString, theme: &Theme) -> gpui::AnyElement {
    div()
        .w_full()
        .pt_3()
        .pb_1()
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.text_muted)
        .child(label.clone())
        .into_any_element()
}

fn render_image_row(
    atts: &[ChannelAttachment],
    theme: &Theme,
    entity: &Entity<GalleryModal>,
    _cx: &App,
) -> gpui::AnyElement {
    let mut row = div().flex().flex_row().gap_3().pb_3();
    for att in atts {
        let id = att.id;
        let entity = entity.clone();
        let src = if att.is_video {
            SharedString::from(att.url.clone())
        } else {
            att.thumb_src.clone()
        };
        let is_video = att.is_video;
        row = row.child(
            div()
                .id(("gallery-tile", id as usize))
                .size(px(TILE))
                .rounded(px(6.))
                .overflow_hidden()
                .bg(theme.bg_tertiary)
                .cursor_pointer()
                .relative()
                .child(img(src).size_full().object_fit(gpui::ObjectFit::Cover))
                .when(is_video, |el| {
                    el.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::PlayButton)
                                    .size(px(32.))
                                    .text_color(gpui::white()),
                            ),
                    )
                })
                .on_mouse_down(
                    MouseButton::Left,
                    move |_: &MouseDownEvent, _window, cx: &mut App| {
                        entity.update(cx, |this, cx| this.open_attachment(id, cx));
                    },
                ),
        );
    }
    row.into_any_element()
}

fn empty_label(filter: MediaFilter, date_filtered: bool, locale: &str) -> String {
    let key = if date_filtered {
        "channelTopbar.gallery.emptyState.noMediaFilesDateRange"
    } else {
        match filter {
            MediaFilter::All => "channelTopbar.gallery.emptyState.noMediaFiles",
            MediaFilter::Image => "channelTopbar.gallery.emptyState.noImages",
            MediaFilter::Video => "channelTopbar.gallery.emptyState.noVideos",
        }
    };
    mezon_i18n::t(locale, key).to_string()
}

fn parse_date_text(text: &str) -> Option<NaiveDate> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    NaiveDate::parse_from_str(trimmed, DATE_FMT).ok()
}

fn format_date_label(date: NaiveDate) -> String {
    date.format(DATE_FMT).to_string()
}

fn start_of_day_ts(date: NaiveDate) -> u32 {
    date.and_hms_opt(0, 0, 0)
        .and_then(|dt| dt.and_utc().timestamp().try_into().ok())
        .unwrap_or(0)
}

fn end_of_day_ts(date: NaiveDate) -> u32 {
    date.and_hms_opt(23, 59, 59)
        .and_then(|dt| dt.and_utc().timestamp().try_into().ok())
        .unwrap_or(0)
}

fn calculate_timestamps(
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> (Option<u32>, Option<u32>) {
    match (from, to) {
        (Some(start), Some(end)) if start == end => {
            (Some(start_of_day_ts(start)), Some(end_of_day_ts(start)))
        }
        (from, to) => (from.map(start_of_day_ts), to.map(end_of_day_ts)),
    }
}
