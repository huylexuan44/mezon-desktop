use gpui::{
    AnyElement, Context, Entity, FontWeight, Hsla, ObjectFit, SharedString, StyledImage, div, img,
    prelude::*, px,
};
use mezon_store::{
    Channel, Settings, VoiceCallStatus, VoiceConnection, VoiceMember, VoiceParticipant, VoiceStore,
};

use crate::ChatLayout;
use crate::components::primitives::{Avatar, Icon, IconName};
use crate::theme::Theme;
use ui::Tooltip;

/// Shared brand accent (Discord-style blurple) used across the voice UI and
/// the screen-share modal. Single source of truth — do not duplicate.
pub(crate) const ACCENT_BLUE: u32 = 0x5865f2;

#[allow(clippy::too_many_arguments)]
pub fn render_voice_channel(
    theme: &Theme,
    locale: &str,
    channel: &Channel,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    cx: &Context<ChatLayout>,
) -> AnyElement {
    let store = voice.read(cx);
    let connecting = matches!(
        store.connection(),
        VoiceConnection::Connecting { channel_id, .. } if *channel_id == channel.id.to_string()
    );

    if store.is_connected_to(&channel.id.to_string()) || connecting {
        return render_in_call(theme, locale, channel, voice, settings, store, connecting);
    }

    let error = match store.connection() {
        VoiceConnection::Failed {
            channel_id,
            message,
        } if *channel_id == channel.id.to_string() => Some(message.clone()),
        _ => None,
    };

    render_pre_join(
        theme,
        locale,
        channel,
        voice,
        input_device_id,
        output_device_id,
        error,
    )
}

pub fn render_mini_bar(
    theme: &Theme,
    locale: &str,
    channel_name: &str,
    voice: &Entity<VoiceStore>,
    mic_enabled: bool,
) -> AnyElement {
    let mic_icon = if mic_enabled {
        IconName::Mic
    } else {
        IconName::MicDisable
    };

    let mic_btn = {
        let voice = voice.clone();
        small_icon_button("voice-minibar-mic", mic_icon, theme.bg_hover)
            .text_color(if mic_enabled {
                theme.text_primary
            } else {
                theme.status_dnd
            })
            .on_click(move |_, _, cx| {
                voice.update(cx, |store, cx| store.toggle_mic(cx));
            })
    };

    let leave_btn = {
        let voice = voice.clone();
        small_icon_button("voice-minibar-leave", IconName::PhoneOff, theme.bg_hover)
            .text_color(theme.status_dnd)
            .on_click(move |_, _, cx| {
                voice.update(cx, |store, cx| store.leave(cx));
            })
    };

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .bg(theme.bg_tertiary)
        .border_t_1()
        .border_color(theme.border)
        .child(
            Icon::new(IconName::Speaker)
                .size(px(18.))
                .text_color(theme.status_online),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.status_online)
                        .child(mezon_i18n::t(locale, "channelVoice.voiceConnected").to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .child(channel_name.to_string()),
                ),
        )
        .child(mic_btn)
        .child(leave_btn)
        .into_any_element()
}

fn small_icon_button(
    id: &'static str,
    icon: IconName,
    bg_hover: impl Into<Hsla>,
) -> gpui::Stateful<gpui::Div> {
    let bg_hover = bg_hover.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(30.))
        .h(px(30.))
        .rounded_md()
        .cursor_pointer()
        .hover(move |s| s.bg(bg_hover))
        .child(Icon::new(icon).size(px(18.)))
}

fn voice_header(
    theme: &Theme,
    name: &str,
    in_call: bool,
    status_badge: Option<(SharedString, Hsla, Hsla)>,
) -> AnyElement {
    let right = in_call.then(|| {
        let bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .child(decorative_icon(theme, IconName::Chat))
            .child(decorative_icon(theme, IconName::VoiceGridIcon))
            .child(decorative_icon(theme, IconName::VoiceFocusIcon));
        bar.when_some(status_badge, |this, (label, text_color, bg_color)| {
            this.child(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .rounded_full()
                    .bg(bg_color)
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text_color)
                    .child(label),
            )
        })
    });

    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px_4()
        .py_2()
        .h(px(50.))
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.bg_primary)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(IconName::Speaker)
                        .size(px(20.0))
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(name.to_string()),
                ),
        )
        .children(right)
        .into_any_element()
}

fn decorative_icon(theme: &Theme, icon: IconName) -> AnyElement {
    Icon::new(icon)
        .size(px(18.))
        .text_color(theme.text_muted)
        .into_any_element()
}

fn render_pre_join(
    theme: &Theme,
    locale: &str,
    channel: &Channel,
    voice: &Entity<VoiceStore>,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    error: Option<String>,
) -> AnyElement {
    let members = &channel.voice_members;
    let subtitle = if members.is_empty() {
        mezon_i18n::t(locale, "channelVoice.noOneInRoom")
    } else {
        mezon_i18n::t(locale, "channelVoice.everyoneWaiting")
    };

    let avatars = (!members.is_empty()).then(|| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_2()
            .children(members.iter().take(12).map(|m| {
                let mut avatar = Avatar::new().name(m.display_name.clone()).size_px(px(56.));
                if !m.avatar_url.is_empty() {
                    avatar = avatar.src(m.avatar_url.clone());
                }
                avatar
            }))
    });

    // A single shared closure builds a join action so both the primary "Join"
    // button and the error-state "Retry" button trigger the same (re)join.
    let make_join_action = {
        let voice = voice.clone();
        let channel_id = channel.id.to_string();
        let clan_id = channel.clan_id.to_string();
        let channel_label = channel.name.clone();
        let input_device_id = input_device_id.clone();
        let output_device_id = output_device_id.clone();
        move || {
            let voice = voice.clone();
            let channel_id = channel_id.clone();
            let clan_id = clan_id.clone();
            let channel_label = channel_label.clone();
            let input_device_id = input_device_id.clone();
            let output_device_id = output_device_id.clone();
            move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut gpui::App| {
                voice.update(cx, |store, cx| {
                    store.join(
                        channel_id.clone(),
                        clan_id.clone(),
                        channel_label.clone(),
                        input_device_id.clone(),
                        output_device_id.clone(),
                        cx,
                    );
                });
            }
        }
    };

    let join = {
        let green = theme.status_online;
        let green_hover = darken(theme.status_online, 0.12);
        div()
            .id("voice-join-btn")
            .flex()
            .items_center()
            .justify_center()
            .px_5()
            .py(px(10.))
            .rounded_full()
            .bg(green)
            .cursor_pointer()
            .hover(move |s| s.bg(green_hover))
            .text_color(gpui::rgb(0xffffff))
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .child(mezon_i18n::t(locale, "channelVoice.joinChannelVoiceBS.joinVoice").to_string())
            .on_click(make_join_action())
    };

    let body = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_4()
        .bg(theme.bg_tertiary)
        .children(avatars)
        .child(
            div()
                .text_color(theme.text_primary)
                .text_xl()
                .font_weight(FontWeight::BOLD)
                .child(channel.name.clone()),
        )
        .child(
            div()
                .text_color(theme.text_secondary)
                .text_sm()
                .child(subtitle.to_string()),
        )
        .when_some(error, |this, message| {
            this.child(div().text_color(theme.status_dnd).text_sm().child(message))
        })
        .child(join);

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .child(voice_header(theme, &channel.name, false, None))
        .child(body)
        .into_any_element()
}

struct VideoCell {
    id: String,
    name: String,
    key: Option<u64>,
    is_screen: bool,
    speaking: bool,
    muted: bool,
}

impl VideoCell {
    fn camera(p: &VoiceParticipant) -> Self {
        Self {
            id: format!("{}\u{1}camera", p.identity),
            name: p.name.clone(),
            key: p.camera,
            is_screen: false,
            speaking: p.speaking,
            muted: p.muted,
        }
    }

    fn screen(p: &VoiceParticipant) -> Self {
        Self {
            id: format!("{}\u{1}screen", p.identity),
            name: p.name.clone(),
            key: p.screenshare,
            is_screen: true,
            speaking: p.speaking,
            muted: p.muted,
        }
    }
}

fn render_in_call(
    theme: &Theme,
    locale: &str,
    channel: &Channel,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    store: &VoiceStore,
    connecting: bool,
) -> AnyElement {
    let participants = store.participants();
    let focused = store.focused_tile();

    let mut cells: Vec<VideoCell> = Vec::new();
    for p in participants {
        if p.screenshare.is_some() {
            cells.push(VideoCell::screen(p));
        }
    }
    for p in participants {
        cells.push(VideoCell::camera(p));
    }

    let focused_idx = focused.and_then(|id| cells.iter().position(|c| c.id == id));

    let body = match focused_idx {
        Some(idx) => render_focus_layout(theme, locale, store, voice, &cells, idx),
        None => render_grid(
            theme,
            locale,
            store,
            voice,
            &cells,
            &channel.voice_members,
            connecting,
        ),
    };

    let status_badge = if connecting {
        Some((
            SharedString::from(mezon_i18n::t(locale, "channelVoice.connecting").to_string()),
            theme.text_primary.into(),
            theme.bg_hover.into(),
        ))
    } else {
        match store.call_status() {
            VoiceCallStatus::Reconnecting => Some((
                SharedString::from(mezon_i18n::t(locale, "channelVoice.reconnecting").to_string()),
                theme.status_idle.into(),
                theme.bg_hover.into(),
            )),
            VoiceCallStatus::WeakNetwork => Some((
                SharedString::from(mezon_i18n::t(locale, "channelVoice.weakNetwork").to_string()),
                theme.status_idle.into(),
                theme.bg_hover.into(),
            )),
            VoiceCallStatus::Stable => None,
        }
    };

    let mic_modal = store
        .mic_permission_denied()
        .then(|| mic_permission_modal(theme, locale, voice));

    div()
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .child(voice_header(theme, &channel.name, true, status_badge))
        .child(body)
        .when(store.fullscreen_screen().is_none(), |this| {
            this.child(control_bar(theme, locale, voice, settings, store))
        })
        .children(mic_modal)
        .into_any_element()
}

pub(crate) fn render_screen_fullscreen_overlay(
    theme: &Theme,
    locale: &str,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    store: &VoiceStore,
) -> Option<AnyElement> {
    let key = store.fullscreen_screen()?;
    let exit_voice = voice.clone();
    let bg_voice = voice.clone();

    let media = match store.render_image(key) {
        Some(image) => img(image)
            .size_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        None => Icon::new(IconName::VoiceScreenShareIcon)
            .size(px(64.))
            .text_color(theme.text_muted)
            .into_any_element(),
    };

    let exit_btn = div()
        .id("screen-fs-exit")
        .absolute()
        .top_4()
        .right_4()
        .flex()
        .items_center()
        .justify_center()
        .w(px(40.))
        .h(px(40.))
        .rounded_full()
        .bg(gpui::rgba(0x000000a6))
        .cursor_pointer()
        .hover(|s| s.bg(gpui::rgba(0x000000d9)))
        .child(
            Icon::new(IconName::ExitFullScreen)
                .size(px(20.))
                .text_color(gpui::rgb(0xffffff)),
        )
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            exit_voice.update(cx, |store, cx| store.clear_fullscreen_screen(cx));
        });

    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::rgb(0x000000))
            .child(
                div()
                    .id("screen-fs-video")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(media)
                    .child(exit_btn)
                    .on_click(move |_, _, cx| {
                        bg_voice.update(cx, |store, cx| store.clear_fullscreen_screen(cx));
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .py_4()
                    .child(control_bar(theme, locale, voice, settings, store)),
            )
            .into_any_element(),
    )
}

fn open_microphone_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .spawn();
    }
}

fn mic_permission_modal(theme: &Theme, locale: &str, voice: &Entity<VoiceStore>) -> AnyElement {
    let title =
        SharedString::from(mezon_i18n::t(locale, "channelVoice.micPermissionTitle").to_string());
    let body =
        SharedString::from(mezon_i18n::t(locale, "channelVoice.micPermissionBody").to_string());
    let open_label =
        SharedString::from(mezon_i18n::t(locale, "channelVoice.openSettings").to_string());
    let later_label = SharedString::from(mezon_i18n::t(locale, "channelVoice.later").to_string());

    let later_hover = darken(theme.bg_tertiary, 0.03);
    let primary_hover = theme.brand_hover;
    let voice_later = voice.clone();

    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgba(0x000000b3))
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_4()
                .w(px(380.))
                .p_6()
                .rounded_xl()
                .bg(theme.bg_floating)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(56.))
                        .h(px(56.))
                        .rounded_full()
                        .bg(theme.bg_hover)
                        .child(
                            Icon::new(IconName::VoiceMicDisabledIcon)
                                .size(px(26.))
                                .text_color(theme.status_dnd),
                        ),
                )
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_sm()
                        .text_center()
                        .text_color(theme.text_muted)
                        .child(body),
                )
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .w_full()
                        .child(
                            div()
                                .id("mic-perm-later")
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .py_2()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(theme.bg_tertiary)
                                .text_color(theme.text_primary)
                                .hover(move |s| s.bg(later_hover))
                                .on_click(move |_, _, cx| {
                                    voice_later.update(cx, |store, cx| {
                                        store.dismiss_mic_permission_prompt(cx)
                                    })
                                })
                                .child(later_label),
                        )
                        .child(
                            div()
                                .id("mic-perm-open")
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .py_2()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(theme.brand)
                                .text_color(gpui::rgb(0xffffff))
                                .hover(move |s| s.bg(primary_hover))
                                .on_click(|_, _, _| open_microphone_settings())
                                .child(open_label),
                        ),
                ),
        )
        .into_any_element()
}

fn render_grid(
    theme: &Theme,
    locale: &str,
    store: &VoiceStore,
    voice: &Entity<VoiceStore>,
    cells: &[VideoCell],
    room_members: &[VoiceMember],
    connecting: bool,
) -> AnyElement {
    if cells.is_empty() {
        if !room_members.is_empty() {
            return div()
                .flex()
                .flex_row()
                .flex_wrap()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_3()
                .p_3()
                .bg(theme.bg_tertiary)
                .children(room_members.iter().map(|member| {
                    let mut avatar = Avatar::new()
                        .name(member.display_name.clone())
                        .size_px(px(80.));
                    if !member.avatar_url.is_empty() {
                        avatar = avatar.src(member.avatar_url.clone());
                    }
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .w(px(180.))
                        .h(px(180.))
                        .rounded_lg()
                        .bg(theme.bg_secondary)
                        .child(avatar)
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text_primary)
                                .child(member.display_name.clone()),
                        )
                }))
                .into_any_element();
        }

        let key = if connecting {
            "channelVoice.connecting"
        } else {
            "channelVoice.noOneInRoom"
        };
        return div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .bg(theme.bg_tertiary)
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_secondary)
                    .child(mezon_i18n::t(locale, key).to_string()),
            )
            .into_any_element();
    }

    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_3()
        .p_3()
        .bg(theme.bg_tertiary)
        .children(
            cells
                .iter()
                .map(|c| video_tile(theme, locale, store, voice, c, px(360.), px(220.))),
        )
        .into_any_element()
}

fn render_focus_layout(
    theme: &Theme,
    locale: &str,
    store: &VoiceStore,
    voice: &Entity<VoiceStore>,
    cells: &[VideoCell],
    focused_idx: usize,
) -> AnyElement {
    let focused = &cells[focused_idx];

    let main = div()
        .flex()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .p_3()
        .child(focus_main_tile(theme, locale, store, voice, focused));

    let strip = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_2()
        .px_3()
        .pb_3()
        .overflow_hidden()
        .children(
            cells
                .iter()
                .enumerate()
                .map(|(i, c)| strip_tile(theme, locale, store, voice, c, i == focused_idx)),
        );

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .bg(theme.bg_tertiary)
        .child(main)
        .child(strip)
        .into_any_element()
}

fn focus_main_tile(
    theme: &Theme,
    locale: &str,
    store: &VoiceStore,
    voice: &Entity<VoiceStore>,
    cell: &VideoCell,
) -> AnyElement {
    let voice = voice.clone();
    let inner = tile_inner(theme, store, cell, true);
    let group = SharedString::from(format!("screen-ctl-{}", cell.id));
    let screen_key = cell.key.filter(|_| cell.is_screen);

    div()
        .id(SharedString::from(format!("focus-main-{}", cell.id)))
        .group(group.clone())
        .relative()
        .flex()
        .flex_1()
        .size_full()
        .min_h_0()
        .items_center()
        .justify_center()
        .rounded_lg()
        .overflow_hidden()
        .bg(theme.bg_secondary)
        .cursor_pointer()
        .child(inner)
        .child(tile_label(theme, locale, cell))
        .when_some(screen_key, |this, key| {
            this.child(screen_tile_controls(&voice, key, group.clone()))
        })
        .on_click(move |_, _, cx| {
            voice.update(cx, |store, cx| store.clear_focus(cx));
        })
        .into_any_element()
}

fn strip_tile(
    theme: &Theme,
    locale: &str,
    store: &VoiceStore,
    voice: &Entity<VoiceStore>,
    cell: &VideoCell,
    is_focused: bool,
) -> AnyElement {
    let voice = voice.clone();
    let id = cell.id.clone();
    let inner = tile_inner(theme, store, cell, false);

    div()
        .id(SharedString::from(format!("strip-{}", cell.id)))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .w(px(214.))
        .h(px(120.))
        .rounded_lg()
        .overflow_hidden()
        .bg(theme.bg_secondary)
        .cursor_pointer()
        .when(is_focused, |this| {
            this.border_2().border_color(gpui::rgb(ACCENT_BLUE))
        })
        .when(cell.speaking && !cell.is_screen && !is_focused, |this| {
            this.border_2().border_color(theme.status_online)
        })
        .child(inner)
        .child(tile_label(theme, locale, cell))
        .on_click(move |_, _, cx| {
            voice.update(cx, |store, cx| store.set_focus(id.clone(), cx));
        })
        .into_any_element()
}

fn video_tile(
    theme: &Theme,
    locale: &str,
    store: &VoiceStore,
    voice: &Entity<VoiceStore>,
    cell: &VideoCell,
    width: gpui::Pixels,
    height: gpui::Pixels,
) -> AnyElement {
    let voice = voice.clone();
    let id = cell.id.clone();
    let inner = tile_inner(theme, store, cell, false);
    let group = SharedString::from(format!("screen-ctl-{}", cell.id));
    let screen_key = cell.key.filter(|_| cell.is_screen);

    div()
        .id(SharedString::from(format!("tile-{}", cell.id)))
        .group(group.clone())
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .w(width)
        .h(height)
        .rounded_lg()
        .overflow_hidden()
        .bg(theme.bg_secondary)
        .cursor_pointer()
        .when(cell.speaking && !cell.is_screen, |this| {
            this.border_2().border_color(theme.status_online)
        })
        .child(inner)
        .child(tile_label(theme, locale, cell))
        .when_some(screen_key, |this, key| {
            this.child(screen_tile_controls(&voice, key, group.clone()))
        })
        .on_click(move |_, _, cx| {
            voice.update(cx, |store, cx| store.toggle_focus(id.clone(), cx));
        })
        .into_any_element()
}

fn tile_inner(theme: &Theme, store: &VoiceStore, cell: &VideoCell, large: bool) -> AnyElement {
    if let Some(key) = cell.key
        && let Some(image) = store.render_image(key)
    {
        let fit = if cell.is_screen {
            ObjectFit::Contain
        } else {
            ObjectFit::Cover
        };
        return img(image).size_full().object_fit(fit).into_any_element();
    }

    if cell.is_screen {
        return Icon::new(IconName::VoiceScreenShareIcon)
            .size(px(48.))
            .text_color(theme.text_muted)
            .into_any_element();
    }

    let mut avatar =
        Avatar::new()
            .name(cell.name.clone())
            .size_px(if large { px(120.) } else { px(80.) });
    if cell.speaking {
        avatar = avatar.border_color(theme.status_online);
    }
    avatar.into_any_element()
}

fn tile_label(theme: &Theme, locale: &str, cell: &VideoCell) -> AnyElement {
    let label = if cell.is_screen {
        mezon_i18n::t(locale, "channelVoice.usernameScreen").replace("{{username}}", &cell.name)
    } else {
        cell.name.clone()
    };

    div()
        .absolute()
        .left_2()
        .bottom_2()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px(px(6.))
        .py(px(2.))
        .rounded_md()
        .bg(gpui::rgba(0x000000b0))
        .when(cell.muted && !cell.is_screen, |this| {
            this.child(
                Icon::new(IconName::VoiceMicDisabledIcon)
                    .size(px(14.))
                    .text_color(theme.status_dnd),
            )
        })
        .when(cell.is_screen, |this| {
            this.child(
                Icon::new(IconName::VoiceScreenShareIcon)
                    .size(px(14.))
                    .text_color(gpui::rgb(0xffffff)),
            )
        })
        .child(div().text_xs().text_color(gpui::rgb(0xffffff)).child(label))
        .into_any_element()
}

fn control_bar(
    theme: &Theme,
    locale: &str,
    voice: &Entity<VoiceStore>,
    settings: &Entity<Settings>,
    store: &VoiceStore,
) -> AnyElement {
    let mic_enabled = store.mic_enabled();
    let camera_enabled = store.camera_enabled();
    let screen_enabled = store.screen_share_enabled();

    let neutral_bg = theme.bg_secondary;
    let neutral_hover = darken(theme.bg_secondary, 0.1);

    let mic_tooltip = mezon_i18n::t(
        locale,
        if mic_enabled {
            "channelVoice.turnOffMicrophone"
        } else {
            "channelVoice.turnOnMicrophone"
        },
    );
    let camera_tooltip = mezon_i18n::t(
        locale,
        if camera_enabled {
            "channelVoice.turnOffCamera"
        } else {
            "channelVoice.turnOnCamera"
        },
    );
    let screen_tooltip = mezon_i18n::t(
        locale,
        if screen_enabled {
            "channelVoice.stopScreenShare"
        } else {
            "channelVoice.shareYourScreen"
        },
    );
    let leave_tooltip = mezon_i18n::t(locale, "channelVoice.leave");

    let mic_button = {
        let voice = voice.clone();
        circle_button(
            "voice-mic-btn",
            neutral_bg,
            neutral_hover,
            if mic_enabled {
                IconName::VoiceMicIcon
            } else {
                IconName::VoiceMicDisabledIcon
            },
            if mic_enabled {
                theme.text_primary
            } else {
                theme.status_dnd
            },
        )
        .tooltip(Tooltip::text(mic_tooltip))
        .on_click(move |_, _, cx| voice.update(cx, |store, cx| store.toggle_mic(cx)))
    };

    let camera_button = {
        let voice = voice.clone();
        circle_button(
            "voice-camera-btn",
            neutral_bg,
            neutral_hover,
            if camera_enabled {
                IconName::VoiceCameraIcon
            } else {
                IconName::VoiceCameraDisabledIcon
            },
            if camera_enabled {
                theme.text_primary
            } else {
                theme.status_dnd
            },
        )
        .tooltip(Tooltip::text(camera_tooltip))
        .on_click(move |_, _, cx| voice.update(cx, |store, cx| store.toggle_camera(cx)))
    };

    let screen_button = {
        let voice = voice.clone();
        let settings = settings.clone();
        let (bg, hover, color): (Hsla, Hsla, Hsla) = if screen_enabled {
            (
                gpui::rgb(ACCENT_BLUE).into(),
                darken(gpui::rgb(ACCENT_BLUE), 0.1),
                gpui::rgb(0xffffff).into(),
            )
        } else {
            (neutral_bg.into(), neutral_hover, theme.text_primary.into())
        };
        circle_button(
            "voice-screen-btn",
            bg,
            hover,
            if screen_enabled {
                IconName::VoiceScreenShareStopIcon
            } else {
                IconName::VoiceScreenShareIcon
            },
            color,
        )
        .tooltip(Tooltip::text(screen_tooltip))
        .on_click(move |_, window, cx| {
            if screen_enabled {
                voice.update(cx, |store, cx| store.stop_screen_share(cx));
            } else {
                crate::chat::screen_share_modal::open_screen_share_modal(
                    voice.clone(),
                    settings.clone(),
                    window,
                    cx,
                );
            }
        })
    };

    let leave_button = {
        let voice = voice.clone();
        circle_button(
            "voice-leave-btn",
            theme.status_dnd,
            darken(theme.status_dnd, 0.12),
            IconName::CancelCall,
            gpui::rgb(0xffffff),
        )
        .tooltip(Tooltip::text(leave_tooltip))
        .on_click(move |_, _, cx| voice.update(cx, |store, cx| store.leave(cx)))
    };

    let left = div()
        .flex()
        .flex_row()
        .flex_1()
        .items_center()
        .gap_3()
        .child(decorative_circle(theme, IconName::VoiceEmojiControlIcon))
        .child(decorative_circle(theme, IconName::VoiceSoundControlIcon));

    let center = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_3()
        .child(mic_button)
        .child(camera_button)
        .child(screen_button)
        .child(decorative_circle(theme, IconName::ShadowBotIcon))
        .child(leave_button);

    div()
        .flex()
        .flex_row()
        .items_center()
        .px_4()
        .py_3()
        .bg(theme.bg_tertiary)
        .border_t_1()
        .border_color(theme.border)
        .child(left)
        .child(center)
        .child(div().flex_1())
        .into_any_element()
}

fn decorative_circle(theme: &Theme, icon: IconName) -> AnyElement {
    // Purely decorative: same footprint as the real control buttons but with no
    // id/on_click and an explicit default cursor so it never reads as clickable.
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(44.))
        .h(px(44.))
        .rounded_full()
        .cursor_default()
        .bg(theme.bg_secondary)
        .child(Icon::new(icon).size(px(20.)).text_color(theme.text_muted))
        .into_any_element()
}

fn screen_tile_controls(voice: &Entity<VoiceStore>, key: u64, group: SharedString) -> AnyElement {
    let pip_voice = voice.clone();
    let fs_voice = voice.clone();
    let btn_bg = gpui::rgba(0x000000a6);
    let btn_hover = gpui::rgba(0x000000d9);
    div()
        .absolute()
        .bottom_2()
        .right_2()
        .flex()
        .gap_2()
        .opacity(0.)
        .group_hover(group, |s| s.opacity(1.))
        .child(
            div()
                .id(SharedString::from(format!("screen-pip-{key}")))
                .flex()
                .items_center()
                .justify_center()
                .w(px(36.))
                .h(px(36.))
                .rounded_full()
                .bg(btn_bg)
                .cursor_pointer()
                .hover(move |s| s.bg(btn_hover))
                .child(
                    Icon::new(IconName::VoicePopOutIcon)
                        .size(px(18.))
                        .text_color(gpui::rgb(0xffffff)),
                )
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    if let Some(handle) = crate::chat::screen_share_pip::open_screen_share_pip(
                        pip_voice.clone(),
                        key,
                        cx,
                    ) {
                        pip_voice.update(cx, |store, cx| store.set_pip(key, handle, cx));
                    }
                }),
        )
        .child(
            div()
                .id(SharedString::from(format!("screen-fs-{key}")))
                .flex()
                .items_center()
                .justify_center()
                .w(px(36.))
                .h(px(36.))
                .rounded_full()
                .bg(btn_bg)
                .cursor_pointer()
                .hover(move |s| s.bg(btn_hover))
                .child(
                    Icon::new(IconName::FullScreen)
                        .size(px(18.))
                        .text_color(gpui::rgb(0xffffff)),
                )
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    fs_voice.update(cx, |store, cx| store.toggle_fullscreen_screen(key, cx));
                }),
        )
        .into_any_element()
}

fn circle_button(
    id: &'static str,
    bg: impl Into<Hsla>,
    bg_hover: Hsla,
    icon: IconName,
    icon_color: impl Into<Hsla>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(44.))
        .h(px(44.))
        .rounded_full()
        .bg(bg.into())
        .cursor_pointer()
        .hover(move |s| s.bg(bg_hover))
        .child(Icon::new(icon).size(px(20.)).text_color(icon_color.into()))
}

fn darken(color: impl Into<Hsla>, amount: f32) -> Hsla {
    let mut hsla = color.into();
    hsla.l = (hsla.l - amount).max(0.0);
    hsla
}
