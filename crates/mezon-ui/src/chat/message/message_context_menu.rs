use gpui::{App, ClipboardItem, SharedString, WeakEntity, Window};
use mezon_store::{Message, MessageCode, MessagesStore, PinnedMessagesStore};

use super::channel_messages::ChannelMessages;
use super::content::{first_link, open_message_link};
use crate::app::shell::Shell;
use crate::components::primitives::{ContextMenu, IconName};

fn coming_soon_click(message: SharedString) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_window: &mut Window, cx: &mut App| {
        let message = message.clone();
        Shell::global(cx).update(cx, move |shell, cx| shell.info(message, cx));
    }
}

/// Builds the "···" / right-click message context menu. Real items reuse existing
/// store actions; items that depend on not-yet-ported subsystems (forward, create
/// thread, polls, topic discussion, give-a-coffee, report, quick menus, mark
/// unread/inbox) are shown for layout parity but only toast "coming soon".
pub(crate) fn build(
    msg: &Message,
    current_user_id: &str,
    is_clan_owner: bool,
    locale: &str,
    host: WeakEntity<ChannelMessages>,
    cx: &App,
) -> ContextMenu {
    let t = |key: &'static str| mezon_i18n::t(locale, key).to_string();
    let coming_soon_msg: SharedString = t("common.comingSoon").into();
    let is_own_message = current_user_id == msg.sender_id.as_str();
    let is_poll = msg.code == MessageCode::Poll;
    let sender_is_real = !msg.sender_id.is_empty() && msg.sender_id.as_str() != "0";
    let is_pinned = PinnedMessagesStore::global(cx)
        .read(cx)
        .is_pinned(&msg.id.to_string());

    let dismiss = {
        let host = host.clone();
        move |_window: &mut Window, cx: &mut App| {
            if let Some(view) = host.upgrade() {
                view.update(cx, |this, cx| this.close_context_menu(cx));
            }
        }
    };

    let mut menu = ContextMenu::new().on_dismiss(dismiss);

    {
        let host = host.clone();
        let message_id = msg.id;
        menu = menu.item_icon(
            t("contextMenu.addReaction"),
            IconName::Smile,
            move |window, cx| {
                let position = window.mouse_position();
                let _ = host.update(cx, |this, cx| {
                    this.open_reaction_picker(message_id, position, window, cx);
                });
            },
        );
    }

    if !is_own_message && sender_is_real {
        menu = menu.item_icon(
            t("contextMenu.giveACoffee"),
            IconName::DollarIconRightClick,
            coming_soon_click(coming_soon_msg.clone()),
        );
    }

    let show_edit = is_own_message
        && msg.code != MessageCode::SendToken
        && msg.code.is_user_timeline()
        && !is_poll
        && !msg.is_forwarded;
    if show_edit {
        let host = host.clone();
        let message_id = msg.id;
        menu = menu.item_icon(
            t("contextMenu.editMessage"),
            IconName::PenEdit,
            move |window, cx| {
                let _ = host.update(cx, |this, cx| {
                    this.begin_edit(message_id, window, cx);
                });
            },
        );
    }

    if is_pinned {
        let message_id = msg.id;
        menu = menu.item_icon(
            t("contextMenu.unpinMessage"),
            IconName::PinMessageRightClick,
            move |_window, cx| {
                let message_id_str = message_id.to_string();
                if let Some(pin_id) = PinnedMessagesStore::global(cx)
                    .read(cx)
                    .pinned()
                    .iter()
                    .find(|p| p.message_id == message_id_str)
                    .map(|p| p.id.clone())
                {
                    PinnedMessagesStore::global(cx)
                        .update(cx, |store, cx| store.unpin(&pin_id, &message_id_str, cx));
                }
            },
        );
    }

    menu = menu.separator();

    {
        let message_id = msg.id;
        menu = menu.item_icon(
            t("contextMenu.reply"),
            IconName::ReplyRightClick,
            move |_, cx| {
                MessagesStore::global(cx)
                    .update(cx, |store, cx| store.set_reply_to(message_id, cx));
            },
        );
    }

    if !is_poll {
        menu = menu
            .item(
                t("contextMenu.forwardMessage"),
                coming_soon_click(coming_soon_msg.clone()),
            )
            .item_icon(
                t("contextMenu.forwardAllMessage"),
                IconName::ForwardAllRightClick,
                coming_soon_click(coming_soon_msg.clone()),
            )
            .item_icon(
                t("contextMenu.createThread"),
                IconName::ThreadIcon,
                coming_soon_click(coming_soon_msg.clone()),
            );
    }

    if !msg.content.is_empty() && !is_poll {
        let content = msg.content.clone();
        menu = menu.item_icon(
            t("contextMenu.copyText"),
            IconName::CopyTextRightClick,
            move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(content.clone()));
            },
        );
    }

    if !is_pinned {
        let message_id = msg.id;
        menu = menu.item_icon(
            t("contextMenu.pinMessage"),
            IconName::PinMessageRightClick,
            move |_window, cx| {
                PinnedMessagesStore::global(cx)
                    .update(cx, |store, cx| store.pin(&message_id.to_string(), cx));
            },
        );
    }

    if is_poll && is_own_message {
        menu = menu.item_icon(
            t("contextMenu.endPollNow"),
            IconName::EndPollNowIcon,
            coming_soon_click(coming_soon_msg.clone()),
        );
    }

    if !is_poll {
        menu = menu.item_icon(
            t("contextMenu.topicDiscussion"),
            IconName::TopicIcon,
            coming_soon_click(coming_soon_msg.clone()),
        );
    }

    menu = menu
        .item_icon(
            t("contextMenu.markUnread"),
            IconName::MarkUnreadIcon,
            coming_soon_click(coming_soon_msg.clone()),
        )
        .item_icon(
            t("contextMenu.addToInbox"),
            IconName::AddToInboxIcon,
            coming_soon_click(coming_soon_msg.clone()),
        )
        .item_icon(
            t("contextMenu.quickMenus"),
            IconName::QuickMenusIcon,
            coming_soon_click(coming_soon_msg.clone()),
        );

    let link = first_link(msg);
    let image_url = msg
        .attachments
        .iter()
        .find(|a| a.is_image())
        .map(|a| a.url.clone());
    if link.is_some() || image_url.is_some() || !is_own_message {
        menu = menu.separator();
    }
    if let Some(link) = link {
        let link_for_copy = link.clone();
        menu = menu.item(t("contextMenu.copyLink"), move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(link_for_copy.clone()));
        });
        menu = menu.item(t("contextMenu.openLink"), move |_, cx| {
            open_message_link(link.clone(), cx);
        });
    }
    if !is_own_message {
        menu = menu.item_icon(
            t("contextMenu.reportMessage"),
            IconName::ReportMessageRightClick,
            coming_soon_click(coming_soon_msg.clone()),
        );
    }
    if let Some(image_url) = image_url {
        let url_for_copy = image_url.clone();
        menu = menu.item(t("contextMenu.copyImage"), move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(url_for_copy.clone()));
        });
        menu = menu.item(t("contextMenu.saveImage"), move |_, cx| {
            open_message_link(image_url.clone(), cx);
        });
    }

    if is_own_message || is_clan_owner {
        let message_id = msg.id;
        let locale_owned = locale.to_string();
        menu = menu.separator().danger_item_icon(
            t("contextMenu.deleteMessage"),
            IconName::DeleteMessageRightClick,
            move |window, cx| {
                let locale = locale_owned.clone();
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.confirm_delete_message(message_id, &locale, window, cx);
                });
            },
        );
    }

    menu
}
