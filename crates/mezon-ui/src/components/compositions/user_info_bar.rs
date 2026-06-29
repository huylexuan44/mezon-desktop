use gpui::{App, AppContext, ClickEvent, Context, Entity, SharedString, Window, div, prelude::*, px};

use crate::components::primitives::{Avatar, Icon, IconName, Sizable, Size};
use crate::image_cache::{
    AVATAR_ENTRY_MAX_BYTES, AVATAR_IMAGE_CACHE_BYTES, AVATAR_IMAGE_CACHE_CAPACITY, LruImageCache,
};
use crate::theme::ActiveTheme;
use mezon_store::{
    AccountStore, AuthState, ClanList, PresenceStore, active_clan_id, current_user_clan_avatar,
};

fn on_settings_click() -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        crate::router::navigate(cx, crate::router::Route::SettingsAccount);
    }
}

pub struct UserInfoBar {
    auth_state: Entity<AuthState>,
    username: SharedString,
    presence: SharedString,
    avatar_raw: SharedString,
    avatar_src: SharedString,
    avatar_image_cache: Entity<LruImageCache>,
}

impl UserInfoBar {
    pub fn new(auth_state: Entity<AuthState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&PresenceStore::global(cx), |this, _, cx| {
            if this.sync_state(cx) {
                cx.notify();
            }
        })
        .detach();
        cx.observe(&auth_state, |this, _, cx| {
            if this.sync_state(cx) {
                cx.notify();
            }
        })
        .detach();
        cx.observe(&AccountStore::global(cx), |this, _, cx| {
            if this.sync_state(cx) {
                cx.notify();
            }
        })
        .detach();
        cx.observe(&ClanList::global(cx), |this, _, cx| {
            if this.sync_state(cx) {
                cx.notify();
            }
        })
        .detach();

        let avatar_image_cache = cx.new(|cx| {
            LruImageCache::avatar_thumbnail(
                "user-info-avatar",
                AVATAR_IMAGE_CACHE_CAPACITY,
                AVATAR_IMAGE_CACHE_BYTES,
                AVATAR_ENTRY_MAX_BYTES,
                cx,
            )
        });

        let username = Self::read_username(&auth_state, cx);
        let mut bar = Self {
            auth_state,
            username,
            presence: SharedString::from("Offline"),
            avatar_raw: SharedString::default(),
            avatar_src: SharedString::default(),
            avatar_image_cache,
        };
        bar.sync_state(cx);
        bar
    }

    fn read_username(auth_state: &Entity<AuthState>, cx: &App) -> SharedString {
        match auth_state.read(cx) {
            AuthState::Authenticated(session) => SharedString::from(session.username.clone()),
            _ => SharedString::from("Unknown"),
        }
    }

    fn sync_avatar(&mut self, cx: &App) -> bool {
        let prev_raw = self.avatar_raw.clone();
        let prev_src = self.avatar_src.clone();
        let clan_id = active_clan_id(cx);
        let raw = current_user_clan_avatar(cx, clan_id);
        if raw.is_empty() {
            self.avatar_raw = SharedString::default();
            self.avatar_src = SharedString::default();
        } else {
            self.avatar_raw = SharedString::from(raw.clone());
            self.avatar_src = SharedString::from(crate::util::imgproxy::avatar_url(cx, &raw));
        }
        self.avatar_raw != prev_raw || self.avatar_src != prev_src
    }

    pub fn sync_state(&mut self, cx: &App) -> bool {
        let prev_username = self.username.clone();
        let prev_presence = self.presence.clone();
        let avatar_changed = self.sync_avatar(cx);
        let user_id = match self.auth_state.read(cx) {
            AuthState::Authenticated(session) => {
                self.username = SharedString::from(session.username.clone());
                session.user_id.clone()
            }
            _ => {
                self.username = SharedString::from("Unknown");
                self.presence = SharedString::from("Offline");
                return self.username != prev_username
                    || self.presence != prev_presence
                    || avatar_changed;
            }
        };
        let online = PresenceStore::global(cx)
            .read(cx)
            .user_online
            .contains(&user_id.parse().unwrap_or_default());
        self.presence = SharedString::from(if online { "Online" } else { "Offline" });
        self.username != prev_username || self.presence != prev_presence || avatar_changed
    }
}

impl Render for UserInfoBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let presence_color = match self.presence.as_ref() {
            "Online" => theme.status_online,
            "Idle" => theme.status_idle,
            "Dnd" => theme.status_dnd,
            _ => theme.status_offline,
        };

        let mut settings_btn = div()
            .id(SharedString::from("settings-btn"))
            .cursor_pointer()
            .p_1()
            .rounded_md()
            .hover(|s| s.bg(theme.tokens.bg_item_hover))
            .child(
                Icon::new(IconName::SettingProfile)
                    .size(px(20.0))
                    .text_color(theme.text_secondary),
            );
        settings_btn.interactivity().on_click(on_settings_click());

        let mut avatar = Avatar::new()
            .name(self.username.clone())
            .with_size(Size::Small)
            .image_cache(self.avatar_image_cache.clone());
        if !self.avatar_src.is_empty() {
            avatar = avatar
                .src(self.avatar_src.clone())
                .fallback_src(self.avatar_raw.clone());
        }

        div()
            .w_full()
            .min_h(px(56.0))
            .overflow_hidden()
            .rounded(px(12.0))
            .border_1()
            .border_color(theme.tokens.border_theme_primary)
            .shadow_lg()
            .bg(theme.tokens.bg_surface)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .gap_2()
                    .pl_2()
                    .pr_4()
                    .py_2()
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .h(px(40.0))
                            .items_center()
                            .gap_3()
                            .pl_2()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.tokens.bg_item_hover))
                            .child(
                                div()
                                    .relative()
                                    .child(avatar)
                                    .child(
                                        div()
                                            .absolute()
                                            .bottom_0()
                                            .right_0()
                                            .size_2()
                                            .rounded_full()
                                            .bg(presence_color),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme.text_primary)
                                            .child(self.username.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.text_muted)
                                            .child(self.presence.clone()),
                                    ),
                            ),
                    )
                    .child(settings_btn),
            )
    }
}
