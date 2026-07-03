use gpui::{FontWeight, SharedString, div, prelude::*, px};
use mezon_store::ChannelType;

use crate::components::primitives::Icon;
use crate::components::primitives::IconName;
use crate::components::primitives::mention_count_badge_on_channel_row;
use crate::theme::Theme;

pub struct ChannelRow {
    row_id: SharedString,
    name: SharedString,
    channel_type: ChannelType,
    unread: bool,
    private: bool,
    selected: bool,
    badge_count: u32,
    muted: bool,
    is_thread: bool,
    suppress_hover: bool,
}

impl ChannelRow {
    pub fn new(name: impl Into<SharedString>, channel_type: ChannelType) -> Self {
        Self {
            row_id: SharedString::default(),
            name: name.into(),
            channel_type,
            unread: false,
            private: false,
            selected: false,
            badge_count: 0,
            muted: false,
            is_thread: false,
            suppress_hover: false,
        }
    }

    pub fn row_id(mut self, row_id: impl Into<SharedString>) -> Self {
        self.row_id = row_id.into();
        self
    }

    pub fn suppress_hover(mut self, suppress: bool) -> Self {
        self.suppress_hover = suppress;
        self
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
        self
    }

    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    pub fn render(self, theme: &Theme) -> impl IntoElement {
        let icon = channel_type_icon(self.channel_type, self.private);
        let highlight_type = shows_left_unread_nub(self.channel_type);
        let text_bright =
            self.selected || ((self.unread || self.badge_count > 0) && highlight_type);
        let bold = (self.selected || self.unread) && highlight_type;
        let icon_active = self.selected || self.unread || self.badge_count > 0;

        let text_base = if text_bright {
            theme.tokens.text_secondary
        } else {
            theme.tokens.text_theme_primary
        };
        let text_hover = theme.tokens.text_secondary;
        let icon_base = if icon_active {
            theme.tokens.bg_icon_theme_active
        } else {
            theme.tokens.bg_icon_theme
        };
        let icon_hover = theme.tokens.bg_icon_theme_active;
        let name_weight = if bold {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        };

        let selected_bg = theme.tokens.bg_active_member_channel;
        let hover_bg = theme.bg_hover;
        let show_badge = crate::SHOW_UNREAD_BADGE_COUNT && self.badge_count > 0;
        let group_name = self.row_id.clone();
        let unread_nub = self.unread && !self.selected && highlight_type;
        let nub_color = theme.tokens.text_secondary;

        let muted = self.muted;
        let selected = self.selected;
        let hoverable = !self.selected && !self.suppress_hover;
        div()
            .w_full()
            .relative()
            .group(group_name.clone())
            .when(unread_nub, |el| {
                el.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(12.))
                        .w(px(4.))
                        .h(px(8.))
                        .rounded_r(px(4.))
                        .bg(nub_color),
                )
            })
            .when(show_badge, |el| {
                el.child(mention_count_badge_on_channel_row(
                    self.badge_count,
                    group_name.clone(),
                ))
            })
            .px_2()
            .child(
                div()
                    .id(self.row_id.clone())
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .when(selected, move |el| el.bg(selected_bg))
                    .when(hoverable, move |el| el.hover(|s| s.bg(hover_bg)))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .flex_1()
                            .min_w_0()
                            .when(muted, |el| el.opacity(0.7))
                            .child(Icon::new(icon).size(px(16.0)).text_color(icon_base).when(
                                hoverable,
                                |ic| {
                                    ic.group_hover(group_name.clone(), move |s| {
                                        s.text_color(icon_hover)
                                    })
                                },
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .ml_2()
                                    .mr_6()
                                    .text_base()
                                    .overflow_hidden()
                                    .text_color(text_base)
                                    .font_weight(name_weight)
                                    .when(hoverable, |el| {
                                        el.group_hover(group_name.clone(), move |s| {
                                            s.text_color(text_hover)
                                        })
                                    })
                                    .when(muted, |el| el.opacity(0.7))
                                    .child(self.name),
                            ),
                    ),
            )
    }
}

fn shows_left_unread_nub(channel_type: ChannelType) -> bool {
    !matches!(
        channel_type,
        ChannelType::Voice | ChannelType::Stream | ChannelType::App | ChannelType::Unknown(_)
    )
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
