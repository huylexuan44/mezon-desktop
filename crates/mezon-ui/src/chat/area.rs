use gpui::{
    AnyView, Context, Entity, SharedString, StyleRefinement, Subscription, Window, div, prelude::*,
    px,
};
use mezon_store::{ChannelId, Settings};
use ui::PopoverMenuHandle;

use crate::chat::ReplyTarget;
use crate::chat::channel_header::ChatHeader;
use crate::chat::channel_typing::ChannelTyping;
use crate::chat::input_bar::InputBar;
use crate::chat::member_list::{MemberListPanel, MemberSource};
use crate::chat::mention_input::{MentionInput, MentionInputEvent};
use crate::chat::message::ChannelMessages;
use crate::chat::pinned_popover::PinnedPopoverPanel;
use crate::image_cache::{
    AVATAR_ENTRY_MAX_BYTES, AVATAR_IMAGE_CACHE_BYTES, AVATAR_IMAGE_CACHE_CAPACITY, LruImageCache,
};
use crate::theme::ActiveTheme;

pub struct ChatArea {
    pub(crate) timeline: Entity<ChannelMessages>,
    pub(crate) mention_input: Option<Entity<MentionInput>>,
    member_panel: Option<Entity<MemberListPanel>>,
    member_source: Option<MemberSource>,
    member_avatar_cache: Entity<LruImageCache>,
    #[allow(dead_code)]
    replying_to: Option<ReplyTarget>,
    settings: Entity<Settings>,
    header: Entity<ChatHeader>,
    typing: Entity<ChannelTyping>,
    _submit_sub: Option<Subscription>,
}

impl ChatArea {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<crate::ChatLayout>) -> Self {
        let timeline = cx.new({
            let settings = settings.clone();
            move |cx| ChannelMessages::new(settings, cx)
        });
        let layout = cx.weak_entity();
        let header = cx.new(|cx| ChatHeader::new(layout, &settings, cx));
        let typing = cx.new(|cx| ChannelTyping::new(&settings, cx));
        let member_avatar_cache = cx.new(|cx| {
            LruImageCache::avatar_thumbnail(
                "member-avatar",
                AVATAR_IMAGE_CACHE_CAPACITY,
                AVATAR_IMAGE_CACHE_BYTES,
                AVATAR_ENTRY_MAX_BYTES,
                cx,
            )
        });
        Self {
            timeline,
            mention_input: None,
            member_panel: None,
            member_source: None,
            member_avatar_cache,
            replying_to: None,
            settings,
            header,
            typing,
            _submit_sub: None,
        }
    }

    pub fn bind_channel_members(&mut self, cx: &mut Context<crate::ChatLayout>) {
        self.set_member_source(Some(MemberSource::Channel), cx);
    }

    pub fn bind_group_members(&mut self, cx: &mut Context<crate::ChatLayout>) {
        self.set_member_source(Some(MemberSource::Group), cx);
    }

    pub fn clear_member_panel(&mut self) {
        self.member_source = None;
        self.member_panel = None;
    }

    fn set_member_source(
        &mut self,
        source: Option<MemberSource>,
        cx: &mut Context<crate::ChatLayout>,
    ) {
        if self.member_source == source {
            return;
        }
        self.member_source = source;
        self.member_panel = source.map(|source| {
            let settings = self.settings.clone();
            let avatar_cache = self.member_avatar_cache.clone();
            cx.new(move |cx| MemberListPanel::new(source, settings, avatar_cache, cx))
        });
    }

    pub fn bind_window(&mut self, window: &mut Window, cx: &mut Context<crate::ChatLayout>) {
        self.timeline
            .update(cx, |timeline, cx| timeline.bind_window(window, cx));
    }

    pub fn ensure_input(&mut self, window: &mut Window, cx: &mut Context<crate::ChatLayout>) {
        if self.mention_input.is_none() {
            let locale = self.settings.read(cx).language.clone();
            let placeholder = mezon_i18n::t(&locale, "messageBox.placeholder");
            let settings = self.settings.clone();
            let mention_input = cx.new(|cx| MentionInput::new(placeholder, settings, window, cx));
            let submit_sub = cx.subscribe_in(
                &mention_input,
                window,
                |this: &mut crate::ChatLayout, _, event: &MentionInputEvent, window, cx| match event
                {
                    MentionInputEvent::Submit => this.send_current_message(window, cx),
                    MentionInputEvent::SendSticker { url, filename } => {
                        this.send_sticker(url.clone(), filename.clone(), cx)
                    }
                },
            );
            self._submit_sub = Some(submit_sub);
            self.mention_input = Some(mention_input);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        locale: &str,
        channel_name: Option<&str>,
        is_dm: bool,
        channel_id: Option<ChannelId>,
        show_members_button: bool,
        show_member_panel: bool,
        pin_handle: Option<PopoverMenuHandle<PinnedPopoverPanel>>,
        cx: &mut Context<crate::ChatLayout>,
    ) -> gpui::AnyElement {
        let mention_input = match self.mention_input.clone() {
            Some(s) => s,
            None => {
                return div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .into_any_element();
            }
        };

        self.header.update(cx, |header, cx| {
            header.sync(
                channel_name.map(SharedString::from),
                is_dm,
                show_members_button,
                show_member_panel,
                pin_handle,
                cx,
            );
        });

        self.typing
            .update(cx, |typing, cx| typing.sync(channel_id, cx));

        let theme = cx.theme();

        let input_bar = InputBar::new().with_mention_input(mention_input);

        let header = AnyView::from(self.header.clone()).cached(
            StyleRefinement::default()
                .w_full()
                .h(px(crate::app::window_controls::APP_HEADER_HEIGHT))
                .flex_shrink_0(),
        );

        let message_column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(div().flex_1().min_h_0().overflow_hidden().child(
                AnyView::from(self.timeline.clone()).cached(StyleRefinement::default().size_full()),
            ))
            .child(self.typing.clone())
            .child(input_bar.render(theme, locale));

        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .child(message_column)
            .when(show_member_panel, |row| match &self.member_panel {
                Some(panel) => row.child(
                    AnyView::from(panel.clone()).cached(
                        StyleRefinement::default()
                            .w(px(245.))
                            .h_full()
                            .flex_shrink_0(),
                    ),
                ),
                None => row.child(div().w(px(245.)).h_full().flex_shrink_0()),
            });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .w_full()
            .h_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(header)
            .child(body)
            .into_any_element()
    }
}
