use std::sync::Arc;

use crate::components::primitives::{InputEvent, InputState};
use gpui::{AnyView, App, Context, Entity, StyleRefinement, SharedString, Window, div, prelude::*, px};
use mezon_store::Settings;
use ui::PopoverMenuHandle;

use crate::chat::ReplyTarget;
use crate::chat::channel_header::ChannelHeader;
use crate::chat::inbox::InboxPopoverPanel;
use crate::chat::input_bar::InputBar;
use crate::chat::member_list::{MemberListPanel, MemberSource};
use crate::chat::message_list::MessageTimeline;
use crate::theme::Theme;

pub struct ChatArea {
    pub(crate) timeline: Entity<MessageTimeline>,
    pub(crate) input_state: Option<Entity<InputState>>,
    member_panel: Option<Entity<MemberListPanel>>,
    member_source: Option<MemberSource>,
    #[allow(dead_code)]
    replying_to: Option<ReplyTarget>,
    settings: Entity<Settings>,
}

impl ChatArea {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<crate::ChatLayout>) -> Self {
        let timeline = cx.new({
            let settings = settings.clone();
            move |cx| MessageTimeline::new(settings, cx)
        });
        Self {
            timeline,
            input_state: None,
            member_panel: None,
            member_source: None,
            replying_to: None,
            settings,
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
            cx.new(move |cx| MemberListPanel::new(source, settings, cx))
        });
    }

    pub fn ensure_input(&mut self, window: &mut Window, cx: &mut Context<crate::ChatLayout>) {
        if self.input_state.is_none() {
            let locale = self.settings.read(cx).language.clone();
            let placeholder = mezon_i18n::t(&locale, "chat.messagePlaceholder");
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            cx.subscribe_in(
                &input,
                window,
                |this: &mut crate::ChatLayout, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter = event {
                        this.send_current_message(window, cx);
                    }
                },
            )
            .detach();
            self.input_state = Some(input);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        theme: &Theme,
        locale: &str,
        layout_entity: Entity<crate::ChatLayout>,
        channel_name: &str,
        is_dm: bool,
        show_members_button: bool,
        show_member_panel: bool,
        typing_label: Option<SharedString>,
        clan_id: Option<&str>,
        inbox_handle: Option<PopoverMenuHandle<InboxPopoverPanel>>,
        window: &mut Window,
        cx: &App,
    ) -> gpui::AnyElement {
        let input_state = match self.input_state.clone() {
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

        let on_send = {
            let handle = layout_entity.clone();
            Arc::new(move |window: &mut Window, cx: &mut App| {
                handle.update(cx, |this, cx| this.send_current_message(window, cx));
            })
        };

        let input_bar = InputBar::new()
            .with_input(input_state)
            .on_send(on_send)
            .typing_label(typing_label);

        let mut header = ChannelHeader::new(channel_name)
            .dm(is_dm)
            .show_inbox(!is_dm)
            .members_action(show_members_button)
            .members_active(show_member_panel)
            .on_toggle_members({
                let handle = layout_entity.clone();
                Arc::new(move |_window: &mut Window, cx: &mut App| {
                    handle.update(cx, |this, cx| this.toggle_member_list(cx));
                })
            });

        if let (Some(clan_id), Some(handle)) = (clan_id, inbox_handle) {
            header = header.inbox_popover(handle).inbox_context(clan_id, locale);
        }

        let message_column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(div().flex_1().min_h_0().child(
                // Cache the message list so an unrelated sibling/parent notify
                // (presence in the member panel, typing indicator, user info
                // bar, theme change…) does not force a full re-layout of every
                // row. It self-invalidates whenever the timeline itself
                // notifies (scroll, new message, GIF animation), so behaviour
                // is unchanged — only redundant re-renders are skipped.
                AnyView::from(self.timeline.clone()).cached(StyleRefinement::default().size_full()),
            ))
            .child(input_bar.render(theme, locale));

        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .child(message_column)
            .when(show_member_panel, |row| match &self.member_panel {
                // Cache the member panel so it is not re-rendered (and its avatars
                // re-painted) every frame the message timeline notifies during
                // scroll/load-more. GPUI marks the whole ancestor chain of a
                // notified view dirty, so the timeline's churn forces chat_layout
                // to re-render its subtree; caching keeps the panel reused unless
                // the panel itself is notified (member/presence change or scroll).
                Some(panel) => row.child(
                    AnyView::from(panel.clone()).cached(
                        StyleRefinement::default()
                            .w(px(245.))
                            .h_full()
                            .flex_shrink_0(),
                    ),
                ),
                None => row,
            });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .child(header.render(theme, window, cx))
            .child(body)
            .into_any_element()
    }
}
