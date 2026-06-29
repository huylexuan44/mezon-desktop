use std::rc::Rc;

use gpui::{
    App, ClickEvent, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    Hsla, ListAlignment, ListState, MouseDownEvent, Window, div, img, list, prelude::*, px,
};
use mezon_store::{
    ChannelPermissionsStore, ClanId, ClanList, ClanMembersStore, MessagesStore, ProfileContext,
    ThreadSummary, ThreadsStore, UserId, group_threads, resolve_user_profile,
};
use ui::{PopoverMenuHandle, ScrollAxes, Scrollbars, WithScrollbar};
use ui::utils::{DateTimeType, format_distance_from_now};

use crate::chat::layout::ChatLayout;
use crate::chat::message::DEFAULT_DISPLAY_NAME_COLOR;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Input, InputState, Sizable, Size,
    Spinner, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};

const PANEL_WIDTH: f32 = 540.;
const HEADER_HEIGHT: f32 = 48.;
const PANEL_MIN_HEIGHT: f32 = 400.;
const LIST_BODY_HEIGHT: f32 = 500.;
const PANEL_MAX_VIEWPORT_OFFSET: f32 = 180.;
const THREAD_AVATAR_SIZE: f32 = 16.;

type ThreadPopoverOnOpen = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct ThreadsPopoverPanel {
    layout: Entity<ChatLayout>,
    search_input: Entity<InputState>,
    popover_handle: PopoverMenuHandle<ThreadsPopoverPanel>,
    scroll_body: Entity<ThreadsScrollBody>,
    focus_handle: FocusHandle,
}

enum ThreadRow {
    SectionHeader(String),
    Card(Box<ThreadSummary>),
    LoadingMore,
}

struct ThreadsScrollBody {
    list_state: ListState,
    layout: Entity<ChatLayout>,
    list_dirty: bool,
}

impl ThreadsScrollBody {
    fn new(layout: Entity<ChatLayout>, cx: &mut Context<Self>) -> Self {
        cx.observe(&ThreadsStore::global(cx), |this, _, cx| {
            this.list_dirty = true;
            cx.notify();
        })
        .detach();
        cx.observe(&ClanMembersStore::global(cx), |this, _, cx| {
            this.list_dirty = true;
            cx.notify();
        })
        .detach();
        cx.observe(&MessagesStore::global(cx), |this, _, cx| {
            this.list_dirty = true;
            cx.notify();
        })
        .detach();
        cx.observe(&ChannelPermissionsStore::global(cx), |_, _, cx| {
            cx.notify();
        })
        .detach();
        let list_state = ListState::new(0, ListAlignment::Top, px(400.)).measure_all();
        list_state.set_scroll_handler(move |event, _window, cx| {
            let visible_end = event.visible_range.end;
            ThreadsStore::global(cx).update(cx, |store, cx| {
                store.maybe_load_more(visible_end, cx);
            });
        });
        Self {
            list_state,
            layout,
            list_dirty: true,
        }
    }

    fn sync_list(&mut self, row_count: usize) {
        if row_count != self.list_state.item_count() {
            self.list_state.reset(row_count);
        } else if self.list_dirty {
            self.list_state.remeasure();
        }
        self.list_dirty = false;
    }
}

impl Render for ThreadsScrollBody {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.layout.read(cx).settings_language(cx);
        let layout = self.layout.clone();

        let (rows, show_spinner, show_search_no_results, search_query, clan_ids) = {
            let store = ThreadsStore::global(cx);
            let store_ref = store.read(cx);
            let threads = store_ref.threads();
            let search_results = store_ref.search_results();
            let search_query = store_ref.search_query().to_string();
            let loading = store_ref.is_loading();
            let loading_more = store_ref.is_loading_more();
            let query = search_query.trim();
            let show_search_no_results = !query.is_empty()
                && !store_ref.is_searching()
                && search_results.is_some_and(|results| results.is_empty());
            let rows = build_rows(threads, search_results, query, &locale, loading_more);
            let show_spinner = (!query.is_empty()
                && (store_ref.is_searching() || search_results.is_none()))
                || (query.is_empty() && loading && threads.is_empty());
            let displayed_threads: &[ThreadSummary] = if query.is_empty() {
                threads
            } else {
                search_results.unwrap_or(&[])
            };
            let clan_ids = clan_ids_from_threads(displayed_threads);
            (
                rows,
                show_spinner,
                show_search_no_results,
                search_query,
                clan_ids,
            )
        };

        ensure_clan_members(clan_ids, cx);

        if show_search_no_results {
            return render_search_no_results(&locale, theme.as_ref(), search_query.trim())
                .into_any_element();
        }

        if let Some(rows) = rows {
            self.sync_list(rows.len());
            let rows = Rc::new(rows);
            return div()
                .w_full()
                .h(px(LIST_BODY_HEIGHT))
                .flex_shrink_0()
                .overflow_hidden()
                .px_4()
                .pb_4()
                .bg(theme.tokens.theme_setting_primary)
                .child(
                    list(self.list_state.clone(), {
                        let theme = theme.clone();
                        let locale = locale.to_string();
                        move |ix, _window, cx| {
                            render_row(&rows[ix], &theme, &locale, layout.clone(), cx)
                        }
                    })
                    .size_full(),
                )
                .custom_scrollbars(
                    Scrollbars::new(ScrollAxes::Vertical)
                        .tracked_scroll_handle(&self.list_state),
                    _window,
                    cx,
                )
                .into_any_element();
        }

        if show_spinner {
            centered_spinner(&theme).into_any_element()
        } else if search_query.trim().is_empty() {
            let can_create = ThreadsStore::global(cx).read(cx).can_create_thread(cx);
            render_empty_body(&locale, &theme, layout, can_create).into_any_element()
        } else {
            centered_spinner(&theme).into_any_element()
        }
    }
}

impl ThreadsPopoverPanel {
    pub fn new(
        layout: Entity<ChatLayout>,
        search_input: Entity<InputState>,
        popover_handle: PopoverMenuHandle<ThreadsPopoverPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let scroll_body = cx.new(|cx| ThreadsScrollBody::new(layout.clone(), cx));

        cx.observe(&ThreadsStore::global(cx), |_, _, cx| cx.notify()).detach();
        cx.observe(&ChannelPermissionsStore::global(cx), |_, _, cx| cx.notify()).detach();

        Self {
            layout,
            search_input,
            popover_handle,
            scroll_body,
            focus_handle,
        }
    }
}

impl Focusable for ThreadsPopoverPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for ThreadsPopoverPanel {}

impl Render for ThreadsPopoverPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.layout.read(cx).settings_language(cx);
        let searching = ThreadsStore::global(cx).read(cx).is_searching();
        let can_create = ThreadsStore::global(cx).read(cx).can_create_thread(cx);
        let layout = self.layout.clone();
        let search_input = self.search_input.clone();
        let viewport_h = f32::from(window.viewport_size().height);
        let panel_max_h = (viewport_h - PANEL_MAX_VIEWPORT_OFFSET).max(PANEL_MIN_HEIGHT);
        let tokens = &theme.tokens;

        v_flex()
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .on_mouse_down_out(cx.listener(|_, _: &MouseDownEvent, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .w(px(PANEL_WIDTH))
            .min_h(px(PANEL_MIN_HEIGHT))
            .max_h(px(panel_max_h))
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(tokens.border_primary)
            .bg(tokens.theme_setting_primary)
            .text_color(tokens.text_theme_message)
            .child(
                v_flex()
                    .w_full()
                    .min_h(px(PANEL_MIN_HEIGHT))
                    .max_h(px(panel_max_h))
                    .overflow_hidden()
                    .rounded_md()
                    .child(render_header(
                        &theme,
                        &locale,
                        layout,
                        self.popover_handle.clone(),
                        search_input,
                        searching,
                        can_create,
                    ))
                    .child(self.scroll_body.clone()),
            )
    }
}

fn render_header(
    theme: &Theme,
    locale: &str,
    layout: Entity<ChatLayout>,
    _handle: PopoverMenuHandle<ThreadsPopoverPanel>,
    search_input: Entity<InputState>,
    searching: bool,
    can_create: bool,
) -> impl IntoElement {
    let tokens = &theme.tokens;
    let create_layout = layout.clone();
    let dismiss_layout = layout.clone();

    let title_block = h_flex()
        .items_center()
        .gap_4()
        .pr_4()
        .flex_shrink_0()
        .border_r_1()
        .border_color(tokens.border_primary)
        .child(
            Icon::new(IconName::ThreadIcon)
                .size_4()
                .text_color(tokens.bg_icon_theme),
        )
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tokens.text_theme_message)
                .child(mezon_i18n::t(
                    locale,
                    "channelTopbar.modals.threads.title",
                )),
        );

    let search_icon = Icon::new(if searching {
        IconName::LoadingSpinner
    } else {
        IconName::Search
    })
    .size_4()
    .text_color(tokens.text_theme_primary);

    let search_block = div()
        .relative()
        .w(px(224.))
        .flex_shrink_0()
        .child(
            h_flex()
                .w_full()
                .h(px(24.))
                .pl_4()
                .pr_2()
                .rounded_md()
                .bg(tokens.theme_input)
                .items_center()
                .child(
                    Input::new(&search_input)
                        .flex_1()
                        .text_sm()
                        .text_color(tokens.text_theme_primary),
                ),
        )
        .child(
            h_flex()
                .absolute()
                .right(px(4.))
                .top_0()
                .bottom_0()
                .items_center()
                .pl_1()
                .child(search_icon),
        );

    let close_btn = Button::new("thread-close-btn")
        .icon(
            Icon::new(IconName::Close)
                .size_4()
                .text_color(tokens.text_theme_primary_hover),
        )
        .ghost()
        .with_size(Size::Small)
        .on_click(move |_: &ClickEvent, _window, cx| {
            dismiss_layout.update(cx, |layout, cx| layout.dismiss_threads_popover(cx));
        });

    let mut actions = h_flex().items_center().gap_4().flex_shrink_0();
    if can_create {
        let create_btn = Button::new("thread-create-btn")
            .label(mezon_i18n::t(locale, "channelTopbar.modals.threads.create"))
            .primary()
            .with_size(Size::Small)
            .on_click(move |_: &ClickEvent, _window, cx| {
                create_layout.update(cx, |layout, cx| layout.open_create_thread(cx));
            });
        actions = actions.child(create_btn);
    }
    actions = actions.child(close_btn);

    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .px_4()
        .h(px(HEADER_HEIGHT))
        .border_b_1()
        .border_color(tokens.border_primary)
        .bg(tokens.theme_setting_nav)
        .child(title_block)
        .child(search_block)
        .child(actions)
}

fn centered_spinner(theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w_full()
        .min_h(px(PANEL_MIN_HEIGHT))
        .bg(theme.tokens.theme_setting_primary)
        .child(Spinner::new().with_size(Size::Small))
}

fn build_rows(
    threads: &[ThreadSummary],
    search_results: Option<&[ThreadSummary]>,
    query: &str,
    locale: &str,
    loading_more: bool,
) -> Option<Vec<ThreadRow>> {
    if !query.is_empty() {
        let results = search_results?;
        let title = format!(
            "{} ({})",
            mezon_i18n::t(locale, "channelTopbar.modals.threads.results"),
            results.len()
        );
        let mut rows = Vec::with_capacity(results.len() + 1);
        rows.push(ThreadRow::SectionHeader(title));
        rows.extend(results.iter().cloned().map(|t| ThreadRow::Card(Box::new(t))));
        return Some(rows);
    }

    if threads.is_empty() {
        return None;
    }

    let (joined, active, older) = group_threads(threads);
    let mut rows = Vec::with_capacity(threads.len() + 3);

    for (key, bucket) in [
        ("channelTopbar.joinedThreads", joined),
        ("channelTopbar.activeThreads", active),
        ("channelTopbar.archivedThreads", older),
    ] {
        if bucket.is_empty() {
            continue;
        }
        rows.push(ThreadRow::SectionHeader(format!(
            "{} ({})",
            mezon_i18n::t(locale, key).to_uppercase(),
            bucket.len()
        )));
        rows.extend(bucket.into_iter().cloned().map(|t| ThreadRow::Card(Box::new(t))));
    }

    if loading_more {
        rows.push(ThreadRow::LoadingMore);
    }

    if rows.is_empty() { None } else { Some(rows) }
}

fn render_search_no_results(locale: &str, theme: &Theme, query: &str) -> impl IntoElement {
    let tokens = &theme.tokens;
    let title = mezon_i18n::t(locale, "searchMessageChannel.noResults");
    let description = mezon_i18n::t(locale, "searchMessageChannel.emptySearch.title");
    let search_for = format!(
        "{} {}",
        mezon_i18n::t(locale, "searchMessageChannel.searchFor"),
        query
    );

    v_flex()
        .w_full()
        .h(px(LIST_BODY_HEIGHT))
        .items_center()
        .justify_center()
        .gap_4()
        .p_8()
        .bg(tokens.theme_setting_primary)
        .child(
            Icon::new(IconName::Search)
                .size(px(48.))
                .text_color(theme.text_muted),
        )
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tokens.text_theme_message)
                .text_center()
                .child(title),
        )
        .child(
            div()
                .text_sm()
                .text_center()
                .text_color(tokens.text_theme_message)
                .max_w(px(280.))
                .child(description),
        )
        .child(
            div()
                .text_xs()
                .text_center()
                .text_color(tokens.text_theme_message)
                .max_w(px(280.))
                .child(search_for),
        )
}

fn render_empty_body(
    locale: &str,
    theme: &Theme,
    layout: Entity<ChatLayout>,
    can_create: bool,
) -> impl IntoElement {
    let tokens = &theme.tokens;
    let create_layout = layout;

    let mut body = v_flex()
        .w_full()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_4()
        .p_12()
        .min_h(px(PANEL_MIN_HEIGHT))
        .bg(tokens.theme_setting_primary)
        .child(
            div()
                .relative()
                .mx_auto()
                .mb_4()
                .p(px(22.))
                .rounded_full()
                .bg(tokens.bg_active_member_channel)
                .child(img("icons/thread-empty.svg").size(px(36.)).flex_none())
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(-10.))
                        .child(
                            img("icons/empty-unread-style.svg")
                                .w(px(104.))
                                .h(px(80.))
                                .flex_none(),
                        ),
                ),
        )
        .child(
            div()
                .text_2xl()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tokens.text_theme_message)
                .child(mezon_i18n::t(locale, "channelTopbar.threads.emptyTitle")),
        )
        .child(
            div()
                .text_base()
                .text_center()
                .text_color(tokens.text_theme_message)
                .child(mezon_i18n::t(
                    locale,
                    "channelTopbar.threads.emptyDescription",
                )),
        );

    if can_create {
        body = body.child(
            Button::new("thread-empty-create")
                .label(mezon_i18n::t(locale, "channelTopbar.threads.createThread"))
                .primary()
                .on_click(move |_: &ClickEvent, _window, cx| {
                    create_layout.update(cx, |layout, cx| layout.open_create_thread(cx));
                }),
        );
    }

    body
}

fn render_row(
    row: &ThreadRow,
    theme: &Theme,
    locale: &str,
    layout: Entity<ChatLayout>,
    cx: &App,
) -> gpui::AnyElement {
    match row {
        ThreadRow::SectionHeader(title) => section_header(title, theme),
        ThreadRow::Card(thread) => div()
            .w_full()
            .px_4()
            .child(thread_card(thread.as_ref(), theme, locale, layout, cx))
            .into_any_element(),
        ThreadRow::LoadingMore => div()
            .w_full()
            .py_3()
            .flex()
            .items_center()
            .justify_center()
            .child(Spinner::new().with_size(Size::Small))
            .into_any_element(),
    }
}

fn section_header(title: &str, theme: &Theme) -> gpui::AnyElement {
    let tokens = &theme.tokens;
    div()
        .w_full()
        .px_4()
        .pt_2()
        .pb_1()
        .child(
            div()
                .h(px(24.))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tokens.text_secondary)
                .child(title.to_string()),
        )
        .into_any_element()
}

fn thread_card(
    thread: &ThreadSummary,
    theme: &Theme,
    locale: &str,
    layout: Entity<ChatLayout>,
    cx: &App,
) -> gpui::AnyElement {
    let tokens = &theme.tokens;
    let channel_id = thread.channel_id.clone();
    let clan_id = thread.clan_id.clone();
    let preview = resolve_thread_preview(thread, cx);
    let (sender_label, avatar_url) = resolve_thread_sender(thread, &preview, cx);
    let time_label = format_thread_time(preview.timestamp, locale);
    let sender_color = Hsla::from(gpui::rgb(DEFAULT_DISPLAY_NAME_COLOR));

    let mut avatar = Avatar::new()
        .name(&sender_label)
        .size_px(px(THREAD_AVATAR_SIZE));
    if !avatar_url.is_empty() {
        let proxied = crate::util::imgproxy::avatar_url(cx, &avatar_url);
        avatar = avatar.src(proxied).fallback_src(avatar_url);
    }

    h_flex()
        .id(format!(
            "thread-item-{}-{}-{}",
            thread.channel_id, preview.timestamp, preview.sender_id_raw
        ))
        .w_full()
        .h(px(72.))
        .mb_2()
        .px_4()
        .items_center()
        .gap_3()
        .rounded_lg()
        .cursor_pointer()
        .bg(tokens.bg_active_member_channel)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(tokens.text_theme_message)
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(thread.channel_label.clone()),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .child(avatar)
                        .when(!sender_label.is_empty(), |this| {
                            this.child(
                                div()
                                    .max_w(px(150.))
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(sender_color)
                                    .flex_shrink_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(format!("{sender_label}:")),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .text_color(tokens.text_theme_message)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(preview.content),
                        )
                        .when(!time_label.is_empty(), |this| {
                            this.child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(tokens.text_theme_primary)
                                    .child(format!("• {time_label}")),
                            )
                        }),
                ),
        )
        .on_click(move |_: &ClickEvent, _window, cx| {
            layout.update(cx, |layout, cx| {
                layout.navigate_to_thread(&channel_id, &clan_id, cx);
            });
        })
        .into_any_element()
}

struct ThreadCardPreview {
    content: String,
    sender_id_raw: String,
    sender_name: String,
    sender_avatar: String,
    timestamp: i64,
}

fn resolve_thread_preview(thread: &ThreadSummary, cx: &App) -> ThreadCardPreview {
    if let Some(message) = MessagesStore::global(cx)
        .read(cx)
        .last_cached_message(&thread.channel_id)
    {
        return ThreadCardPreview {
            content: message.content.clone(),
            sender_id_raw: message.sender_id.clone(),
            sender_name: message.sender_name.clone(),
            sender_avatar: message.avatar_url.clone(),
            timestamp: message.create_time,
        };
    }

    let sender_id_raw = if !thread.last_message_sender_id.is_empty() {
        thread.last_message_sender_id.clone()
    } else {
        thread.creator_id.clone()
    };

    ThreadCardPreview {
        content: thread.last_message_content.clone(),
        sender_id_raw,
        sender_name: thread.last_message_sender_name.clone(),
        sender_avatar: thread.last_message_sender_avatar.clone(),
        timestamp: thread.last_sent_timestamp,
    }
}

fn resolve_thread_sender(
    thread: &ThreadSummary,
    preview: &ThreadCardPreview,
    cx: &App,
) -> (String, String) {
    let clan_id = thread
        .clan_id
        .parse::<ClanId>()
        .ok()
        .or_else(|| {
            ThreadsStore::global(cx)
                .read(cx)
                .list_clan_id()
                .and_then(|id| id.parse::<ClanId>().ok())
        });

    if let (Some(user_id), Some(clan_id)) = (
        preview.sender_id_raw.parse::<UserId>().ok(),
        clan_id,
    ) && let Some(profile) = resolve_user_profile(user_id, ProfileContext::Clan(clan_id), cx)
    {
        return (profile.display_name, profile.avatar_url);
    }

    if !preview.sender_name.is_empty() {
        return (
            preview.sender_name.clone(),
            preview.sender_avatar.clone(),
        );
    }

    (String::new(), String::new())
}

fn format_thread_time(timestamp_sec: i64, locale: &str) -> String {
    if timestamp_sec <= 0 {
        return String::new();
    }
    let now = chrono::Local::now().timestamp();
    let diff = now.saturating_sub(timestamp_sec);
    if diff <= 1 {
        return mezon_i18n::t(locale, "common.justNow").to_string();
    }
    let Some(utc) = chrono::DateTime::from_timestamp(timestamp_sec, 0) else {
        return String::new();
    };
    let naive = utc.with_timezone(&chrono::Local).naive_local();
    format_distance_from_now(DateTimeType::Naive(naive), false, true, false)
}

pub fn thread_popover_on_open(layout: Entity<ChatLayout>) -> ThreadPopoverOnOpen {
    Rc::new(move |window, cx| {
        layout.update(cx, |layout, cx| {
            layout.ensure_thread_search_input(window, cx);
        });
        ThreadsStore::global(cx).update(cx, |store, cx| {
            store.open_popover(cx);
        });
        if let Some(clan_id_str) = ThreadsStore::global(cx)
            .read(cx)
            .list_clan_id()
            .map(str::to_string)
            .or_else(|| {
                ClanList::global(cx)
                    .read(cx)
                    .active_clan_id
                    .map(|id| id.to_string())
            })
            && let Ok(clan_id) = clan_id_str.parse::<ClanId>()
        {
            ClanMembersStore::global(cx).update(cx, |store, cx| {
                store.ensure_loaded(clan_id, cx);
                cx.notify();
            });
        }
    })
}

fn clan_ids_from_threads(threads: &[ThreadSummary]) -> Vec<ClanId> {
    let mut seen = std::collections::HashSet::new();
    let mut ids = Vec::new();
    for thread in threads {
        let Ok(clan_id) = thread.clan_id.parse::<ClanId>() else {
            continue;
        };
        if seen.insert(clan_id) {
            ids.push(clan_id);
        }
    }
    ids
}

fn ensure_clan_members(clan_ids: Vec<ClanId>, cx: &mut App) {
    for clan_id in clan_ids {
        ClanMembersStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, cx);
        });
    }
}
