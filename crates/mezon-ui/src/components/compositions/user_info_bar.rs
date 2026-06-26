use gpui::{App, ClickEvent, Context, Entity, SharedString, Window, div, prelude::*, px};

use crate::components::primitives::{Avatar, Icon, IconName, Sizable, Size};
use crate::theme::ActiveTheme;
use mezon_store::{AuthState, PresenceStore};

fn on_settings_click() -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        crate::router::navigate(cx, crate::router::Route::SettingsAccount);
    }
}

pub struct UserInfoBar {
    auth_state: Entity<AuthState>,
    username: SharedString,
    presence: SharedString,
}

impl UserInfoBar {
    pub fn new(auth_state: Entity<AuthState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&PresenceStore::global(cx), |this, _, cx| {
            this.sync_presence(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&auth_state, |this, _, cx| {
            this.sync_presence(cx);
            cx.notify();
        })
        .detach();
        let username = Self::read_username(&auth_state, cx);
        let mut bar = Self {
            auth_state,
            username,
            presence: SharedString::from("Offline"),
        };
        bar.sync_presence(cx);
        bar
    }

    fn read_username(auth_state: &Entity<AuthState>, cx: &App) -> SharedString {
        match auth_state.read(cx) {
            AuthState::Authenticated(session) => SharedString::from(session.username.clone()),
            _ => SharedString::from("Unknown"),
        }
    }

    pub fn sync_presence(&mut self, cx: &App) {
        let user_id = match self.auth_state.read(cx) {
            AuthState::Authenticated(session) => {
                self.username = SharedString::from(session.username.clone());
                session.user_id.clone()
            }
            _ => {
                self.username = SharedString::from("Unknown");
                self.presence = SharedString::from("Offline");
                return;
            }
        };
        let online = PresenceStore::global(cx)
            .read(cx)
            .user_online
            .contains(&user_id.parse().unwrap_or_default());
        self.presence = SharedString::from(if online { "Online" } else { "Offline" });
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

        // Positioning (absolute / insets) is applied by the cached wrapper in
        // the chat layout so this view can be `.cached()`; keep only the visual
        // box here.
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
                                    .child(
                                        Avatar::new()
                                            .name(self.username.clone())
                                            .with_size(Size::Small),
                                    )
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
