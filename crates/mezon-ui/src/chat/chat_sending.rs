use gpui::{App, Entity};
use mezon_store::{AuthState, MessagesStore, OutgoingAttachment, OutgoingContent};

pub struct ChatSending;

impl ChatSending {
    fn current_user(auth_state: &Entity<AuthState>, cx: &App) -> (String, String) {
        match auth_state.read(cx) {
            AuthState::Authenticated(session) => {
                (session.user_id.clone(), session.username.clone())
            }
            _ => (String::new(), String::new()),
        }
    }

    pub fn send_text(
        content: impl Into<String>,
        content_tokens: OutgoingContent,
        attachments: Vec<OutgoingAttachment>,
        auth_state: &Entity<AuthState>,
        cx: &mut App,
    ) {
        let content = content.into();
        if content.is_empty() && content_tokens.is_empty() && attachments.is_empty() {
            return;
        }
        let (uid, uname) = Self::current_user(auth_state, cx);
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.send_message(content, uid, uname, content_tokens, attachments, cx);
        });
    }

    pub fn send_sticker(
        url: String,
        filename: String,
        auth_state: &Entity<AuthState>,
        cx: &mut App,
    ) {
        if url.is_empty() {
            return;
        }
        let (uid, uname) = Self::current_user(auth_state, cx);
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.send_sticker(url, filename, uid, uname, cx);
        });
    }

    pub fn send_sound(url: String, filename: String, auth_state: &Entity<AuthState>, cx: &mut App) {
        if url.is_empty() {
            return;
        }
        let (uid, uname) = Self::current_user(auth_state, cx);
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.send_sound(url, filename, uid, uname, cx);
        });
    }
}
