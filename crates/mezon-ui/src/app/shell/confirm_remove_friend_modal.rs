use std::ops::Range;

use gpui::{
    Context, FocusHandle, FontWeight, HighlightStyle, SharedString, StyledText, Window, div,
    prelude::*, px,
};
use mezon_store::{FriendStore, UserId};

use super::Shell;
use crate::components::primitives::{Button, ButtonVariants, h_flex, v_flex};
use crate::theme::ActiveTheme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FriendRemovalKind {
    RemoveFriend,
    CancelRequest,
    RejectRequest,
}

impl FriendRemovalKind {
    pub(super) fn title_key(self) -> &'static str {
        match self {
            Self::RemoveFriend => "friendsPage.removeFriendModal.title",
            Self::CancelRequest => "friendsPage.cancelRequestModal.title",
            Self::RejectRequest => "friendsPage.rejectRequestModal.title",
        }
    }

    pub(super) fn description_key(self) -> &'static str {
        match self {
            Self::RemoveFriend => "friendsPage.removeFriendModal.description",
            Self::CancelRequest => "friendsPage.cancelRequestModal.description",
            Self::RejectRequest => "friendsPage.rejectRequestModal.description",
        }
    }

    pub(super) fn confirm_key(self) -> &'static str {
        match self {
            Self::RemoveFriend => "friendsPage.removeFriendModal.confirm",
            Self::CancelRequest => "friendsPage.cancelRequestModal.confirm",
            Self::RejectRequest => "friendsPage.rejectRequestModal.confirm",
        }
    }
}

pub(super) fn interpolate_username(
    template: &str,
    username: &str,
) -> (SharedString, Option<Range<usize>>) {
    const OPEN: &str = "<bold>";
    const CLOSE: &str = "</bold>";
    let filled = template.replace("{{username}}", username);
    let Some(open) = filled.find(OPEN) else {
        return (filled.into(), None);
    };
    let Some(close) = filled.find(CLOSE) else {
        return (filled.into(), None);
    };
    if close < open + OPEN.len() {
        return (filled.into(), None);
    }
    let inner = &filled[open + OPEN.len()..close];
    let text = format!(
        "{}{}{}",
        &filled[..open],
        inner,
        &filled[close + CLOSE.len()..]
    );
    let range = open..open + inner.len();
    (text.into(), Some(range))
}

pub(super) struct ConfirmRemoveFriendModal {
    pub(super) focus_handle: FocusHandle,
    pub(super) friend_id: UserId,
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) description_bold: Option<Range<usize>>,
    pub(super) cancel_label: SharedString,
    pub(super) confirm_label: SharedString,
}

impl Render for ConfirmRemoveFriendModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let friend_id = self.friend_id;

        let highlights = self
            .description_bold
            .clone()
            .map(|range| {
                (
                    range,
                    HighlightStyle {
                        font_weight: Some(FontWeight::SEMIBOLD),
                        color: Some(theme.text_primary.into()),
                        ..Default::default()
                    },
                )
            })
            .into_iter()
            .collect::<Vec<_>>();

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .w(px(420.))
            .gap_2()
            .p(px(24.))
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .truncate()
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_secondary)
                    .child(StyledText::new(self.description.clone()).with_highlights(highlights)),
            )
            .child(
                h_flex()
                    .pt(px(12.))
                    .justify_end()
                    .gap_3()
                    .child(
                        Button::new("remove-friend-cancel")
                            .label(self.cancel_label.clone())
                            .ghost()
                            .on_click(|_, _window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(
                        Button::new("remove-friend-confirm")
                            .label(self.confirm_label.clone())
                            .danger()
                            .on_click(move |_, _window, cx| {
                                FriendStore::global(cx).update(cx, |store, cx| {
                                    store.delete_friend(friend_id, cx);
                                });
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::interpolate_username;

    #[test]
    fn interpolate_extracts_bold_span() {
        let (text, range) = interpolate_username(
            "Are you sure you want to remove <bold>{{username}}</bold> from your friends?",
            "ngoc",
        );
        assert_eq!(
            text.as_ref(),
            "Are you sure you want to remove ngoc from your friends?"
        );
        let range = range.expect("bold range");
        assert_eq!(&text[range], "ngoc");
    }

    #[test]
    fn interpolate_without_bold_tags_returns_plain_text() {
        let (text, range) = interpolate_username("Remove '{{username}}'", "ngoc");
        assert_eq!(text.as_ref(), "Remove 'ngoc'");
        assert!(range.is_none());
    }
}
