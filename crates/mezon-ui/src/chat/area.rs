use std::sync::Arc;

use crate::components::primitives::{InputEvent, InputState};
use gpui::{AnyView, App, Context, Entity, SharedString, Window, div, prelude::*};
use ui::PopoverMenuHandle;

use crate::chat::ReplyTarget;
use crate::chat::channel_header::ChannelHeader;
use crate::chat::input_bar::InputBar;
use crate::chat::message_list::MessageTimeline;
use crate::chat::threads_popover::ThreadsPopoverPanel;
use crate::theme::Theme;

pub struct ChatAreaRenderParams<'a> {
    pub theme: &'a Theme,
    pub locale: &'a str,
    pub layout_entity: Entity<crate::ChatLayout>,
    pub channel_name: &'a str,
    pub is_dm: bool,
    pub typing_label: Option<SharedString>,
    pub thread_handle: Option<PopoverMenuHandle<ThreadsPopoverPanel>>,
    pub show_threads: bool,
}

pub struct ChatArea {
    pub(crate) timeline: Entity<MessageTimeline>,
    pub(crate) input_state: Option<Entity<InputState>>,
    #[allow(dead_code)]
    replying_to: Option<ReplyTarget>,
    settings: Entity<mezon_store::Settings>,
}

impl ChatArea {
    pub fn new(settings: Entity<mezon_store::Settings>, cx: &mut Context<crate::ChatLayout>) -> Self {
        let timeline = cx.new({
            let settings = settings.clone();
            move |cx| MessageTimeline::new(settings, cx)
        });
        Self {
            timeline,
            input_state: None,
            replying_to: None,
            settings,
        }
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

    pub fn render(
        &self,
        params: ChatAreaRenderParams<'_>,
        window: &mut Window,
        cx: &mut Context<crate::ChatLayout>,
    ) -> gpui::AnyElement {
        let Some(input_state) = self.input_state.clone() else {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .into_any_element();
        };

        let on_send = {
            let handle = params.layout_entity.clone();
            Arc::new(move |window: &mut Window, cx: &mut App| {
                handle.update(cx, |this, cx| this.send_current_message(window, cx));
            })
        };

        let input_bar = InputBar::new()
            .with_input(input_state)
            .on_send(on_send)
            .typing_label(params.typing_label);

        let mut header = ChannelHeader::new(params.channel_name)
            .dm(params.is_dm)
            .show_threads(params.show_threads)
            .layout(params.layout_entity.clone());
        if let Some(handle) = params.thread_handle {
            header = header.thread_popover(handle);
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .w_full()
            .h_full()
            .overflow_hidden()
            .child(header.render(params.theme, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(AnyView::from(self.timeline.clone())),
            )
            .child(input_bar.render(params.theme, params.locale))
            .into_any_element()
    }
}
