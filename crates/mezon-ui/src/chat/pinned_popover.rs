use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, ClickEvent, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    Hsla, ListAlignment, ListState, MouseDownEvent, SharedString, Window, div, list, prelude::*, px,
};
use mezon_store::{
    ClanMembersStore, MessageId, MessagesStore, PinnedMessage, PinnedMessagesStore, ProfileContext,
    Settings, UserId, resolve_user_profile,
};
use ui::{PopoverMenuHandle, ScrollAxes, Scrollbars, WithScrollbar};

use crate::chat::message::DEFAULT_DISPLAY_NAME_COLOR;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Sizable, Size, Spinner, h_flex, v_flex,
};
use crate::image_cache::{
    AVATAR_ENTRY_MAX_BYTES, AVATAR_IMAGE_CACHE_BYTES, AVATAR_IMAGE_CACHE_CAPACITY, LruImageCache,
};
use crate::theme::{ActiveTheme, Theme};

const POPOVER_WIDTH: f32 = 420.;
const HEADER_HEIGHT: f32 = 48.;
const MIN_BODY_HEIGHT: f32 = 144.;
const LIST_BODY_HEIGHT: f32 = 520.;
const LIST_OVERDRAW: f32 = 200.;
const MAX_VH: f32 = 0.8;

pub struct PinnedPopoverPanel {
    settings: Entity<Settings>,
    popover_handle: PopoverMenuHandle<PinnedPopoverPanel>,
    list_state: ListState,
    focus_handle: FocusHandle,
    avatar_image_cache: Entity<LruImageCache>,
}

impl PinnedPopoverPanel {
    pub fn new(
        settings: Entity<Settings>,
        popover_handle: PopoverMenuHandle<PinnedPopoverPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_blur(&focus_handle, window, |_, _, cx| cx.emit(DismissEvent))
            .detach();

        cx.observe(&PinnedMessagesStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        cx.observe(&ClanMembersStore::global(cx), |_, _, cx| cx.notify())
            .detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();

        let avatar_image_cache = cx.new(|cx| {
            LruImageCache::avatar_thumbnail(
                "pin-avatar",
                AVATAR_IMAGE_CACHE_CAPACITY,
                AVATAR_IMAGE_CACHE_BYTES,
                AVATAR_ENTRY_MAX_BYTES,
                cx,
            )
        });

        let list_state = ListState::new(0, ListAlignment::Top, px(LIST_OVERDRAW)).measure_all();

        Self {
            settings,
            popover_handle,
            list_state,
            focus_handle,
            avatar_image_cache,
        }
    }
}

impl Focusable for PinnedPopoverPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for PinnedPopoverPanel {}

impl Render for PinnedPopoverPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let store = PinnedMessagesStore::global(cx);
        let pinned = Rc::new(store.read(cx).pinned().to_vec());
        let loading = store.read(cx).is_loading();
        let clan_id = store.read(cx).clan_id();
        let handle = self.popover_handle.clone();
        let avatar_cache = self.avatar_image_cache.clone();
        let tokens = &theme.tokens;

        if let Some(clan_id) = clan_id {
            ClanMembersStore::global(cx).update(cx, |members, cx| {
                members.ensure_loaded(clan_id, cx);
            });
        }

        if self.list_state.item_count() != pinned.len() {
            self.list_state.reset(pinned.len());
        }
        let list_state = self.list_state.clone();

        v_flex()
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .on_mouse_down_out(cx.listener(|_, _: &MouseDownEvent, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .w(px(POPOVER_WIDTH))
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(tokens.border_primary)
            .bg(tokens.theme_setting_primary)
            .text_color(tokens.text_theme_message)
            .child(render_header(&theme, &locale))
            .child(render_body(
                pinned,
                loading,
                clan_id,
                theme.clone(),
                locale,
                handle,
                list_state,
                avatar_cache,
                window,
                cx,
            ))
    }
}

fn render_header(theme: &Theme, locale: &str) -> impl IntoElement {
    let tokens = &theme.tokens;
    h_flex()
        .w_full()
        .items_center()
        .gap_3()
        .px(px(16.))
        .h(px(HEADER_HEIGHT))
        .border_b_1()
        .border_color(tokens.border_primary)
        .bg(tokens.theme_setting_nav)
        .child(
            Icon::new(IconName::PinRight)
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
                    "channelTopbar.modals.pinnedMessages.title",
                )),
        )
}

#[allow(clippy::too_many_arguments)]
fn render_body(
    pinned: Rc<Vec<PinnedMessage>>,
    loading: bool,
    clan_id: Option<mezon_store::ClanId>,
    theme: Arc<Theme>,
    locale: String,
    popover_handle: PopoverMenuHandle<PinnedPopoverPanel>,
    list_state: ListState,
    avatar_cache: Entity<LruImageCache>,
    window: &mut Window,
    cx: &mut Context<PinnedPopoverPanel>,
) -> impl IntoElement {
    let tokens = &theme.tokens;

    let viewport_h = f32::from(window.viewport_size().height);
    let max_body = (viewport_h * MAX_VH - HEADER_HEIGHT).max(MIN_BODY_HEIGHT);
    let body_h = px(LIST_BODY_HEIGHT.min(max_body));

    let body: gpui::AnyElement = if pinned.is_empty() {
        let inner = if loading {
            Spinner::new().with_size(Size::Small).into_any_element()
        } else {
            div()
                .text_sm()
                .text_color(tokens.text_secondary)
                .child(mezon_i18n::t(
                    &locale,
                    "channelTopbar.pinnedMessages.emptyTitle",
                ))
                .into_any_element()
        };
        div()
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .child(inner)
            .into_any_element()
    } else {
        let pinned_for_list = pinned.clone();
        let theme_for_list = theme.clone();
        let locale_for_list = locale.clone();
        let handle_for_list = popover_handle.clone();
        let avatar_for_list = avatar_cache.clone();
        div()
            .size_full()
            .overflow_hidden()
            .child(
                list(list_state.clone(), move |ix, _window, cx| {
                    let Some(msg) = pinned_for_list.get(ix) else {
                        return div().into_any_element();
                    };
                    div()
                        .w_full()
                        .pt(if ix == 0 { px(8.) } else { px(0.) })
                        .pb(px(8.))
                        .child(pin_card(
                            ix,
                            msg,
                            clan_id,
                            &theme_for_list,
                            &locale_for_list,
                            handle_for_list.clone(),
                            avatar_for_list.clone(),
                            cx,
                        ))
                        .into_any_element()
                })
                .size_full(),
            )
            .custom_scrollbars(
                Scrollbars::new(ScrollAxes::Vertical).tracked_scroll_handle(&list_state),
                window,
                cx,
            )
            .into_any_element()
    };

    div()
        .w_full()
        .h(body_h)
        .overflow_hidden()
        .bg(tokens.theme_setting_primary)
        .child(body)
}

fn resolve_pin_sender(
    msg: &PinnedMessage,
    clan_id: Option<mezon_store::ClanId>,
    cx: &App,
) -> (String, String) {
    if let (Some(user_id), Some(clan_id)) = (
        msg.sender_id.parse::<UserId>().ok(),
        clan_id,
    ) && let Some(profile) = resolve_user_profile(user_id, ProfileContext::Clan(clan_id), cx)
    {
        return (profile.display_name, profile.avatar_url);
    }

    (msg.sender_name.clone(), msg.avatar_url.clone())
}

fn pin_card(
    index: usize,
    msg: &PinnedMessage,
    clan_id: Option<mezon_store::ClanId>,
    theme: &Theme,
    locale: &str,
    popover_handle: PopoverMenuHandle<PinnedPopoverPanel>,
    avatar_cache: Entity<LruImageCache>,
    cx: &App,
) -> gpui::AnyElement {
    let tokens = &theme.tokens;
    let group_name = SharedString::from(format!("pin-card-{index}"));
    let (sender_label, avatar_raw) = resolve_pin_sender(msg, clan_id, cx);
    let sender_color = Hsla::from(gpui::rgb(DEFAULT_DISPLAY_NAME_COLOR));

    let mut avatar = Avatar::new()
        .name(&sender_label)
        .with_size(Size::Small)
        .image_cache(avatar_cache);
    if !avatar_raw.is_empty() {
        let proxied = crate::util::imgproxy::avatar_url(cx, &avatar_raw);
        avatar = avatar
            .src(proxied)
            .fallback_src(avatar_raw);
    } else if !msg.avatar_proxied.is_empty() {
        avatar = avatar.src(msg.avatar_proxied.clone());
        if !msg.avatar_url.is_empty() && msg.avatar_url != msg.avatar_proxied.as_ref() {
            avatar = avatar.fallback_src(msg.avatar_url.clone());
        }
    }

    let name_row = h_flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(sender_color)
                .overflow_hidden()
                .text_ellipsis()
                .child(sender_label),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(tokens.text_secondary)
                .child(format_pin_time(msg.create_time, locale)),
        );

    let content = div()
        .text_sm()
        .text_color(tokens.text_theme_message)
        .child(msg.content.clone());

    let jump_message_id = msg.message_id.clone();
    let jump_handle = popover_handle.clone();
    let jump = Button::new(("pin-jump", index))
        .label(mezon_i18n::t(locale, "channelTopbar.tooltips.jump"))
        .ghost()
        .with_size(Size::XSmall)
        .on_click(move |_: &ClickEvent, _window, cx| {
            if let Ok(jump_target) = jump_message_id.parse::<MessageId>() {
                MessagesStore::global(cx).update(cx, |store, cx| {
                    store.jump_to_message(jump_target, cx);
                });
            }
            jump_handle.hide(cx);
        });

    let pin_id = msg.id.clone();
    let message_id = msg.message_id.clone();
    let delete = Button::new(("pin-del", index))
        .label("✕")
        .ghost()
        .with_size(Size::XSmall)
        .on_click(move |_: &ClickEvent, _window, cx| {
            let pin_id = pin_id.clone();
            let message_id = message_id.clone();
            PinnedMessagesStore::global(cx).update(cx, move |store, cx| {
                store.unpin(&pin_id, &message_id, cx);
            });
        });

    let actions = h_flex()
        .absolute()
        .top(px(8.))
        .right(px(8.))
        .items_center()
        .gap_1()
        .invisible()
        .group_hover(group_name.clone(), |s| s.visible())
        .child(jump)
        .child(delete);

    h_flex()
        .id(("pin-item", index))
        .group(group_name)
        .relative()
        .mx(px(8.))
        .items_start()
        .gap_2()
        .px(px(12.))
        .py(px(12.))
        .rounded(px(4.))
        .border_1()
        .border_color(tokens.border_primary)
        .bg(tokens.bg_active_member_channel)
        .child(avatar)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(name_row)
                .child(content),
        )
        .child(actions)
        .into_any_element()
}

/// Format a pin's create time (unix seconds): Today at HH:MM, Yesterday at HH:MM, otherwise dd/MM/yyyy, HH:MM (in the local timezone).
fn format_pin_time(create_time: i64, locale: &str) -> String {
    let Some(utc) = chrono::DateTime::from_timestamp(create_time, 0) else {
        return String::new();
    };
    let local = utc.with_timezone(&chrono::Local);
    let date = local.date_naive();
    let today = chrono::Local::now().date_naive();
    let time = local.format("%H:%M").to_string();

    if date == today {
        format!("{} {}", mezon_i18n::t(locale, "common.todayAt"), time)
    } else if Some(date) == today.pred_opt() {
        format!("{} {}", mezon_i18n::t(locale, "common.yesterdayAt"), time)
    } else {
        local.format("%d/%m/%Y, %H:%M").to_string()
    }
}

pub fn pin_popover_on_open() -> Rc<dyn Fn(&mut Window, &mut App)> {
    Rc::new(|_window, cx| {
        PinnedMessagesStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(cx);
            if let Some(clan_id) = store.clan_id() {
                ClanMembersStore::global(cx).update(cx, |members, cx| {
                    members.ensure_loaded(clan_id, cx);
                });
            }
        });
    })
}
