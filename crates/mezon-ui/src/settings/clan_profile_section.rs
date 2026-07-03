use std::time::Duration;

use crate::components::primitives::{
    Avatar, Button as GpuiButton, ButtonVariants, Icon, IconName, Input, InputEvent, InputState,
    Label, Sizable, Size, h_flex, v_flex,
};
use gpui::{
    Context, Entity, FontWeight, PathPromptOptions, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use mezon_store::{AccountEvent, AccountStore, ClanList, Settings};

use crate::theme::{ActiveTheme, Theme};

struct ClanProfileState {
    selected_clan_id: SharedString,
    nick_name: SharedString,
    avatar_url: Option<SharedString>,
    original_nick_name: SharedString,
    original_avatar_url: Option<SharedString>,
    loading: bool,
    saving: bool,
    duplicate_error: bool,
    #[allow(dead_code)]
    fetched: bool,
}

pub struct ClanProfileSection {
    settings: Entity<Settings>,
    clan_list: Entity<ClanList>,
    profile: Option<ClanProfileState>,
    display_name: SharedString,
    username: SharedString,
    nick_name_input: Option<Entity<InputState>>,
    _subscriptions: Vec<Subscription>,
    toast_message: Option<SharedString>,
    selected_clan_id: String,
}

impl ClanProfileSection {
    pub fn new(
        settings: Entity<Settings>,
        clan_list: Entity<ClanList>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&clan_list, |_, _, cx| cx.notify()).detach();
        cx.observe(&AccountStore::global(cx), |this, store, cx| {
            if let Some(clan_profile) = store.read(cx).clan_profile.as_ref()
                && clan_profile.clan_id.to_string() == this.selected_clan_id
            {
                let nick: SharedString = clan_profile.nick_name.clone().into();
                let avatar: Option<SharedString> = clan_profile.avatar_url.clone().map(Into::into);
                this.profile = Some(ClanProfileState {
                    selected_clan_id: clan_profile.clan_id.to_string().into(),
                    nick_name: nick.clone(),
                    avatar_url: avatar.clone(),
                    original_nick_name: nick,
                    original_avatar_url: avatar,
                    loading: store.read(cx).clan_profile_loading,
                    saving: false,
                    duplicate_error: store.read(cx).nickname_duplicate,
                    fetched: true,
                });
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(
            &AccountStore::global(cx),
            |this, _store, event, cx| match event {
                AccountEvent::ClanProfileSaved => {
                    if let Some(state) = &mut this.profile {
                        state.original_nick_name = state.nick_name.clone();
                        state.original_avatar_url = state.avatar_url.clone();
                        state.saving = false;
                    }
                    let locale = this.settings.read(cx).language.clone();
                    this.show_toast(mezon_i18n::t(&locale, "setting.clanProfile.saved"), cx);
                }
                AccountEvent::ClanProfileSaveFailed(msg) => {
                    if let Some(state) = &mut this.profile {
                        state.saving = false;
                    }
                    let locale = this.settings.read(cx).language.clone();
                    this.show_toast(
                        format!(
                            "{} {}",
                            mezon_i18n::t(&locale, "setting.clanProfile.saveFailed"),
                            msg
                        ),
                        cx,
                    );
                }
                AccountEvent::ClanProfileLoadFailed(msg) => {
                    let locale = this.settings.read(cx).language.clone();
                    this.show_toast(
                        format!(
                            "{} {}",
                            mezon_i18n::t(&locale, "setting.clanProfile.loadFailed"),
                            msg
                        ),
                        cx,
                    );
                }
                AccountEvent::NicknameDuplicateChecked(is_dup) => {
                    if let Some(state) = &mut this.profile {
                        state.duplicate_error = *is_dup;
                    }
                    cx.notify();
                }
                AccountEvent::AvatarUploaded(url) => {
                    if let Some(state) = &mut this.profile {
                        state.avatar_url = Some(url.clone().into());
                    }
                    cx.notify();
                }
                AccountEvent::AvatarUploadFailed(msg) => {
                    let locale = this.settings.read(cx).language.clone();
                    this.show_toast(
                        format!(
                            "{} {}",
                            mezon_i18n::t(&locale, "setting.clanProfile.avatarUploadFailed"),
                            msg
                        ),
                        cx,
                    );
                }
                _ => {}
            },
        )
        .detach();
        Self {
            settings,
            clan_list,
            profile: None,
            display_name: SharedString::default(),
            username: SharedString::default(),
            nick_name_input: None,
            _subscriptions: Vec::new(),
            toast_message: None,
            selected_clan_id: String::new(),
        }
    }

    pub fn set_user_profile(&mut self, display_name: SharedString, username: SharedString) {
        self.display_name = display_name;
        self.username = username;
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

    fn init_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.settings.read(cx).language.clone();
        let nick_ph = mezon_i18n::t(&locale, "setting.clanProfile.nicknamePlaceholder");
        let nick = cx.new(|cx| InputState::new(window, cx).placeholder(nick_ph));

        if let Some(state) = &self.profile {
            nick.update(cx, |input, cx| {
                input.set_value(&state.nick_name, window, cx);
            });
        }

        self._subscriptions.push(cx.subscribe_in(&nick, window, {
            let nick = nick.clone();
            move |this: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    let value = nick.read(cx).value().to_string();
                    if let Some(state) = &mut this.profile
                        && !state.saving
                    {
                        state.nick_name = value.clone().into();
                        state.duplicate_error = false;
                    }
                    cx.notify();

                    let value = value.trim().to_string();
                    if value.len() >= 2 {
                        let clan_id = this.selected_clan_id.clone();
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(Duration::from_millis(600))
                                .await;
                            this.update(cx, |_, cx| {
                                AccountStore::global(cx).update(cx, |store, cx| {
                                    store.check_clan_nickname(
                                        clan_id.parse().unwrap_or_default(),
                                        &value,
                                        cx,
                                    );
                                });
                            })
                            .ok();
                        })
                        .detach();
                    }
                }
            }
        }));

        self.nick_name_input = Some(nick);
    }

    fn is_dirty(&self) -> bool {
        if let Some(state) = &self.profile {
            state.nick_name != state.original_nick_name
                || state.avatar_url != state.original_avatar_url
        } else {
            false
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.profile else {
            return;
        };
        if state.saving || state.duplicate_error {
            return;
        }
        state.saving = true;
        cx.notify();

        let clan_id: String = state.selected_clan_id.to_string();
        let nick_name: String = state.nick_name.to_string();
        let avatar_url: Option<String> = state.avatar_url.as_ref().map(|s| s.to_string());

        AccountStore::global(cx).update(cx, |store, cx| {
            store.save_clan_profile(
                clan_id.parse().unwrap_or_default(),
                nick_name,
                avatar_url,
                cx,
            );
        });
    }

    fn discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.profile {
            state.nick_name = state.original_nick_name.clone();
            state.avatar_url = state.original_avatar_url.clone();
            state.duplicate_error = false;
        }
        if let (Some(input), Some(state)) = (&self.nick_name_input, &self.profile) {
            input.update(cx, |input_state, cx| {
                input_state.set_value(state.nick_name.clone(), window, cx);
            });
        }
    }

    pub fn fetch(&mut self, clan_id: &str, cx: &mut Context<Self>) {
        self.selected_clan_id = clan_id.to_string();
        self.profile = Some(ClanProfileState {
            selected_clan_id: clan_id.into(),
            nick_name: "".into(),
            avatar_url: None,
            original_nick_name: "".into(),
            original_avatar_url: None,
            loading: true,
            saving: false,
            duplicate_error: false,
            fetched: false,
        });
        cx.notify();
        AccountStore::global(cx).update(cx, |store, cx| {
            store.fetch_clan_profile(clan_id.parse().unwrap_or_default(), cx)
        });
    }
}

impl Render for ClanProfileSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();

        if self.profile.as_ref().is_some_and(|p| !p.loading) && self.nick_name_input.is_none() {
            self.init_inputs(window, cx);
        }

        let is_dirty = self.is_dirty();
        let entity = cx.entity().clone();
        let loading = self.profile.as_ref().is_some_and(|p| p.loading);
        let saving = self.profile.as_ref().map(|p| p.saving).unwrap_or(false);

        let clans = self.clan_list.read(cx);
        let clan_options: Vec<(SharedString, SharedString)> = clans
            .clans
            .iter()
            .map(|c| (c.id.to_string().into(), c.name.clone().into()))
            .collect();

        let selected_clan_id: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |s| s.selected_clan_id.clone());

        let nick_name: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |s| s.nick_name.clone());

        let avatar_url = self.profile.as_ref().and_then(|s| s.avatar_url.clone());
        let avatar_display = avatar_url
            .as_ref()
            .map(|url| SharedString::from(crate::util::imgproxy::profile_url(cx, url.as_ref())));

        let duplicate_error = self.profile.as_ref().is_some_and(|s| s.duplicate_error);

        let form = self.render_clan_form(
            &theme,
            &clan_options,
            &selected_clan_id,
            &nick_name,
            avatar_display.clone(),
            loading,
            duplicate_error,
            cx,
        );
        let preview = Self::render_clan_preview(
            &theme,
            &locale,
            &nick_name,
            avatar_display,
            &self.display_name,
            &self.username,
        );

        v_flex()
            .gap_6()
            .child(h_flex().gap_8().child(form).child(preview))
            .when(is_dirty || saving, |el| {
                el.child(
                    v_flex()
                        .gap_3()
                        .pt_4()
                        .child(h_flex().gap_2().items_center().child(
                            div().text_sm().text_color(theme.status_dnd).child(format!(
                                "⚠ {}",
                                mezon_i18n::t(&locale, "setting.profile.unsavedWarning")
                            )),
                        ))
                        .child(
                            h_flex()
                                .gap_3()
                                .child(
                                    GpuiButton::new("clan-save-btn")
                                        .label(if saving {
                                            mezon_i18n::t(&locale, "setting.profile.saving")
                                        } else {
                                            mezon_i18n::t(&locale, "setting.profile.saveChanges")
                                        })
                                        .disabled(saving)
                                        .text_color(theme.text_secondary)
                                        .on_click({
                                            let e = entity.clone();
                                            move |_, _, cx| {
                                                e.update(cx, |this, cx| {
                                                    this.save(cx);
                                                });
                                            }
                                        }),
                                )
                                .child(
                                    GpuiButton::new("clan-discard-btn")
                                        .label(mezon_i18n::t(&locale, "common.discard"))
                                        .disabled(saving)
                                        .text_color(theme.text_primary)
                                        .ghost()
                                        .on_click({
                                            let e = entity.clone();
                                            move |_, window, cx| {
                                                e.update(cx, |this, cx| {
                                                    this.discard(window, cx);
                                                });
                                            }
                                        }),
                                ),
                        ),
                )
            })
            .when_some(self.toast_message.clone(), |this, msg| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .bg(theme.bg_floating)
                        .rounded_md()
                        .text_sm()
                        .text_color(theme.text_primary)
                        .child(msg),
                )
            })
            .into_any_element()
    }
}

impl ClanProfileSection {
    #[allow(clippy::too_many_arguments)]
    fn render_clan_form(
        &self,
        theme: &Theme,
        clan_options: &[(SharedString, SharedString)],
        selected_clan_id: &SharedString,
        nick_name: &SharedString,
        avatar_url: Option<SharedString>,
        loading: bool,
        duplicate_error: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let locale = self.settings.read(cx).language.clone();
        v_flex()
            .gap_4()
            .child(
                v_flex().gap_1().child(
                    div()
                        .text_sm()
                        .text_color(theme.text_muted)
                        .child(mezon_i18n::t(&locale, "setting.clanProfile.customize")),
                ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(mezon_i18n::t(&locale, "setting.clanProfile.chooseClan")),
                    )
                    .child({
                        let entity = cx.entity().clone();
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(clan_options.iter().map(move |(id, name)| {
                                let is_selected = *id == *selected_clan_id;
                                let click_id = id.clone();
                                let e = entity.clone();
                                div()
                                    .id(SharedString::from(format!("clan-opt-{}", id)))
                                    .flex()
                                    .items_center()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .when(is_selected, |el| el.bg(theme.bg_hover))
                                    .hover(|el| el.bg(theme.bg_hover))
                                    .child(name.clone())
                                    .child(div().flex_1())
                                    .when(is_selected, |el| {
                                        el.child(
                                            Icon::new(IconName::Check)
                                                .size_4()
                                                .text_color(theme.brand),
                                        )
                                    })
                                    .on_click({
                                        let e = e.clone();
                                        let click_id = click_id.clone();
                                        move |_, _, cx| {
                                            e.update(cx, |this, cx| {
                                                this.fetch(&click_id, cx);
                                            });
                                        }
                                    })
                            }))
                    }),
            )
            .when(loading, |el| {
                el.child(
                    Label::new(mezon_i18n::t(&locale, "setting.profile.loadingClan"))
                        .text_color(theme.text_muted)
                        .text_sm(),
                )
            })
            .when(!loading, |el| {
                el.child(
                    v_flex()
                        .gap_4()
                        .child(
                            h_flex()
                                .gap_3()
                                .items_center()
                                .child(
                                    Avatar::new()
                                        .when_some(avatar_url.clone(), |av, url| av.src(url))
                                        .name(nick_name.clone())
                                        .with_size(Size::Large),
                                )
                                .child(
                                    GpuiButton::new("clan-change-avatar-btn")
                                        .label(mezon_i18n::t(&locale, "common.changeAvatar"))
                                        .text_color(theme.text_primary)
                                        .ghost()
                                        .on_click({
                                            let entity = cx.entity().clone();
                                            let choose_avatar = mezon_i18n::t(
                                                &locale,
                                                "setting.profile.chooseAvatar",
                                            );
                                            move |_, _, cx| {
                                                let entity = entity.clone();
                                                let rx = cx.prompt_for_paths(PathPromptOptions {
                                                    files: true,
                                                    directories: false,
                                                    multiple: false,
                                                    prompt: Some(choose_avatar.into()),
                                                });
                                                cx.spawn(async move |cx| {
                                                    let paths = match rx.await {
                                                        Ok(Ok(Some(p))) => p,
                                                        _ => return,
                                                    };
                                                    let path = match paths.into_iter().next() {
                                                        Some(p) => p,
                                                        None => return,
                                                    };
                                                    entity.update(cx, |_, cx| {
                                                        AccountStore::global(cx).update(
                                                            cx,
                                                            |store, cx| {
                                                                store.upload_avatar(&path, cx);
                                                            },
                                                        );
                                                    });
                                                })
                                                .detach();
                                            }
                                        }),
                                )
                                .child(
                                    GpuiButton::new("clan-remove-avatar-btn")
                                        .label(mezon_i18n::t(&locale, "common.removeAvatar"))
                                        .text_color(theme.text_muted)
                                        .ghost()
                                        .on_click({
                                            let entity = cx.entity().clone();
                                            move |_, _, cx| {
                                                entity.clone().update(cx, |this, cx| {
                                                    if let Some(state) = &mut this.profile {
                                                        state.avatar_url = None;
                                                    }
                                                    cx.notify();
                                                });
                                            }
                                        }),
                                ),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text_primary)
                                        .child(mezon_i18n::t(
                                            &locale,
                                            "setting.clanProfile.clanNickname",
                                        )),
                                )
                                .child(Input::new(
                                    self.nick_name_input
                                        .as_ref()
                                        .expect("nick_name_input not initialized"),
                                ))
                                .when(duplicate_error, |el| {
                                    el.child(div().text_xs().text_color(theme.status_dnd).child(
                                        mezon_i18n::t(
                                            &locale,
                                            "setting.clanProfile.nicknameExists",
                                        ),
                                    ))
                                }),
                        ),
                )
            })
    }

    fn render_clan_preview(
        theme: &Theme,
        locale: &str,
        nick_name: &SharedString,
        avatar_url: Option<SharedString>,
        display_name: &SharedString,
        username: &SharedString,
    ) -> impl IntoElement {
        let display_label = if nick_name.is_empty() {
            display_name.clone()
        } else {
            nick_name.clone()
        };

        v_flex()
            .gap_4()
            .child(
                Label::new(mezon_i18n::t(locale, "common.preview"))
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                v_flex()
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(theme.bg_primary)
                    .child(div().h(px(105.0)).w_full().bg(theme.brand))
                    .child(
                        v_flex().gap_4().px_6().py_6().child(
                            h_flex()
                                .gap_4()
                                .child(
                                    Avatar::new()
                                        .when_some(avatar_url, |av, url| av.src(url))
                                        .name(nick_name.clone())
                                        .with_size(Size::Large),
                                )
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            Label::new(display_label)
                                                .text_xl()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(theme.text_primary),
                                        )
                                        .child(
                                            Label::new(format!("@{}", username))
                                                .text_sm()
                                                .text_color(theme.text_muted),
                                        ),
                                ),
                        ),
                    ),
            )
    }
}
