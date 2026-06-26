use gpui::{FontWeight, SharedString, div, prelude::*, px};
use mezon_store::ChannelType;

use crate::components::primitives::Icon;
use crate::components::primitives::IconName;
use crate::theme::Theme;

pub struct ChannelRow {
    name: SharedString,
    channel_type: ChannelType,
    unread: bool,
    private: bool,
    selected: bool,
    badge_count: u32,
    badge_label: SharedString,
    muted: bool,
    is_thread: bool,
}

impl ChannelRow {
    pub fn new(name: impl Into<SharedString>, channel_type: ChannelType) -> Self {
        Self {
            name: name.into(),
            channel_type,
            unread: false,
            private: false,
            selected: false,
            badge_count: 0,
            badge_label: SharedString::from(""),
            muted: false,
            is_thread: false,
        }
    }

    pub fn is_thread(mut self, is_thread: bool) -> Self {
        self.is_thread = is_thread;
        self
    }

    pub fn unread(mut self, unread: bool) -> Self {
        self.unread = unread;
        self
    }

    pub fn private(mut self, private: bool) -> Self {
        self.private = private;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn badge_count(mut self, count: u32) -> Self {
        self.badge_count = count;
        self.badge_label = if count > 99 {
            SharedString::from("99+")
        } else {
            SharedString::from(count.to_string())
        };
        self
    }

    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    pub fn render(self, theme: &Theme) -> impl IntoElement {
        let icon = channel_type_icon(self.channel_type, self.private);
        let text_color = if self.muted {
            theme.text_muted
        } else if self.selected {
            theme.text_primary
        } else {
            theme.text_secondary
        };
        let selected_bg = theme.bg_primary;
        let secondary = theme.text_secondary;
        let brand = theme.brand;
        let text_primary = theme.text_primary;

        div().w_full().px_2().child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .px_2()
                .py_1()
                .rounded_lg()
                .cursor_pointer()
                .when(self.selected, move |el| el.bg(selected_bg))
                .text_color(text_color)
                .child(Icon::new(icon).size(px(16.0)).text_color(secondary))
                .child(
                    div()
                        .flex_1()
                        .ml_2()
                        .text_sm()
                        .overflow_hidden()
                        .when(self.unread && !self.muted, |el| {
                            el.font_weight(FontWeight::BOLD)
                        })
                        .child(self.name),
                )
                .when(
                    crate::SHOW_UNREAD_BADGE_COUNT && self.badge_count > 0 && !self.muted,
                    move |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .min_w(px(16.0))
                                .h(px(16.0))
                                .px_1()
                                .rounded_full()
                                .bg(brand)
                                .text_color(text_primary)
                                .text_xs()
                                .child(self.badge_label.clone()),
                        )
                    },
                )
                .when(
                    self.badge_count == 0 && self.unread && !self.muted,
                    move |el| el.child(div().size_2().rounded_full().bg(brand)),
                ),
        )
    }
}

fn channel_type_icon(channel_type: ChannelType, private: bool) -> IconName {
    match (channel_type, private) {
        (ChannelType::Text, false) => IconName::Hashtag,
        (ChannelType::Text, true) => IconName::HashtagLocked,
        (ChannelType::Voice, false) => IconName::Speaker,
        (ChannelType::Voice, true) => IconName::SpeakerLocked,
        (ChannelType::Stream, _) => IconName::Stream,
        (ChannelType::Thread, _) => IconName::ThreadIcon,
        (ChannelType::Forum, _) => IconName::Forum,
        (ChannelType::Announcement, _) => IconName::Announcement,
        (ChannelType::App, false) => IconName::AppChannelIcon,
        (ChannelType::App, true) => IconName::PrivateAppChannelIcon,
        (ChannelType::Unknown(_), _) => IconName::Hashtag,
    }
}
