use gpui::{AnyView, App, Context, Entity, StyleRefinement, Window, div, prelude::*, px};
use mezon_store::{
    AuthState, ChannelId, ChannelList, ChannelType, ClanId, ClanList, DirectChannel, DirectKind,
    DirectMessageStore, GroupMembersStore, MessagesStore, Settings, ThreadsEvent, ThreadsStore,
    VoiceStore,
};
use ui::PopoverMenuHandle;

use crate::app::shell::Shell;
use crate::chat::area::ChatArea;
use crate::chat::threads_popover::ThreadsPopoverPanel;
use crate::components::compositions::user_info_bar::UserInfoBar;
use crate::components::primitives::{InputEvent, InputState};
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
    prefetched_voice_channel: Option<ChannelId>,
    show_member_list: bool,
    pub(crate) thread_popover_handle: PopoverMenuHandle<ThreadsPopoverPanel>,
    pub(crate) thread_search_input: Option<Entity<InputState>>,
    thread_name_input: Option<Entity<InputState>>,
    create_thread_message_input: Option<Entity<InputState>>,
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

        let direct_store = DirectMessageStore::global(cx);
        cx.observe(&direct_store, |_, _, cx| cx.notify()).detach();

        let voice_store = VoiceStore::global(cx);
        cx.observe(&voice_store, |_, _, cx| cx.notify()).detach();

        let threads_store = ThreadsStore::global(cx);
        cx.observe(&threads_store, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&threads_store, |this, _, event, cx| {
            this.on_threads_event(event, cx);
        })
        .detach();

        let chat_area = ChatArea::new(settings.clone(), cx);
        cx.observe(&channel_list, |this, _, cx| {
            this.apply_pending_channel(cx);
            this.ensure_active_channel_for_clan(cx);
            this.dismiss_threads_popover(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&Router::global(cx), |this, _, cx| {
            if matches!(
                Router::global(cx).read(cx).route(),
                Route::Direct | Route::Friends | Route::DirectMessage { .. }
            ) {
                this.dismiss_threads_popover(cx);
            }
            this.sync_active_from_route(cx);
            cx.notify();
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
            prefetched_voice_channel: None,
            show_member_list: true,
            thread_popover_handle: PopoverMenuHandle::default(),
            thread_search_input: None,
            thread_name_input: None,
            create_thread_message_input: None,
        };
        this.sync_active_from_route(cx);
        this
    }

    pub(crate) fn toggle_member_list(&mut self, cx: &mut Context<Self>) {
        self.show_member_list = !self.show_member_list;
        cx.notify();
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

    fn maybe_prefetch_voice_token(&mut self, cx: &mut Context<Self>) {
        let active_voice_channel = self
            .channel_list
            .read(cx)
            .active_channel()
            .filter(|ch| ch.channel_type == ChannelType::Voice)
            .map(|ch| ch.id);

        if active_voice_channel == self.prefetched_voice_channel {
            return;
        }
        self.prefetched_voice_channel = active_voice_channel;

        if let Some(channel_id) = active_voice_channel {
            self.voice_store.update(cx, |store, cx| {
                store.prefetch_meet_token(channel_id.to_string(), cx);
            });
        }
    }
}

impl Render for ChatLayout {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("ChatLayout");
        self.chat_area.ensure_input(window, cx);
        self.maybe_prefetch_voice_token(cx);

        if let Some(ch) = self.channel_list.read(cx).active_channel()
            && ch.channel_type == ChannelType::Voice
        {
            let channel_id = ch.id;
            self.voice_store.update(cx, |store, cx| {
                store.prefetch_meet_token(channel_id.to_string(), cx);
            });
        }

        let nav_body = self.render_nav_body(cx);
        let locale = self.settings.read(cx).language.clone();
        let create_panel = self.build_create_thread_panel(&locale, window, cx);
        let chat_content = self.render_content(cx);
        let main_content = if let Some(panel) = create_panel {
            div()
                .flex()
                .flex_row()
                .flex_1()
                .w_full()
                .min_w_0()
                .h_full()
                .min_h_0()
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .min_h_0()
                        .overflow_hidden()
                        .child(chat_content),
                )
                .child(panel)
                .into_any_element()
        } else {
            chat_content
        };
        let voice_mini_bar = self.render_voice_mini_bar(cx);
        let theme = cx.theme();

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
                                .bottom(px(12.0))
                                .h(px(56.0)),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .min_h_0()
                    .overflow_hidden()
                    .bg(theme.bg_primary)
                    .child(main_content),
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
        crate::chat::ChatSending::send_text(
            content,
            mezon_store::OutgoingContent::default(),
            Vec::new(),
            &self.auth_state,
            cx,
        );
    }

    pub(crate) fn open_create_thread(&mut self, cx: &mut Context<Self>) {
        self.thread_popover_handle.hide(cx);
        self.clear_create_thread_inputs(cx);
        ThreadsStore::global(cx).update(cx, |store, cx| store.start_create(cx));
        cx.notify();
    }

    pub(crate) fn close_create_thread(&mut self, cx: &mut Context<Self>) {
        ThreadsStore::global(cx).update(cx, |store, cx| store.cancel_create(cx));
        self.clear_create_thread_inputs(cx);
        cx.notify();
    }

    pub(crate) fn set_create_thread_private(&mut self, private: bool, cx: &mut Context<Self>) {
        ThreadsStore::global(cx).update(cx, |store, cx| {
            store.set_create_private(private, cx);
        });
    }

    pub(crate) fn dismiss_threads_popover(&mut self, cx: &mut Context<Self>) {
        self.thread_popover_handle.hide(cx);
        self.clear_thread_search(cx);
        self.close_create_thread(cx);
    }

    fn clear_create_thread_inputs(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = &self.thread_name_input {
            input.update(cx, |state, cx| state.clear(cx));
        }
        if let Some(input) = &self.create_thread_message_input {
            input.update(cx, |state, cx| state.clear(cx));
        }
    }

    fn clear_thread_search(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = &self.thread_search_input {
            input.update(cx, |state, cx| state.clear(cx));
        }
        ThreadsStore::global(cx).update(cx, |store, cx| {
            store.set_search_query(String::new(), cx);
        });
    }

    pub(crate) fn submit_create_thread(
        &mut self,
        name: String,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        ThreadsStore::global(cx).update(cx, |store, cx| {
            store.submit_create(name, message, cx);
        });
        let _ = window;
    }

    pub(crate) fn navigate_to_thread(
        &mut self,
        channel_id: &str,
        clan_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Ok(channel_id) = channel_id.parse::<ChannelId>() else {
            return;
        };
        let Ok(clan_id) = clan_id.parse::<ClanId>() else {
            return;
        };
        self.thread_popover_handle.hide(cx);
        self.clear_thread_search(cx);
        self.channel_list.update(cx, |list, cx| {
            list.select_channel(channel_id, cx);
        });
        crate::router::navigate(
            cx,
            Route::Channel {
                clan_id,
                channel_id,
            },
        );
    }

    fn on_threads_event(&mut self, event: &ThreadsEvent, cx: &mut Context<Self>) {
        match event {
            ThreadsEvent::ThreadCreated {
                channel_id,
                clan_id,
            } => {
                self.close_create_thread(cx);
                self.navigate_to_thread(channel_id, clan_id, cx);
                ThreadsStore::global(cx).update(cx, |store, cx| store.refresh(cx));
            }
            ThreadsEvent::CreateFailed { message } => {
                Shell::global(cx).update(cx, |shell, cx| {
                    shell.error(message.clone(), cx);
                });
                cx.notify();
            }
        }
    }

    pub(crate) fn ensure_thread_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.thread_search_input.is_some() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let placeholder = mezon_i18n::t(&locale, "channelMenu.menu.thread.searchThreads");
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .embedded(true)
        });
        let input_for_sub = input.clone();
        cx.subscribe_in(&input, window, move |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                let query = input_for_sub.read(cx).value().to_string();
                ThreadsStore::global(cx).update(cx, |store, cx| {
                    store.set_search_query(query, cx);
                });
            }
        })
        .detach();
        self.thread_search_input = Some(input);
    }

    pub(crate) fn settings_language(&self, cx: &App) -> String {
        self.settings.read(cx).language.clone()
    }

    fn ensure_create_thread_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.thread_name_input.is_none() {
            let locale = self.settings.read(cx).language.clone();
            let ph = mezon_i18n::t(&locale, "channelTopbar.createThread.placeholder.threadName");
            self.thread_name_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(ph)
                    .embedded(true)
            }));
        }
        if self.create_thread_message_input.is_none() {
            let locale = self.settings.read(cx).language.clone();
            let ph = mezon_i18n::t(&locale, "chat.messagePlaceholder");
            self.create_thread_message_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(ph)
                    .embedded(true)
            }));
        }
    }

    fn build_create_thread_panel(
        &mut self,
        locale: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !ThreadsStore::global(cx).read(cx).is_creating() {
            return None;
        }
        self.ensure_create_thread_inputs(window, cx);
        let name_input = self.thread_name_input.clone()?;
        let message_input = self.create_thread_message_input.clone()?;
        let theme = cx.theme().clone();
        let name_error = ThreadsStore::global(cx)
            .read(cx)
            .name_error()
            .map(|s| s.to_string());
        let submitting = ThreadsStore::global(cx).read(cx).is_submitting();
        let create_private = ThreadsStore::global(cx).read(cx).create_private();

        Some(crate::chat::create_thread_panel::render_create_thread_panel(
            crate::chat::create_thread_panel::CreateThreadPanelParams {
                thread_name_input: name_input,
                message_input,
                name_error: name_error.as_deref(),
                submitting,
                create_private,
                locale,
                theme: &theme,
                layout: cx.entity(),
            },
        ))
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

    fn render_content(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();

        if self.is_dm_route(cx) {
            if let Some(dm) = self.current_dm(cx) {
                let is_group = dm.kind == DirectKind::Group;
                return self
                    .chat_area
                    .render(
                        &locale,
                        Some(dm.label.as_str()),
                        true,
                        Some(dm.id),
                        is_group,
                        is_group && self.show_member_list,
                        cx,
                    )
                    .into_any_element();
            }
            if matches!(
                Router::global(cx).read(cx).route(),
                Route::DirectMessage { .. }
            ) {
                return self
                    .chat_area
                    .render(&locale, None, true, None, false, false, cx)
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
            let channel_id = ch.id;
            return self
                .chat_area
                .render(
                    &locale,
                    Some(channel_name.as_str()),
                    false,
                    Some(channel_id),
                    true,
                    self.show_member_list,
                    cx,
                )
                .into_any_element();
        }

        let router = Router::global(cx);
        let route = router.read(cx).route();

        let has_active_clan = self.clan_list.read(cx).active_clan_id.is_some();
        if matches!(route, Route::Channel { .. })
            || (matches!(route, Route::Chat) && has_active_clan)
        {
            return self
                .chat_area
                .render(&locale, None, false, None, true, self.show_member_list, cx)
                .into_any_element();
        }

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
