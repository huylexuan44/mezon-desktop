use std::time::Duration;

use crate::components::primitives::{
    Avatar, Button as GpuiButton, ButtonVariants, Label, Sizable, Size, h_flex, v_flex,
};
use gpui::{Context, Entity, FontWeight, Rgba, SharedString, Task, Window, div, prelude::*, px};
use mezon_store::{AccountStore, Settings};

use crate::image_cache::LruImageCache;
use crate::theme::ActiveTheme;
use crate::util::avatar_color::spawn_banner_color_task;

/// The avatar overlaps the banner, so its box is positioned against the card
/// rather than laid out in flow. Every offset below is derived from these four
/// numbers — changing one must not require hand-recomputing the others.
const BANNER_HEIGHT: f32 = 100.0;
const HEADER_ROW_HEIGHT: f32 = 104.0;
const HEADER_PAD_X: f32 = 20.0;
const AVATAR_SIZE: f32 = 96.0;
const AVATAR_RING: f32 = 6.0;
const AVATAR_BOX: f32 = AVATAR_SIZE + AVATAR_RING * 2.0;
const AVATAR_LEFT: f32 = 20.0;
const AVATAR_TOP: f32 = 72.0;
const AVATAR_NAME_GAP: f32 = 20.0;
/// The header's children already start at `HEADER_PAD_X`; push the name past
/// the avatar box from there.
const NAME_INDENT: f32 = AVATAR_LEFT + AVATAR_BOX + AVATAR_NAME_GAP - HEADER_PAD_X;
/// The header row centers its children; lift the name onto the avatar's midline.
const NAME_LIFT: f32 = (AVATAR_TOP + AVATAR_BOX / 2.0) - (BANNER_HEIGHT + HEADER_ROW_HEIGHT / 2.0);
/// Optical nudge so the button's cap height sits level with the name.
const EDIT_BUTTON_LIFT: f32 = -8.0;

pub struct AccountPage {
    settings: Entity<Settings>,
    avatar_image_cache: Entity<LruImageCache>,
    banner_color: Option<Rgba>,
    banner_source: String,
    banner_task: Option<Task<()>>,
    toast_message: Option<SharedString>,
}

impl AccountPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&AccountStore::global(cx), |this, _, cx| {
            this.refresh_banner_source(cx);
            cx.notify();
        })
        .detach();
        AccountStore::global(cx).update(cx, |store, cx| store.ensure_account(cx));
        let mut page = Self {
            settings,
            avatar_image_cache: crate::image_cache::shared_avatar_cache(cx),
            banner_color: None,
            banner_source: String::new(),
            banner_task: None,
            toast_message: None,
        };
        page.refresh_banner_source(cx);
        page
    }

    fn show_toast(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast_message = Some(message.into());
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.toast_message = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Must stay the single source of the avatar url: the banner reads its color
    /// back out of the image cache, so it can only find what `render` asked for.
    fn avatar_source(account_avatar_url: Option<&str>, cx: &Context<Self>) -> String {
        account_avatar_url
            .map(|url| crate::util::imgproxy::profile_url(cx, url))
            .unwrap_or_default()
    }

    fn refresh_banner_source(&mut self, cx: &mut Context<Self>) {
        let raw = AccountStore::global(cx)
            .read(cx)
            .account
            .as_ref()
            .and_then(|account| account.avatar_url.clone());
        let source = Self::avatar_source(raw.as_deref(), cx);
        if source == self.banner_source {
            return;
        }
        self.banner_source = source.clone();
        self.banner_color = None;
        self.banner_task = spawn_banner_color_task(
            self.avatar_image_cache.clone(),
            source,
            cx,
            |this, color, cx| {
                this.banner_color = Some(color);
                cx.notify();
            },
        );
    }
}

impl Render for AccountPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let locale = self.settings.read(cx).language.clone();
        let store = AccountStore::global(cx).read(cx);

        if store.account_error {
            return v_flex()
                .gap_4()
                .child(
                    Label::new(mezon_i18n::t(&locale, "setting.account.failedToLoad"))
                        .text_color(theme.text_muted),
                )
                .into_any_element();
        }

        if store.account_loading || store.account.is_none() {
            return v_flex()
                .gap_4()
                .child(
                    Label::new(mezon_i18n::t(&locale, "setting.account.loading"))
                        .text_color(theme.text_muted),
                )
                .into_any_element();
        }

        let account = store.account.as_ref().unwrap();

        let display_name: SharedString = account.display_name.clone().into();
        let username: SharedString = account.username.clone().into();

        let email_display = match &account.email {
            Some(email) if !email.is_empty() => SharedString::from(mask_email(email)),
            _ => SharedString::from(mezon_i18n::t(&locale, "common.notSet")),
        };

        let password_label = if account.password_setted {
            SharedString::from(mezon_i18n::t(&locale, "setting.account.changePassword"))
        } else {
            SharedString::from(mezon_i18n::t(&locale, "setting.account.setPassword"))
        };

        let password_display = if account.password_setted {
            SharedString::from("*********")
        } else {
            SharedString::from(mezon_i18n::t(&locale, "setting.account.password"))
        };

        let phone_display = account
            .phone_number
            .as_ref()
            .map(|s| SharedString::from(s.as_str()))
            .unwrap_or(SharedString::from(mezon_i18n::t(&locale, "common.notSet")));

        let phone_label = if account.phone_number.is_some() {
            SharedString::from(mezon_i18n::t(&locale, "setting.account.changePhone"))
        } else {
            SharedString::from(mezon_i18n::t(&locale, "setting.account.setPhone"))
        };

        let avatar_url = SharedString::from(Self::avatar_source(account.avatar_url.as_deref(), cx));
        let banner_color = self
            .banner_color
            .map(gpui::Hsla::from)
            .unwrap_or(theme.tokens.bg_highlight_react_theme.into());

        // React renders these labels through `uppercase font-bold text-xs`.
        let account_field = |label: SharedString, value: SharedString| {
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text_muted)
                        .child(SharedString::from(label.to_uppercase())),
                )
                .child(Label::new(value).text_color(theme.text_secondary))
        };

        let outlined_button = |id: &'static str, label: SharedString| {
            div()
                .id(id)
                .h(px(40.0))
                .px_3()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded_md()
                .bg(theme.tokens.bg_button_secondary)
                .border_1()
                .border_color(theme.border)
                .text_color(theme.text_secondary)
                .cursor_pointer()
                .hover(move |style| {
                    style
                        .bg(theme.tokens.bg_item_hover)
                        .text_color(theme.text_primary)
                })
                .child(label)
        };

        v_flex()
            .gap_6()
            .text_sm()
            .child(
                v_flex()
                    .relative()
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(theme.bg_secondary)
                    .shadow_md()
                    .child(
                        div()
                            .h(px(BANNER_HEIGHT))
                            .w_full()
                            .bg(banner_color)
                            .rounded_t_lg(),
                    )
                    .child(
                        h_flex()
                            .h(px(HEADER_ROW_HEIGHT))
                            .px(px(HEADER_PAD_X))
                            .gap_4()
                            .items_center()
                            .child(
                                Label::new(display_name.clone())
                                    .relative()
                                    .top(px(NAME_LIFT))
                                    .ml(px(NAME_INDENT))
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text_secondary),
                            )
                            .child(div().flex_1())
                            .child(
                                GpuiButton::new("edit-profile-btn")
                                    .label(mezon_i18n::t(
                                        &locale,
                                        "setting.account.editUserProfile",
                                    ))
                                    .relative()
                                    .top(px(EDIT_BUTTON_LIFT))
                                    .with_size(Size::Large)
                                    .primary()
                                    .on_click(move |_, _, cx| {
                                        crate::router::replace(
                                            cx,
                                            crate::router::Route::SettingsProfile,
                                        );
                                    }),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_4()
                            .px_4()
                            .pb_4()
                            .child(
                                v_flex()
                                    .gap_4()
                                    .p_4()
                                    .rounded_md()
                                    .bg(theme.bg_primary)
                                    .shadow_sm()
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .items_center()
                                            .child(account_field(
                                                mezon_i18n::t(
                                                    &locale,
                                                    "setting.account.displayName",
                                                )
                                                .into(),
                                                display_name.clone(),
                                            ))
                                            .child(
                                                outlined_button(
                                                    "edit-display-name-btn",
                                                    mezon_i18n::t(&locale, "accountSetting.edit")
                                                        .into(),
                                                )
                                                .on_click(move |_, _, cx| {
                                                    crate::router::replace(
                                                        cx,
                                                        crate::router::Route::SettingsProfile,
                                                    );
                                                }),
                                            ),
                                    )
                                    .child(account_field(
                                        mezon_i18n::t(&locale, "setting.account.username").into(),
                                        username,
                                    )),
                            )
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .p_4()
                                    .rounded_md()
                                    .bg(theme.bg_primary)
                                    .shadow_sm()
                                    .child(account_field(
                                        mezon_i18n::t(&locale, "setting.account.email").into(),
                                        email_display,
                                    )),
                            )
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .p_4()
                                    .rounded_md()
                                    .bg(theme.bg_primary)
                                    .shadow_sm()
                                    .child(account_field(
                                        mezon_i18n::t(&locale, "setting.account.password").into(),
                                        password_display,
                                    ))
                                    .child(
                                        outlined_button("password-btn", password_label).on_click(
                                            cx.listener(|this, _, _, cx| {
                                                let locale =
                                                    this.settings.read(cx).language.clone();
                                                this.show_toast(
                                                    mezon_i18n::t(
                                                        &locale,
                                                        "setting.account.passwordComingSoon",
                                                    ),
                                                    cx,
                                                );
                                            }),
                                        ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .p_4()
                                    .rounded_md()
                                    .bg(theme.bg_primary)
                                    .shadow_sm()
                                    .child(account_field(
                                        mezon_i18n::t(&locale, "setting.account.phone").into(),
                                        phone_display,
                                    ))
                                    .child(outlined_button("phone-btn", phone_label).on_click(
                                        cx.listener(|this, _, _, cx| {
                                            let locale = this.settings.read(cx).language.clone();
                                            this.show_toast(
                                                mezon_i18n::t(
                                                    &locale,
                                                    "setting.account.phoneComingSoon",
                                                ),
                                                cx,
                                            );
                                        }),
                                    )),
                            ),
                    )
                    // Declared last, so it already paints over the banner without
                    // being promoted into the deferred layer.
                    .child(
                        div()
                            .absolute()
                            .left(px(AVATAR_LEFT))
                            .top(px(AVATAR_TOP))
                            .size(px(AVATAR_BOX))
                            .rounded_full()
                            .bg(theme.bg_primary)
                            .p(px(AVATAR_RING))
                            .child(
                                Avatar::new()
                                    .when(!avatar_url.is_empty(), |avatar| {
                                        avatar.src(avatar_url.clone())
                                    })
                                    .name(display_name)
                                    .size_px(px(AVATAR_SIZE))
                                    .image_cache(self.avatar_image_cache.clone()),
                            ),
                    ),
            )
            .when_some(self.toast_message.clone(), |this, msg| {
                this.child(div().text_sm().text_color(theme.text_muted).child(msg))
            })
            .into_any_element()
    }
}

fn mask_email(email: &str) -> String {
    let at = email.find('@').unwrap_or(email.len());
    if at > 1 {
        format!("{}***{}", &email[..1], &email[at..])
    } else {
        email.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The header offsets used to be hand-tuned literals. Pin the derived values
    /// so a change to the banner or avatar can't silently shift the name.
    #[test]
    fn name_offsets_track_the_avatar_box() {
        assert_eq!(AVATAR_BOX, 108.0);
        assert_eq!(NAME_INDENT, 128.0);
        assert_eq!(NAME_LIFT, -26.0);
    }

    #[test]
    fn name_sits_on_the_avatar_midline() {
        let avatar_center = AVATAR_TOP + AVATAR_BOX / 2.0;
        let header_center = BANNER_HEIGHT + HEADER_ROW_HEIGHT / 2.0;
        assert_eq!(header_center + NAME_LIFT, avatar_center);
    }

    #[test]
    fn name_clears_the_avatar_box() {
        assert_eq!(
            HEADER_PAD_X + NAME_INDENT,
            AVATAR_LEFT + AVATAR_BOX + AVATAR_NAME_GAP
        );
    }
}
