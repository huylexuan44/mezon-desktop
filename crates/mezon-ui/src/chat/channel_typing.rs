use gpui::{Context, Entity, Render, SharedString, Subscription, Window, div, prelude::*, px};
use mezon_store::{ChannelId, PresenceEvent, PresenceStore, Settings};

use crate::theme::ActiveTheme;

pub struct ChannelTyping {
    channel_id: Option<ChannelId>,
    settings: Entity<Settings>,
    _presence_sub: Subscription,
    _settings_sub: Subscription,
}

impl ChannelTyping {
    pub fn new(settings: &Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let _presence_sub = cx.subscribe(&PresenceStore::global(cx), |this, _, event, cx| {
            if let PresenceEvent::TypingChanged { channel_id } = event
                && this.channel_id == Some(*channel_id)
            {
                cx.notify();
            }
        });
        let _settings_sub = cx.observe(settings, |_, _, cx| cx.notify());
        Self {
            channel_id: None,
            settings: settings.clone(),
            _presence_sub,
            _settings_sub,
        }
    }

    pub fn sync(&mut self, channel_id: Option<ChannelId>, cx: &mut Context<Self>) {
        if self.channel_id == channel_id {
            return;
        }
        self.channel_id = channel_id;
        cx.notify();
    }

    fn label(&self, cx: &Context<Self>) -> Option<SharedString> {
        let channel_id = self.channel_id?;
        let presence = PresenceStore::global(cx);
        let presence = presence.read(cx);
        let users = presence.typing_users(channel_id);
        let locale = self.settings.read(cx).language.clone();
        let label = match users.len() {
            0 => return None,
            1 => {
                let name = users.iter().next()?;
                format!("{name} {}", mezon_i18n::t(&locale, "common.isTyping"))
            }
            _ => mezon_i18n::t(&locale, "common.severalPeopleTyping").to_string(),
        };
        Some(SharedString::from(label))
    }
}

impl Render for ChannelTyping {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .mx_3()
            .h(px(16.))
            .flex()
            .flex_row()
            .items_center()
            .gap_1p5()
            .overflow_hidden()
            .text_xs()
            .text_color(theme.text_primary)
            .when_some(self.label(cx), |d, label| d.child(label))
    }
}
