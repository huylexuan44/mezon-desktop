use std::sync::Arc;

use gpui::{
    AnyView, App, Context, Entity, SharedString, StyleRefinement, Window, div, prelude::*, px,
};
use mezon_store::{ChannelId, Settings};

use crate::chat::ReplyTarget;
use crate::chat::channel_header::ChatHeader;
use crate::chat::channel_typing::ChannelTyping;
use crate::chat::input_bar::InputBar;
use crate::chat::member_list::{MemberListPanel, MemberSource};
use crate::chat::message::ChannelMessages;
use crate::components::primitives::{InputEvent, InputState};
use crate::theme::ActiveTheme;

pub struct ChatArea {
    pub(crate) timeline: Entity<ChannelMessages>,
    pub(crate) input_state: Option<Entity<InputState>>,
    member_panel: Option<Entity<MemberListPanel>>,
    member_source: Option<MemberSource>,
    #[allow(dead_code)]
    replying_to: Option<ReplyTarget>,
    settings: Entity<Settings>,
    header: Entity<ChatHeader>,
    typing: Entity<ChannelTyping>,
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
        Self {
            timeline,
            input_state: None,
            member_panel: None,
            member_source: None,
            replying_to: None,
            settings,
            header,
            typing,
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
        locale: &str,
        channel_name: Option<&str>,
        is_dm: bool,
        channel_id: Option<ChannelId>,
        show_members_button: bool,
        show_member_panel: bool,
        cx: &mut Context<crate::ChatLayout>,
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

        self.header.update(cx, |header, cx| {
            header.sync(
                channel_name.map(SharedString::from),
                is_dm,
                show_members_button,
                show_member_panel,
                cx,
            );
        });

        self.typing
            .update(cx, |typing, cx| typing.sync(channel_id, cx));

        let layout_entity = cx.entity();
        let theme = cx.theme();

        let on_send = {
            let handle = layout_entity.clone();
            Arc::new(move |window: &mut Window, cx: &mut App| {
                handle.update(cx, |this, cx| this.send_current_message(window, cx));
            })
        };

        let input_bar = InputBar::new().with_input(input_state).on_send(on_send);

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
            .child(div().flex_1().min_h_0().child(
                AnyView::from(self.timeline.clone()).cached(StyleRefinement::default().size_full()),
            ))
            .child(self.typing.clone())
            .child(input_bar.render(theme, locale));

        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
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
            .min_w_0()
            .min_h_0()
            .child(header)
            .child(body)
            .into_any_element()
    }
}
