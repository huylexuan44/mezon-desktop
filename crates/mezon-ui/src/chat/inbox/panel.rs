use std::rc::Rc;

use gpui::{
    App, ClipboardItem, Context, DismissEvent, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, ListAlignment, ListState, MouseButton, MouseDownEvent, Render,
    SharedString, Subscription, Window, div, list, prelude::*, px,
};
use mezon_store::{
    ChannelId, ChannelType, ClanId, ClanList, InboxCategory, InboxEvent, InboxNotification,
    InboxStore, MessagesStore, TopicDiscussion, TopicsEvent, TopicsStore,
};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use crate::components::primitives::{h_flex, v_flex};

use crate::chat::inbox::InboxTab;
use crate::components::primitives::{Icon, IconName};
use crate::router::{Route, navigate};
use crate::theme::{ActiveTheme, Theme};

const PANEL_WIDTH: f32 = 480.;
const LIST_BODY_HEIGHT: f32 = 520.;
const ROW_HEIGHT: f32 = 104.;
const PREFETCH_THRESHOLD: usize = 5;

pub struct InboxPopoverPanel {
    tab: InboxTab,
    clan_id: String,
    locale: SharedString,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
    list_state: ListState,
    focus_handle: FocusHandle,
    _inbox_sub: Subscription,
    _topics_sub: Subscription,
}

impl InboxPopoverPanel {
    pub fn new(
        clan_id: String,
        locale: String,
        inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list_state = ListState::new(0, ListAlignment::Top, px(ROW_HEIGHT));
        let weak = cx.weak_entity();
        list_state.set_scroll_handler(move |event, _window, cx| {
            weak.update(cx, |panel, cx| {
                panel.maybe_load_more(event.visible_range.end, cx);
            })
            .ok();
        });

        let focus_handle = cx.focus_handle();
        cx.on_blur(&focus_handle, window, |_, _, cx| cx.emit(DismissEvent))
            .detach();

        let inbox_store = InboxStore::global(cx);
        let topics_store = TopicsStore::global(cx);

        if let Some(category) = InboxTab::Mentions.category() {
            inbox_store.update(cx, |store, cx| {
                store.fetch_if_empty(&clan_id, category, cx);
            });
        }

        let _inbox_sub = cx.subscribe(&inbox_store, |this, _, event, cx| {
            if matches!(event, InboxEvent::Updated) {
                this.sync_list_count(cx);
                cx.notify();
            }
        });
        let _topics_sub = cx.subscribe(&topics_store, |this, _, event, cx| {
            if matches!(event, TopicsEvent::Updated) {
                this.sync_list_count(cx);
                cx.notify();
            }
        });

        let mut this = Self {
            tab: InboxTab::Mentions,
            clan_id,
            locale: locale.into(),
            inbox_handle,
            list_state,
            focus_handle,
            _inbox_sub,
            _topics_sub,
        };
        this.sync_list_count(cx);
        this
    }

    fn sync_list_count(&mut self, cx: &mut Context<Self>) {
        let count = self.current_count(cx);
        if self.list_state.item_count() != count {
            self.list_state.reset(count);
        }
    }

    fn current_count(&self, cx: &App) -> usize {
        if self.tab == InboxTab::Topics {
            return TopicsStore::global(cx).read(cx).topics().len();
        }
        let Some(category) = self.tab.category() else {
            return 0;
        };
        InboxStore::global(cx)
            .read(cx)
            .items(&self.clan_id, category)
            .len()
    }

    fn current_items(&self, cx: &App) -> Rc<Vec<ListRow>> {
        if self.tab == InboxTab::Topics {
            let topics = TopicsStore::global(cx).read(cx).topics().to_vec();
            return Rc::new(topics.into_iter().map(ListRow::Topic).collect());
        }
        let category = self.tab.category().expect("notification tab");
        let items = InboxStore::global(cx)
            .read(cx)
            .items(&self.clan_id, category)
            .to_vec();
        Rc::new(items.into_iter().map(ListRow::Notification).collect())
    }

    fn select_tab(&mut self, tab: InboxTab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        if tab == InboxTab::Topics {
            TopicsStore::global(cx).update(cx, |store, cx| {
                store.fetch_if_needed(&self.clan_id, cx);
            });
        } else if let Some(category) = tab.category() {
            InboxStore::global(cx).update(cx, |store, cx| {
                store.fetch_if_empty(&self.clan_id, category, cx);
            });
        }
        self.sync_list_count(cx);
        cx.notify();
    }

    fn delete_notification(&self, id: &str, category: InboxCategory, cx: &mut Context<Self>) {
        InboxStore::global(cx).update(cx, |store, cx| {
            store.delete(&self.clan_id, id, category, cx);
        });
    }

    fn copy_message(&self, text: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    }

    fn maybe_load_more(&self, visible_end: usize, cx: &mut Context<Self>) {
        if self.tab == InboxTab::Topics {
            return;
        }
        let count = self.current_count(cx);
        if count == 0 || count.saturating_sub(visible_end) > PREFETCH_THRESHOLD {
            return;
        }
        if let Some(category) = self.tab.category() {
            InboxStore::global(cx).update(cx, |store, cx| {
                if store.has_more(&self.clan_id, category) {
                    store.fetch_more(&self.clan_id, category, cx);
                }
            });
        }
    }
}

#[derive(Clone)]
enum ListRow {
    Notification(InboxNotification),
    Topic(TopicDiscussion),
}

impl EventEmitter<DismissEvent> for InboxPopoverPanel {}

impl Focusable for InboxPopoverPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for InboxPopoverPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let theme_ref = theme.as_ref();
        let items = self.current_items(cx);
        let locale = self.locale.clone();
        let inbox_handle = self.inbox_handle.clone();
        let active_tab = self.tab;
        let topic_badge = TopicsStore::global(cx).read(cx).badge_label();
        let is_loading = if active_tab == InboxTab::Topics {
            TopicsStore::global(cx).read(cx).is_loading()
        } else if let Some(category) = active_tab.category() {
            InboxStore::global(cx)
                .read(cx)
                .is_loading(&self.clan_id, category)
        } else {
            false
        };

        let list_state = self.list_state.clone();
        let this = cx.weak_entity();

        let list_body: gpui::AnyElement = if items.is_empty() && is_loading {
            render_loading(theme_ref, &locale).into_any_element()
        } else if items.is_empty() {
            render_empty(theme_ref, &locale, active_tab).into_any_element()
        } else {
            let items_for_list = items;
            let locale_for_list = locale.clone();
            let inbox_handle_for_list = inbox_handle.clone();
            let panel_weak = this.clone();
            let list_state_for_scroll = list_state.clone();
            let theme_for_list = theme.clone();
            div()
                .size_full()
                .overflow_hidden()
                .child(
                    list(list_state.clone(), move |ix, _window, _cx| {
                        let Some(row) = items_for_list.get(ix).cloned() else {
                            return div().into_any_element();
                        };
                        render_row(
                            theme_for_list.as_ref(),
                            &locale_for_list,
                            row,
                            active_tab,
                            panel_weak.clone(),
                            inbox_handle_for_list.clone(),
                        )
                    })
                    .size_full(),
                )
                .custom_scrollbars(
                    Scrollbars::new(ScrollAxes::Vertical)
                        .tracked_scroll_handle(&list_state_for_scroll),
                    window,
                    cx,
                )
                .into_any_element()
        };

        v_flex()
            .flex()
            .flex_col()
            .id("inbox-popover-panel")
            .key_context("menu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .on_mouse_down_out(cx.listener(|_, _: &MouseDownEvent, _window, cx| {
                cx.emit(DismissEvent);
            }))
            .w(px(PANEL_WIDTH))
            .max_h(px(LIST_BODY_HEIGHT + 120.))
            .bg(theme.bg_primary)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .overflow_hidden()
            .shadow_lg()
            .child(
                v_flex()
                    .px_3()
                    .py_2()
                    .child(render_header(theme_ref, &locale))
                    .child(render_tabs(
                        theme_ref,
                        &locale,
                        active_tab,
                        topic_badge,
                        this.clone(),
                    )),
            )
            .child(
                div()
                    .h(px(LIST_BODY_HEIGHT))
                    .w_full()
                    .overflow_hidden()
                    .child(list_body),
            )
    }
}

fn render_header(theme: &Theme, locale: &SharedString) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_2()
        .pb_2()
        .child(
            Icon::new(IconName::Inbox)
                .size(px(20.))
                .text_color(theme.text_muted),
        )
        .child(
            div()
                .text_base()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child(mezon_i18n::t(locale, "notifications.inbox")),
        )
}

fn render_tabs(
    theme: &Theme,
    locale: &SharedString,
    active: InboxTab,
    topic_badge: Option<String>,
    this: gpui::WeakEntity<InboxPopoverPanel>,
) -> impl IntoElement {
    h_flex()
        .gap_4()
        .py_3()
        .border_b_1()
        .border_color(theme.border)
        .children(InboxTab::all().into_iter().map(move |tab| {
            let is_active = tab == active;
            let label = mezon_i18n::t(locale, tab.label_key());
            let this = this.clone();
            div()
                .id(SharedString::from(format!("inbox-tab-{:?}", tab)))
                .relative()
                .px_2()
                .py_1()
                .rounded(px(4.))
                .cursor_pointer()
                .when(is_active, |d| d.bg(theme.bg_hover))
                .text_base()
                .font_weight(if is_active {
                    gpui::FontWeight::MEDIUM
                } else {
                    gpui::FontWeight::NORMAL
                })
                .text_color(if is_active {
                    theme.text_primary
                } else {
                    theme.text_muted
                })
                .child(label)
                .when(tab == InboxTab::Topics && topic_badge.is_some(), |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(-4.))
                            .right(px(-4.))
                            .px_1()
                            .min_w(px(16.))
                            .h(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(theme.mention_badge)
                            .text_xs()
                            .text_color(theme.bg_primary)
                            .child(topic_badge.clone().unwrap_or_default()),
                    )
                })
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    this.update(cx, |panel, cx| panel.select_tab(tab, cx)).ok();
                })
        }))
}

fn render_loading(theme: &Theme, locale: &SharedString) -> impl IntoElement {
    let sk = theme.bg_hover;
    let row = || {
        div()
            .mx_3()
            .my_1()
            .p_3()
            .rounded(px(8.))
            .bg(theme.bg_secondary)
            .child(div().h(px(14.)).w(px(280.)).rounded(px(4.)).bg(sk))
            .child(div().mt_2().h(px(12.)).w(px(200.)).rounded(px(4.)).bg(sk))
    };
    v_flex()
        .size_full()
        .overflow_hidden()
        .child(row())
        .child(row())
        .child(row())
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(theme.text_muted)
                .child(mezon_i18n::t(locale, "channelTopbar.loading")),
        )
}

fn render_empty(theme: &Theme, locale: &SharedString, tab: InboxTab) -> impl IntoElement {
    let (title_key, desc_key, icon) = match tab {
        InboxTab::ForYou => (
            "notifications.empty.forYou.title",
            "notifications.empty.forYou.description",
            IconName::Inbox,
        ),
        InboxTab::Messages => (
            "notifications.empty.messages.title",
            "notifications.empty.messages.description",
            IconName::EmptyUnread,
        ),
        InboxTab::Mentions | InboxTab::Topics => (
            "notifications.empty.mentions.title",
            "notifications.empty.mentions.description",
            IconName::EmptyMention,
        ),
    };

    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .p_8()
        .child(Icon::new(icon).size(px(36.)).text_color(theme.text_muted))
        .child(
            div()
                .text_lg()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text_primary)
                .child(mezon_i18n::t(locale, title_key)),
        )
        .child(
            div()
                .text_sm()
                .text_center()
                .text_color(theme.text_muted)
                .max_w(px(360.))
                .child(mezon_i18n::t(locale, desc_key)),
        )
}

fn render_row(
    theme: &Theme,
    locale: &SharedString,
    row: ListRow,
    tab: InboxTab,
    this: gpui::WeakEntity<InboxPopoverPanel>,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
) -> gpui::AnyElement {
    match row {
        ListRow::Notification(notification) => {
            render_notification_item(theme, locale, notification, tab, this, inbox_handle)
        }
        ListRow::Topic(topic) => render_topic_item(theme, locale, topic, inbox_handle),
    }
}

fn schedule_inbox_jump(
    cx: &mut App,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
    route: Route,
    clan_id: String,
    channel_id: String,
    message_id: String,
) {
    cx.defer(move |cx| {
        navigate(cx, route);
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.jump_to_message(&clan_id, &channel_id, &message_id, cx);
        });
        inbox_handle.hide(cx);
    });
}

fn schedule_notification_jump(
    cx: &mut App,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
    notification: InboxNotification,
) {
    let message_id = notification
        .message
        .as_ref()
        .map(|m| m.message_id.as_str())
        .filter(|id| !id.is_empty())
        .unwrap_or("");
    if message_id.is_empty() {
        return;
    }
    let clan_id = notification.clan_id.clone();
    let channel_id = notification.channel_id.clone();
    let message_id = message_id.to_string();
    let Ok(clan) = clan_id.parse::<ClanId>() else {
        return;
    };
    let Ok(channel) = channel_id.parse::<ChannelId>() else {
        return;
    };
    let route = if ChannelType::from_raw(notification.channel_type as u32) == ChannelType::Thread {
        Route::Thread {
            clan_id: clan,
            channel_id: channel,
            thread_id: channel,
        }
    } else {
        Route::Channel {
            clan_id: clan,
            channel_id: channel,
        }
    };
    schedule_inbox_jump(cx, inbox_handle, route, clan_id, channel_id, message_id);
}

fn schedule_topic_jump(
    cx: &mut App,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
    topic: TopicDiscussion,
) {
    if topic.message_id.is_empty() || topic.channel_id.is_empty() {
        return;
    }
    let Ok(clan) = topic.clan_id.parse::<ClanId>() else {
        return;
    };
    let Ok(channel) = topic.channel_id.parse::<ChannelId>() else {
        return;
    };
    schedule_inbox_jump(
        cx,
        inbox_handle,
        Route::Channel {
            clan_id: clan,
            channel_id: channel,
        },
        topic.clan_id,
        topic.channel_id,
        topic.message_id,
    );
}

fn render_notification_item(
    theme: &Theme,
    locale: &SharedString,
    notification: InboxNotification,
    tab: InboxTab,
    this: gpui::WeakEntity<InboxPopoverPanel>,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
) -> gpui::AnyElement {
    let category = notification.category;
    let id: SharedString = notification.id.clone().into();
    let preview: SharedString = notification.preview_text().into();
    let show_jump = tab == InboxTab::Mentions;
    let show_copy = tab == InboxTab::Messages;
    let copy_text = preview.clone();
    let jump_notification = notification;
    let this_delete = this.clone();
    let this_copy = this.clone();
    let inbox_handle_jump = inbox_handle.clone();
    let jump_label = mezon_i18n::t(locale, "channelTopbar.tooltips.jump");

    div()
        .h(px(ROW_HEIGHT))
        .overflow_hidden()
        .flex()
        .flex_col()
        .px_3()
        .py_2()
        .w_full()
        .child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .relative()
                .group("inbox-item")
                .p_2()
                .rounded(px(8.))
                .bg(theme.bg_secondary)
                .child(
                    div()
                        .absolute()
                        .top(px(4.))
                        .right(px(4.))
                        .w(px(20.))
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .bg(theme.bg_hover)
                        .hover(|s| s.bg(theme.bg_primary))
                        .child(
                            Icon::new(IconName::Close)
                                .size(px(12.))
                                .text_color(theme.text_muted),
                        )
                        .on_mouse_down(MouseButton::Left, {
                            let id = id.clone();
                            move |_, _, cx| {
                                this_delete
                                    .update(cx, |panel, cx| {
                                        panel.delete_notification(&id, category, cx);
                                    })
                                    .ok();
                            }
                        }),
                )
                .when(show_copy, |card| {
                    card.child(
                        div()
                            .absolute()
                            .top(px(4.))
                            .right(px(28.))
                            .w(px(20.))
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .bg(theme.bg_hover)
                            .opacity(0.)
                            .group_hover("inbox-item", |s| s.opacity(1.))
                            .hover(|s| s.bg(theme.bg_primary))
                            .child(
                                Icon::new(IconName::CopyIcon)
                                    .size(px(12.))
                                    .text_color(theme.text_muted),
                            )
                            .on_mouse_down(MouseButton::Left, {
                                move |_, _, cx| {
                                    this_copy
                                        .update(cx, |panel, cx| {
                                            panel.copy_message(&copy_text, cx);
                                        })
                                        .ok();
                                }
                            }),
                    )
                })
                .when(show_jump, |card| {
                    card.child(
                        div()
                            .absolute()
                            .bottom(px(10.))
                            .right(px(12.))
                            .px_2()
                            .py_1()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .bg(theme.bg_hover)
                            .border_1()
                            .border_color(theme.border)
                            .text_xs()
                            .text_color(theme.text_primary)
                            .opacity(0.)
                            .group_hover("inbox-item", |s| s.opacity(1.))
                            .child(jump_label)
                            .on_mouse_down(MouseButton::Left, {
                                move |_, _, cx| {
                                    schedule_notification_jump(
                                        cx,
                                        inbox_handle_jump.clone(),
                                        jump_notification.clone(),
                                    );
                                }
                            }),
                    )
                })
                .child(
                    div()
                        .pr(if show_copy { px(52.) } else { px(28.) })
                        .when(show_jump, |content| content.pb(px(28.)))
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text_primary)
                        .overflow_hidden()
                        .child(if preview.is_empty() {
                            SharedString::from("—")
                        } else {
                            preview
                        }),
                ),
        )
        .into_any_element()
}

fn render_topic_item(
    theme: &Theme,
    locale: &SharedString,
    topic: TopicDiscussion,
    inbox_handle: ui::PopoverMenuHandle<InboxPopoverPanel>,
) -> gpui::AnyElement {
    let reply_preview: SharedString = if topic.reply_is_attachment() {
        mezon_i18n::t(locale, "message.clickToSeeAttachment").into()
    } else if topic.reply_preview_text().is_empty() {
        SharedString::from("—")
    } else {
        topic.reply_preview_text().into()
    };
    let jump_topic = topic;
    let inbox_handle_jump = inbox_handle.clone();
    let jump_label = mezon_i18n::t(locale, "channelTopbar.tooltips.jump");
    let replied_label = mezon_i18n::t(locale, "notification.repliedTo");
    let topic_title = mezon_i18n::t(locale, "notification.topicAndYou");

    div()
        .h(px(ROW_HEIGHT))
        .overflow_hidden()
        .flex()
        .flex_col()
        .px_3()
        .py_2()
        .w_full()
        .child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .relative()
                .group("inbox-topic")
                .p_2()
                .rounded(px(8.))
                .bg(theme.bg_secondary)
                .child(
                    div()
                        .absolute()
                        .bottom(px(10.))
                        .right(px(12.))
                        .px_2()
                        .py_1()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .bg(theme.bg_hover)
                        .border_1()
                        .border_color(theme.border)
                        .text_xs()
                        .text_color(theme.text_primary)
                        .opacity(0.)
                        .group_hover("inbox-topic", |s| s.opacity(1.))
                        .child(jump_label)
                        .on_mouse_down(MouseButton::Left, {
                            move |_, _, cx| {
                                schedule_topic_jump(
                                    cx,
                                    inbox_handle_jump.clone(),
                                    jump_topic.clone(),
                                );
                            }
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme.text_primary)
                        .child(topic_title),
                )
                .child(
                    h_flex()
                        .mt_1()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(replied_label)
                        .child(reply_preview),
                ),
        )
        .into_any_element()
}

pub fn clan_has_inbox_badge(clan_id: &str, cx: &App) -> bool {
    let Ok(clan_id) = clan_id.parse::<ClanId>() else {
        return false;
    };
    ClanList::global(cx)
        .read(cx)
        .clans
        .iter()
        .find(|c| c.id == clan_id)
        .is_some_and(|c| c.badge_count > 0 || c.has_unread)
}
