use gpui::{
    AnyElement, Context, Entity, FollowMode, ListAlignment, ListState, SharedString, Task, Window,
    div, list, prelude::*, px,
};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use mezon_store::{ChannelId, ChannelList, Message, MessagesEvent, MessagesStore, Settings};

use crate::chat::message_row::{MessageAttachmentView, MessageRow};
use crate::image_cache::{
    AVATAR_IMAGE_CACHE_CAPACITY, LruImageCache, MESSAGE_IMAGE_CACHE_CAPACITY,
};
use crate::theme::{ActiveTheme, Theme};

const LOAD_MORE_ITEM_THRESHOLD: usize = 6;
const SCROLL_HOVER_RELEASE_MS: u64 = 150;

pub struct MessageTimeline {
    pub(crate) list_state: ListState,
    settings: Entity<Settings>,
    image_cache: Entity<LruImageCache>,
    avatar_image_cache: Entity<LruImageCache>,
    cached_for_channel: Option<ChannelId>,
    skeleton_armed: bool,
    skeleton_channel: Option<ChannelId>,
    _skeleton_timer: Option<Task<()>>,
    suppress_hover: bool,
    _hover_release_task: Option<Task<()>>,
}

impl MessageTimeline {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();

        let store = MessagesStore::global(cx);
        cx.subscribe(&store, |this, store, event, cx| {
            match event {
                MessagesEvent::JumpTo { index } => {
                    this.list_state.scroll_to(gpui::ListOffset {
                        item_ix: *index,
                        offset_in_item: px(0.),
                    });
                }
                MessagesEvent::Reset { count } => {
                    this.list_state.reset(*count);
                    this.list_state.set_follow_mode(FollowMode::Tail);
                }
                MessagesEvent::OlderPrepended { count } => {
                    this.list_state.splice(0..0, *count);
                }
                MessagesEvent::Appended => {
                    let new_len = store.read(cx).messages().len();
                    let old_len = this.list_state.item_count();
                    if new_len >= old_len {
                        this.list_state.splice(old_len..old_len, new_len - old_len);
                    } else {
                        this.list_state.reset(new_len);
                    }
                }
            }
            cx.notify();
        })
        .detach();

        let list_state = ListState::new(0, ListAlignment::Bottom, px(200.));
        list_state.set_follow_mode(FollowMode::Tail);
        let timeline = cx.weak_entity();
        list_state.set_scroll_handler(move |event, _window, cx| {
            if event.visible_range.start < LOAD_MORE_ITEM_THRESHOLD {
                MessagesStore::global(cx).update(cx, |store, cx| store.load_more(cx));
            }
            let _ = timeline.update(cx, |this, cx| {
                let was_suppressed = this.suppress_hover;
                this.suppress_hover = true;
                if !was_suppressed {
                    cx.notify();
                }
                this._hover_release_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(SCROLL_HOVER_RELEASE_MS))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.suppress_hover {
                            this.suppress_hover = false;
                            cx.notify();
                        }
                    });
                }));
            });
        });
        let image_cache = cx.new(|cx| LruImageCache::new(MESSAGE_IMAGE_CACHE_CAPACITY, cx));
        let avatar_image_cache = cx.new(|cx| LruImageCache::new(AVATAR_IMAGE_CACHE_CAPACITY, cx));
        Self {
            list_state,
            settings,
            image_cache,
            avatar_image_cache,
            cached_for_channel: None,
            skeleton_armed: false,
            skeleton_channel: None,
            _skeleton_timer: None,
            suppress_hover: false,
            _hover_release_task: None,
        }
    }

    fn clear_image_cache_if_channel_changed(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channel_id = ChannelList::global(cx).read(cx).active_channel_id;
        if self.cached_for_channel == channel_id {
            return;
        }
        self.cached_for_channel = channel_id;
        self.image_cache
            .update(cx, |cache, cx| cache.clear(window, cx));
        self.avatar_image_cache
            .update(cx, |cache, cx| cache.clear(window, cx));
    }
}

impl Render for MessageTimeline {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("MessageTimeline");
        self.clear_image_cache_if_channel_changed(window, cx);

        let store = MessagesStore::global(cx);
        let channel_id = ChannelList::global(cx).read(cx).active_channel_id;
        let is_empty = store.read(cx).messages().is_empty();
        let loading = store.read(cx).is_loading() && is_empty;
        if loading {
            if self.skeleton_channel != channel_id {
                self.skeleton_channel = channel_id;
                self.skeleton_armed = false;
                self._skeleton_timer = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(200))
                        .await;
                    this.update(cx, |this, cx| {
                        this.skeleton_armed = true;
                        cx.notify();
                    })
                    .ok();
                }));
            }
        } else {
            self.skeleton_armed = false;
            self.skeleton_channel = None;
            self._skeleton_timer = None;
        }
        let show_skeleton = loading && self.skeleton_armed;

        let locale = self.settings.read(cx).language.clone();
        let reply_label: SharedString = mezon_i18n::t(&locale, "chat.replyingToSomeone").into();
        let list_state = self.list_state.clone();
        let suppress_hover = self.suppress_hover;
        let avatar_image_cache = self.avatar_image_cache.clone();

        if show_skeleton {
            return div()
                .size_full()
                .image_cache(self.image_cache.clone())
                .child(message_skeleton(cx))
                .into_any_element();
        }

        div()
            .size_full()
            .overflow_hidden()
            .image_cache(self.image_cache.clone())
            .child(
                list(list_state, move |ix, _window, cx| {
                    render_row(
                        store.read(cx).messages(),
                        ix,
                        cx,
                        "",
                        &reply_label,
                        suppress_hover,
                        avatar_image_cache.clone(),
                    )
                })
                .flex_1()
                .size_full(),
            )
            .custom_scrollbars(
                Scrollbars::new(ScrollAxes::Vertical).tracked_scroll_handle(&self.list_state),
                window,
                cx,
            )
            .into_any_element()
    }
}

fn message_skeleton(cx: &gpui::App) -> AnyElement {
    let sk = cx.theme().bg_hover;
    let row = move |name_w: f32, line_w: f32| {
        div()
            .flex()
            .flex_row()
            .w_full()
            .px_4()
            .pt_3()
            .gap_4()
            .child(div().size(px(40.)).rounded_full().bg(sk).flex_none())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_2()
                    .child(div().h(px(14.)).w(px(name_w)).rounded(px(4.)).bg(sk))
                    .child(div().h(px(12.)).w(px(line_w)).rounded(px(4.)).bg(sk)),
            )
    };
    div()
        .flex()
        .flex_col()
        .size_full()
        .py_2()
        .child(row(110., 300.))
        .child(row(90., 180.))
        .child(row(130., 240.))
        .child(row(80., 320.))
        .child(row(100., 150.))
        .child(row(120., 260.))
        .into_any_element()
}

fn render_row(
    messages: &[Message],
    ix: usize,
    cx: &gpui::App,
    current_user_id: &str,
    reply_label: &SharedString,
    suppress_hover: bool,
    avatar_image_cache: Entity<LruImageCache>,
) -> AnyElement {
    let theme = cx.theme();
    let Some(msg) = messages.get(ix) else {
        return div().into_any_element();
    };
    let prev = ix.checked_sub(1).and_then(|p| messages.get(p));

    let day_label = msg.day_label.as_str();
    let show_separator = prev.map(|p| p.day_label.as_str()) != Some(day_label);
    let combined = !show_separator && msg.combined_with_prev;

    let attachment_views = attachment_views(msg);
    let message_row = MessageRow::new(msg, theme, current_user_id, reply_label.clone())
        .combined(combined)
        .avatar_src(msg.avatar_proxied.clone())
        .avatar_image_cache(avatar_image_cache)
        .attachments(attachment_views)
        .suppress_hover(suppress_hover);

    let mut column = div().flex().flex_col().w_full();
    if show_separator {
        column = column.child(date_separator(theme, day_label));
    }
    column.child(message_row.render()).into_any_element()
}

fn attachment_views(msg: &Message) -> Vec<MessageAttachmentView> {
    msg.attachments
        .iter()
        .map(|att| {
            if att.is_image() {
                let label = if att.filename.is_empty() {
                    "image".to_string()
                } else {
                    att.filename.clone()
                };
                MessageAttachmentView::Image {
                    src: att.proxied_src.clone(),
                    width: px(att.display_width),
                    height: px(att.display_height),
                    label,
                }
            } else {
                let label = if att.filename.is_empty() {
                    "Attachment".to_string()
                } else {
                    att.filename.clone()
                };
                MessageAttachmentView::File { label }
            }
        })
        .collect()
}

fn date_separator(theme: &Theme, label: &str) -> impl IntoElement {
    let id = SharedString::from(format!("date-sep-{}", label));
    let label_owned = label.to_string();
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .w_full()
        .child(div().flex_1().h(px(1.)).bg(theme.border))
        .child(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(label_owned),
        )
        .child(div().flex_1().h(px(1.)).bg(theme.border))
}
