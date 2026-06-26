use gpui::{AnyView, App, Context, Entity, StyleRefinement, Window, div, prelude::*, px};
use mezon_store::{
    AuthState, ChannelId, ChannelList, ChannelType, ClanId, ClanList, DirectChannel, DirectKind,
    DirectMessageStore, GroupMembersStore, InboxStore, MessagesStore, PresenceEvent, PresenceStore,
    Settings, VoiceStore,
};
use ui::PopoverMenuHandle;

use crate::chat::area::ChatArea;
use crate::chat::inbox::InboxPopoverPanel;
use crate::components::compositions::user_info_bar::UserInfoBar;
use crate::router::{Route, Router};
use crate::theme::{ActiveTheme, Theme};
use crate::{ChannelSidebar, ClanSidebar, DirectSidebar};

pub struct ChatLayout {
    pub(crate) channel_list: Entity<ChannelList>,
    pub chat_area: ChatArea,
    clan_sidebar: Entity<ClanSidebar>,
    channel_sidebar: Entity<ChannelSidebar>,
    direct_sidebar: Entity<DirectSidebar>,
    direct_store: Entity<DirectMessageStore>,
    user_info_bar: Entity<UserInfoBar>,
    clan_list: Entity<ClanList>,
    auth_state: Entity<AuthState>,
    settings: Entity<Settings>,
    voice_store: Entity<VoiceStore>,
    pending_channel_id: Option<ChannelId>,
    show_member_list: bool,
    inbox_handle: PopoverMenuHandle<InboxPopoverPanel>,
}

impl ChatLayout {
    pub fn new(
        clan_list: Entity<ClanList>,
        auth_state: Entity<AuthState>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();

        let channel_list = ChannelList::global(cx);

        let clan_list_for_sidebar = clan_list.clone();
        let settings_for_clan = settings.clone();
        let clan_sidebar =
            cx.new(move |cx| ClanSidebar::new(clan_list_for_sidebar, settings_for_clan, cx));

        let clan_list_for_channel = clan_list.clone();
        let channel_list_for_channel = channel_list.clone();
        let settings_for_channel = settings.clone();
        let channel_sidebar = cx.new(move |cx| {
            ChannelSidebar::new(
                clan_list_for_channel,
                channel_list_for_channel,
                settings_for_channel,
                cx,
            )
        });

        let settings_for_direct = settings.clone();
        let direct_sidebar = cx.new(move |cx| DirectSidebar::new(settings_for_direct, cx));

        let user_info_bar = cx.new(|cx| UserInfoBar::new(auth_state.clone(), cx));

        cx.subscribe(&PresenceStore::global(cx), |this, _, event, cx| {
            if let PresenceEvent::TypingChanged { channel_id } = event
                && this.active_typing_channel(cx) == Some(*channel_id)
            {
                cx.notify();
            }
        })
        .detach();

        let direct_store = DirectMessageStore::global(cx);
        cx.observe(&direct_store, |_, _, cx| cx.notify()).detach();

        let messages_store = MessagesStore::global(cx);
        cx.observe(&messages_store, |_, _, cx| cx.notify()).detach();

        let voice_store = VoiceStore::global(cx);
        cx.observe(&voice_store, |_, _, cx| cx.notify()).detach();

        let chat_area = ChatArea::new(settings.clone(), cx);
        cx.observe(&channel_list, |this, _, cx| {
            this.apply_pending_channel(cx);
            this.ensure_active_channel_for_clan(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&Router::global(cx), |this, _, cx| {
            this.sync_active_from_route(cx);
            this.dismiss_inbox_popover(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&clan_list, |this, _, cx| {
            this.sync_inbox_context(cx);
            this.dismiss_inbox_popover(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&channel_list, |this, _, cx| {
            this.sync_inbox_context(cx);
            this.dismiss_inbox_popover(cx);
        })
        .detach();
        let mut this = Self {
            channel_list,
            clan_sidebar,
            channel_sidebar,
            direct_sidebar,
            direct_store,
            user_info_bar,
            clan_list,
            auth_state,
            chat_area,
            settings,
            voice_store,
            pending_channel_id: None,
            show_member_list: true,
            inbox_handle: PopoverMenuHandle::default(),
        };
        this.sync_active_from_route(cx);
        this.sync_inbox_context(cx);
        this
    }

    pub(crate) fn toggle_member_list(&mut self, cx: &mut Context<Self>) {
        self.show_member_list = !self.show_member_list;
        cx.notify();
    }

    fn dismiss_inbox_popover(&self, cx: &mut App) {
        self.inbox_handle.hide(cx);
    }

    fn sync_inbox_context(&self, cx: &mut Context<Self>) {
        let clan_id = self
            .clan_list
            .read(cx)
            .active_clan_id
            .map(|id| id.to_string());
        let channel_id = self
            .channel_list
            .read(cx)
            .active_channel_id
            .map(|id| id.to_string());
        InboxStore::global(cx).update(cx, |store, cx| {
            store.set_active_context(clan_id, channel_id, cx);
        });
    }

    fn active_clan_id(&self, cx: &Context<Self>) -> Option<String> {
        self.clan_list
            .read(cx)
            .active_clan_id
            .map(|id| id.to_string())
    }

    fn sync_active_from_route(&mut self, cx: &mut Context<Self>) {
        match Router::global(cx).read(cx).route() {
            Route::Channel {
                clan_id,
                channel_id,
            } => self.sync_channel_route(clan_id, channel_id, cx),
            Route::Thread {
                clan_id,
                channel_id,
                ..
            } => self.sync_channel_route(clan_id, channel_id, cx),
            Route::Canvas {
                clan_id,
                channel_id,
                ..
            } => self.sync_channel_route(clan_id, channel_id, cx),
            Route::DirectMessage {
                direct_id,
                message_type,
            } => {
                self.pending_channel_id = None;
                self.direct_store
                    .update(cx, |store, cx| store.ensure_loaded(cx));
                let channel_type = message_type.parse::<i32>().unwrap_or_else(|_| {
                    tracing::warn!(
                        "DM route: non-numeric message_type {:?}, defaulting to 3",
                        message_type
                    );
                    3
                });
                if channel_type == DirectKind::Group.channel_type() {
                    self.chat_area.bind_group_members(cx);
                    let group_id = direct_id;
                    GroupMembersStore::global(cx)
                        .update(cx, |store, cx| store.ensure_loaded(group_id, cx));
                } else {
                    self.chat_area.clear_member_panel();
                }
                MessagesStore::global(cx).update(cx, |store, cx| {
                    store.open_direct(direct_id, channel_type, cx)
                });
            }
            Route::Direct | Route::Friends => {
                self.pending_channel_id = None;
                self.direct_store
                    .update(cx, |store, cx| store.ensure_loaded(cx));
                self.chat_area.clear_member_panel();
            }
            _ => {
                self.pending_channel_id = None;
                self.chat_area.clear_member_panel();
            }
        }
    }

    fn sync_channel_route(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.chat_area.bind_channel_members(cx);
        if self.clan_list.read(cx).active_clan_id != Some(clan_id) {
            self.clan_list
                .update(cx, |clan_list, cx| clan_list.select_clan(clan_id, cx));
        }
        let (present, already_active) = {
            let channels = self.channel_list.read(cx);
            (
                channels.find_channel(channel_id).is_some(),
                channels.active_channel_id == Some(channel_id),
            )
        };
        if present {
            self.pending_channel_id = None;
            if !already_active {
                self.channel_list.update(cx, |channel_list, cx| {
                    channel_list.select_channel(channel_id, cx);
                });
            }
            // Force the messages store onto this channel even when it's already the active
            // `ChannelList` selection — covers returning from a DM (where `ChannelList` never
            // re-emits) so the timeline switches back instead of showing the DM.
            MessagesStore::global(cx).update(cx, |store, cx| store.open_channel(channel_id, cx));
        } else {
            self.pending_channel_id = Some(channel_id);
        }
    }

    fn apply_pending_channel(&mut self, cx: &mut Context<Self>) {
        let Some(channel_id) = self.pending_channel_id else {
            return;
        };
        if self
            .channel_list
            .read(cx)
            .find_channel(channel_id)
            .is_some()
        {
            self.pending_channel_id = None;
            self.channel_list.update(cx, |channel_list, cx| {
                channel_list.select_channel(channel_id, cx);
            });
        }
    }

    fn ensure_active_channel_for_clan(&mut self, cx: &mut Context<Self>) {
        if matches!(
            Router::global(cx).read(cx).route(),
            Route::Direct | Route::Friends | Route::DirectMessage { .. }
        ) {
            return;
        }
        let Some(clan_id) = self.clan_list.read(cx).active_clan_id else {
            return;
        };

        if let Route::Channel {
            clan_id: route_clan,
            channel_id,
        } = Router::global(cx).read(cx).route()
            && route_clan == clan_id
            && self
                .channel_list
                .read(cx)
                .channel_in_clan(clan_id, channel_id)
        {
            return;
        }

        let welcome = self.clan_list.read(cx).welcome_channel_id(clan_id);
        let target = {
            let channels = self.channel_list.read(cx);
            welcome
                .filter(|w| channels.channel_in_clan(clan_id, *w))
                .or_else(|| channels.default_channel_id(clan_id))
        };
        let Some(channel_id) = target else {
            return;
        };

        crate::router::navigate(
            cx,
            Route::Channel {
                clan_id,
                channel_id,
            },
        );
    }
}

impl Render for ChatLayout {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("ChatLayout");
        self.chat_area.ensure_input(window, cx);

        if let Some(ch) = self.channel_list.read(cx).active_channel()
            && ch.channel_type == ChannelType::Voice
        {
            let channel_id = ch.id.clone();
            self.voice_store.update(cx, |store, cx| {
                store.prefetch_meet_token(channel_id.to_string(), cx);
            });
        }

        let theme = cx.theme();
        let nav_body = self.render_nav_body(cx);
        let voice_mini_bar = self.render_voice_mini_bar(cx);
        let content = self.render_content(window, cx);

        div()
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .h_full()
            .min_h_0()
            .bg(theme.bg_primary)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(344.0))
                    .h_full()
                    .relative()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_h_0()
                            .child(
                                div().w(px(72.0)).h_full().child(
                                    AnyView::from(self.clan_sidebar.clone())
                                        .cached(StyleRefinement::default().size_full()),
                                ),
                            )
                            .child(div().w(px(272.0)).h_full().child(nav_body)),
                    )
                    .children(voice_mini_bar)
                    .child(
                        AnyView::from(self.user_info_bar.clone()).cached(
                            StyleRefinement::default()
                                .absolute()
                                .left(px(12.0))
                                .right(px(8.0))
                                .bottom(px(12.0)),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    .bg(theme.bg_primary)
                    .child(content),
            )
    }
}

impl ChatLayout {
    pub(crate) fn send_current_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.chat_area.input_state.clone() else {
            return;
        };
        let content = input.read(cx).value().trim().to_string();
        if content.is_empty() {
            return;
        }
        input.update(cx, |state, cx| state.set_value("", window, cx));

        let (uid, uname) = match self.auth_state.read(cx) {
            AuthState::Authenticated(session) => {
                (session.user_id.clone(), session.username.clone())
            }
            _ => (String::new(), String::new()),
        };

        MessagesStore::global(cx).update(cx, |store, cx| {
            store.send_message(content, uid, uname, cx);
        });
    }

    fn current_dm(&self, cx: &Context<Self>) -> Option<DirectChannel> {
        let Route::DirectMessage { direct_id, .. } = Router::global(cx).read(cx).route() else {
            return None;
        };
        self.direct_store.read(cx).find(direct_id).cloned()
    }

    fn is_dm_route(&self, cx: &Context<Self>) -> bool {
        matches!(
            Router::global(cx).read(cx).route(),
            Route::Direct | Route::Friends | Route::DirectMessage { .. }
        )
    }

    fn active_typing_channel(&self, cx: &Context<Self>) -> Option<ChannelId> {
        if self.is_dm_route(cx) {
            self.current_dm(cx).map(|dm| dm.id)
        } else {
            self.channel_list.read(cx).active_channel().map(|ch| ch.id)
        }
    }

    fn render_voice_mini_bar(&self, cx: &Context<Self>) -> Option<gpui::AnyElement> {
        let store = self.voice_store.read(cx);
        store.connection().connected_channel()?;
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        Some(crate::chat::voice::render_mini_bar(
            theme,
            &locale,
            store.channel_label(),
            &self.voice_store,
            store.mic_enabled(),
        ))
    }

    fn render_nav_body(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let view: AnyView = if self.is_dm_route(cx) {
            self.direct_sidebar.clone().into()
        } else {
            self.channel_sidebar.clone().into()
        };
        view.cached(StyleRefinement::default().size_full())
            .into_any_element()
    }

    fn typing_label_for_channel(
        &self,
        channel_id: ChannelId,
        locale: &str,
        cx: &Context<Self>,
    ) -> Option<gpui::SharedString> {
        let users = PresenceStore::global(cx).read(cx).typing_users(channel_id);
        let label = match users.len() {
            0 => return None,
            1 => format!("{} {}", users[0], mezon_i18n::t(locale, "common.isTyping")),
            _ => mezon_i18n::t(locale, "common.severalPeopleTyping").to_string(),
        };
        Some(gpui::SharedString::from(label))
    }

    fn render_content(&self, window: &mut Window, cx: &Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let inbox_handle = self.inbox_handle.clone();
        let active_clan_id = self.active_clan_id(cx);
        let clan_id = active_clan_id.as_deref();

        if self.is_dm_route(cx) {
            if let Some(dm) = self.current_dm(cx) {
                let typing = self.typing_label_for_channel(dm.id, &locale, cx);
                let is_group = dm.kind == DirectKind::Group;
                return self
                    .chat_area
                    .render(
                        theme,
                        &locale,
                        cx.entity(),
                        &dm.label,
                        true,
                        is_group,
                        is_group && self.show_member_list,
                        typing,
                        None,
                        None,
                        window,
                        cx,
                    )
                    .into_any_element();
            }
            return div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .flex_col()
                .gap_4()
                .child(
                    crate::components::primitives::Icon::new(
                        crate::components::primitives::IconName::People,
                    )
                    .size_8()
                    .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .text_base()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text_primary)
                        .child(mezon_i18n::t(&locale, "dm.emptyState")),
                )
                .into_any_element();
        }

        if let Some(ch) = self.channel_list.read(cx).active_channel() {
            if ch.channel_type == ChannelType::Voice {
                let channel = ch.clone();
                let (input_device_id, output_device_id) = {
                    let settings = self.settings.read(cx);
                    (
                        settings.input_device_id.clone(),
                        settings.output_device_id.clone(),
                    )
                };
                return crate::chat::voice::render_voice_channel(
                    theme,
                    &locale,
                    &channel,
                    &self.voice_store,
                    &self.settings,
                    input_device_id,
                    output_device_id,
                    cx,
                );
            }

            let channel_name = ch.name.clone();
            let typing = self.typing_label_for_channel(ch.id, &locale, cx);
            return self
                .chat_area
                .render(
                    theme,
                    &locale,
                    cx.entity(),
                    &channel_name,
                    false,
                    true,
                    self.show_member_list,
                    typing,
                    clan_id,
                    Some(inbox_handle),
                    window,
                    cx,
                )
                .into_any_element();
        }

        let router = Router::global(cx);
        let route = router.read(cx).route();
        let current_path = router.read(cx).current_path();

        let placeholder = match route {
            Route::Chat => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::Inbox,
                mezon_i18n::t(&locale, "nav.chat"),
                &current_path,
            ),
            Route::Direct => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::People,
                mezon_i18n::t(&locale, "dm.title"),
                &current_path,
            ),
            Route::DirectMessage {
                direct_id,
                message_type: _,
            } => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::People,
                &format!("Direct {direct_id}"),
                &current_path,
            ),
            Route::Channel { .. } => div().into_any_element(),
            Route::Friends => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::IconFriends,
                mezon_i18n::t(&locale, "directMessage.friends"),
                &current_path,
            ),
            Route::Thread { channel_id, .. } => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::Hashtag,
                &format!("Thread #{channel_id}"),
                &current_path,
            ),
            Route::Canvas { channel_id, .. } => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::Hashtag,
                &format!("Canvas #{channel_id}"),
                &current_path,
            ),
            Route::AddFriend { username } => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::People,
                &format!("Add Friend: {username}"),
                &current_path,
            ),
            Route::Invite { invite_id } => self.render_placeholder(
                theme,
                crate::components::primitives::IconName::People,
                &format!("Invite: {invite_id}"),
                &current_path,
            ),
            Route::SettingsAccount
            | Route::SettingsProfile
            | Route::SettingsDevices
            | Route::SettingsAppearance
            | Route::SettingsActivity
            | Route::SettingsNotifications
            | Route::SettingsLanguage
            | Route::SettingsVoice
            | Route::SettingsAdvanced
            | Route::NotFound { .. } => div().into_any_element(),
        };

        div()
            .flex_1()
            .min_h_0()
            .p_6()
            .child(placeholder)
            .into_any_element()
    }

    fn render_placeholder(
        &self,
        theme: &Theme,
        icon: crate::components::primitives::IconName,
        title: &str,
        _path: &str,
    ) -> gpui::AnyElement {
        use crate::components::primitives::Icon;

        div()
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .flex_col()
            .gap_4()
            .child(Icon::new(icon).size_8().text_color(theme.text_muted))
            .child(
                div()
                    .text_base()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(title.to_string()),
            )
            .into_any_element()
    }
}
