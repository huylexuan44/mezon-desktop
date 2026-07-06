use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, DragMoveEvent, Empty, EntityId, FocusHandle,
    KeyDownEvent, MouseButton, MouseDownEvent, ObjectFit, Pixels, Rgba, SharedString, Window,
    canvas, div, img, prelude::*, px, relative,
};
use mezon_store::PlatformStore;
use mezon_video::{VideoFrame, VideoPlayer};

use crate::app::shell::Shell;
use crate::components::primitives::{Icon, IconName, h_flex};
use crate::theme::ActiveTheme;

const SEEK_STEP_SECONDS: f64 = 5.0;
const END_EPSILON_SECONDS: f64 = 0.25;
const THEATER_FILL: f32 = 0.92;
const CONTROL_TINT: Rgba = Rgba {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.16,
};
const SCRIM: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.55,
};
const TRACK_BG: Rgba = Rgba {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.3,
};
const OVERLAY_BG: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.3,
};
const PLAY_DISC_BG: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.5,
};

pub struct VideoActivation {
    pub url: SharedString,
    pub poster: SharedString,
    pub width: f32,
    pub height: f32,
}

#[derive(Default)]
struct SharedPlayback {
    frame: Option<VideoFrame>,
    current_time: f64,
    duration: f64,
    playing: bool,
    muted: bool,
    failed: bool,
}

type Shared = Rc<RefCell<SharedPlayback>>;

#[derive(Clone)]
struct SeekDrag(EntityId);

impl Render for SeekDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

pub struct VideoPlayerView {
    theater: bool,
    focus_handle: FocusHandle,
    url: SharedString,
    poster: SharedString,
    width: f32,
    height: f32,
    player: Option<Rc<VideoPlayer>>,
    shared: Shared,
    track_bounds: Bounds<Pixels>,
    time_label: SharedString,
    last_label_seconds: (u64, u64),
}

impl VideoPlayerView {
    pub fn new(activation: VideoActivation, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let VideoActivation {
            url,
            poster,
            width,
            height,
        } = activation;
        let player = VideoPlayer::open(url.as_ref(), None).ok().map(Rc::new);
        if let Some(player) = player.as_ref() {
            player.play();
        }
        let shared = Rc::new(RefCell::new(SharedPlayback {
            playing: player.is_some(),
            ..SharedPlayback::default()
        }));
        Self::register_teardown(cx);
        Self {
            theater: false,
            focus_handle: cx.focus_handle(),
            url,
            poster,
            width,
            height,
            player,
            shared,
            track_bounds: Bounds::default(),
            time_label: SharedString::new_static("00:00 / 00:00"),
            last_label_seconds: (0, 0),
        }
    }

    fn open_theater(
        player: Rc<VideoPlayer>,
        shared: Shared,
        poster: SharedString,
        window: &mut Window,
        cx: &mut App,
    ) {
        let view = cx.new(|cx| Self {
            theater: true,
            focus_handle: cx.focus_handle(),
            url: SharedString::default(),
            poster,
            width: 0.0,
            height: 0.0,
            player: Some(player),
            shared,
            track_bounds: Bounds::default(),
            time_label: SharedString::new_static("00:00 / 00:00"),
            last_label_seconds: (0, 0),
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn poll_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        let playing = player.is_playing();
        let new_frame = if playing { player.copy_frame() } else { None };
        let current_time = player.current_time();
        let duration = player.duration();
        let muted = player.is_muted();
        let failed = player.failed();
        let previous = {
            let mut shared = self.shared.borrow_mut();
            shared.failed = failed;
            shared.playing = playing;
            shared.current_time = current_time;
            shared.duration = duration;
            shared.muted = muted;
            new_frame.and_then(|frame| shared.frame.replace(frame))
        };
        Self::release_frame(previous, window, cx);
        self.refresh_time_label(current_time, duration);
    }

    fn refresh_time_label(&mut self, current_time: f64, duration: f64) {
        let seconds = (whole_seconds(current_time), whole_seconds(duration));
        if seconds != self.last_label_seconds {
            self.last_label_seconds = seconds;
            self.time_label = SharedString::from(format!(
                "{} / {}",
                format_seconds(current_time),
                format_seconds(duration)
            ));
        }
    }

    pub fn pause_for_background(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        if self.shared.borrow().playing {
            player.pause();
            self.shared.borrow_mut().playing = false;
            cx.notify();
        }
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        {
            let mut shared = self.shared.borrow_mut();
            if shared.playing {
                player.pause();
                shared.playing = false;
            } else {
                if shared.duration > 0.0
                    && shared.current_time >= shared.duration - END_EPSILON_SECONDS
                {
                    player.seek(0.0);
                    shared.current_time = 0.0;
                }
                player.play();
                shared.playing = true;
            }
        }
        cx.notify();
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        let next = !self.shared.borrow().muted;
        player.set_muted(next);
        self.shared.borrow_mut().muted = next;
        cx.notify();
    }

    fn seek_relative(&mut self, delta: f64, cx: &mut Context<Self>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        let target = {
            let mut shared = self.shared.borrow_mut();
            if shared.duration <= 0.0 {
                return;
            }
            let target = (shared.current_time + delta).clamp(0.0, shared.duration);
            shared.current_time = target;
            target
        };
        player.seek(target);
        cx.notify();
    }

    fn seek_to_x(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        let bounds = self.track_bounds;
        let target = {
            let mut shared = self.shared.borrow_mut();
            if shared.duration <= 0.0 {
                return;
            }
            let target = fraction_from_position(bounds, x) as f64 * shared.duration;
            shared.current_time = target;
            target
        };
        player.seek(target);
        cx.notify();
    }

    fn open_fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(player) = self.player.clone() {
            Self::open_theater(player, self.shared.clone(), self.poster.clone(), window, cx);
        }
    }

    fn close_theater(cx: &mut Context<Self>) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn open_external(&self, cx: &mut App) {
        if let Some(store) = PlatformStore::try_global(cx) {
            let _ = store.read(cx).open_url_external(&self.url);
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "space" => self.toggle_play(cx),
            "left" => self.seek_relative(-SEEK_STEP_SECONDS, cx),
            "right" => self.seek_relative(SEEK_STEP_SECONDS, cx),
            "f" if !self.theater => self.open_fullscreen(window, cx),
            "escape" if self.theater => Self::close_theater(cx),
            _ => {}
        }
    }

    #[cfg(target_os = "macos")]
    fn release_frame(_previous: Option<VideoFrame>, _window: &mut Window, _cx: &mut Context<Self>) {
    }

    #[cfg(not(target_os = "macos"))]
    fn release_frame(previous: Option<VideoFrame>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(previous) = previous {
            cx.drop_image(previous, Some(window));
        }
    }

    #[cfg(target_os = "macos")]
    fn register_teardown(_cx: &mut Context<Self>) {}

    #[cfg(not(target_os = "macos"))]
    fn register_teardown(cx: &mut Context<Self>) {
        cx.on_release(|view, cx| {
            if let Some(frame) = view.shared.borrow_mut().frame.take() {
                cx.drop_image(frame, None);
            }
        })
        .detach();
    }

    #[cfg(target_os = "macos")]
    pub fn release_textures(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    #[cfg(not(target_os = "macos"))]
    pub fn release_textures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(frame) = self.shared.borrow_mut().frame.take() {
            cx.drop_image(frame, Some(window));
        }
    }

    #[cfg(target_os = "macos")]
    fn frame_child(&self) -> Option<AnyElement> {
        self.shared
            .borrow()
            .frame
            .clone()
            .map(|frame| gpui::surface(frame).size_full().into_any_element())
    }

    #[cfg(not(target_os = "macos"))]
    fn frame_child(&self) -> Option<AnyElement> {
        self.shared
            .borrow()
            .frame
            .clone()
            .map(|frame| gpui::img(frame).size_full().into_any_element())
    }

    fn has_frame(&self) -> bool {
        self.shared.borrow().frame.is_some()
    }

    fn render_seek(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity_id = cx.entity_id();
        let fraction = {
            let shared = self.shared.borrow();
            if shared.duration > 0.0 {
                (shared.current_time / shared.duration).clamp(0.0, 1.0) as f32
            } else {
                0.0
            }
        };
        let brand = cx.theme().brand;
        div()
            .id("video-seek")
            .relative()
            .flex()
            .items_center()
            .h(px(14.))
            .w_full()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &MouseDownEvent, _window, cx| {
                    view.seek_to_x(event.position.x, cx);
                }),
            )
            .on_drag(SeekDrag(entity_id), |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .on_drag_move(
                cx.listener(move |view, event: &DragMoveEvent<SeekDrag>, _window, cx| {
                    let SeekDrag(id) = event.drag(cx);
                    if *id != entity_id {
                        return;
                    }
                    view.seek_to_x(event.event.position.x, cx);
                }),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(4.))
                    .rounded_full()
                    .bg(TRACK_BG)
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(relative(fraction))
                            .bg(brand)
                            .rounded_full(),
                    ),
            )
            .child({
                let view = cx.entity();
                canvas(
                    move |bounds, _, cx| view.update(cx, |this, _| this.track_bounds = bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
            })
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theater = self.theater;
        let (playing, muted) = {
            let shared = self.shared.borrow();
            (shared.playing, shared.muted)
        };
        let label = self.time_label.clone();
        let play_icon = if playing {
            IconName::PauseButton
        } else {
            IconName::PlayButton
        };
        let mute_icon = if muted {
            IconName::MutedVolume
        } else {
            IconName::LoudVolume
        };
        let last_icon = if theater {
            IconName::ExitFullScreen
        } else {
            IconName::FullScreen
        };
        div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1p5()
            .bg(SCRIM)
            .opacity(0.0)
            .group_hover("video-player", |s| s.opacity(1.0))
            .child(self.render_seek(cx))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(control_button(
                        "video-playpause",
                        play_icon,
                        cx.listener(|view, _, _window, cx| view.toggle_play(cx)),
                    ))
                    .child(div().text_xs().text_color(gpui::white()).child(label))
                    .child(div().flex_1())
                    .child(control_button(
                        "video-mute",
                        mute_icon,
                        cx.listener(|view, _, _window, cx| view.toggle_mute(cx)),
                    ))
                    .child(control_button(
                        "video-fullscreen",
                        last_icon,
                        cx.listener(move |view, _, window, cx| {
                            if theater {
                                Self::close_theater(cx);
                            } else {
                                view.open_fullscreen(window, cx);
                            }
                        }),
                    )),
            )
    }
}

impl Render for VideoPlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.poll_frame(window, cx);
        let theme = cx.theme();
        let has_frame = self.has_frame();
        let (playing, failed) = {
            let shared = self.shared.borrow();
            (shared.playing, shared.failed)
        };
        let mut root = div()
            .id("video-player")
            .group("video-player")
            .relative()
            .overflow_hidden()
            .bg(theme.bg_tertiary)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                view.on_key(event, window, cx);
            }));
        root = if self.theater {
            let viewport = window.viewport_size();
            root.w(viewport.width * THEATER_FILL)
                .h(viewport.height * THEATER_FILL)
        } else {
            root.w(px(self.width))
                .h(px(self.height))
                .max_w_full()
                .rounded_lg()
        };

        if self.player.is_none() || failed {
            return root
                .cursor_pointer()
                .when(!self.poster.is_empty(), |d| {
                    d.child(
                        img(self.poster.clone())
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                })
                .child(play_circle())
                .on_click(cx.listener(|view, _, _window, cx| view.open_external(cx)))
                .into_any_element();
        }

        if playing && window.is_window_active() {
            window.request_animation_frame();
        }

        root.children(self.frame_child())
            .when(!has_frame && !self.poster.is_empty(), |d| {
                d.child(
                    img(self.poster.clone())
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
            })
            .child(self.render_controls(cx))
            .into_any_element()
    }
}

fn control_button(
    id: &'static str,
    icon: IconName,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(CONTROL_TINT))
        .on_click(on_click)
        .child(Icon::new(icon).size(px(16.)).text_color(gpui::white()))
}

fn play_circle() -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(OVERLAY_BG)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(48.))
                .rounded_full()
                .bg(PLAY_DISC_BG)
                .child(
                    Icon::new(IconName::PlayButton)
                        .size(px(20.))
                        .text_color(gpui::white()),
                ),
        )
}

fn fraction_from_position(bounds: Bounds<Pixels>, x: Pixels) -> f32 {
    let width = bounds.size.width;
    if width <= px(0.0) {
        return 0.0;
    }
    ((x - bounds.left()) / width).clamp(0.0, 1.0)
}

fn whole_seconds(total: f64) -> u64 {
    if total.is_finite() && total > 0.0 {
        total as u64
    } else {
        0
    }
}

fn format_seconds(total: f64) -> String {
    let seconds = whole_seconds(total);
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}
