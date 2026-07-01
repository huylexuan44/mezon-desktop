use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyElement, Context, ObjectFit, Render, SharedString, Window, div, img, prelude::*, px,
};
use mezon_video::{VideoFrame, VideoPlayer};

use crate::theme::ActiveTheme;

#[derive(Default)]
struct ThumbState {
    frame: Option<VideoFrame>,
    failed: bool,
}

pub struct VideoThumbnail {
    player: Option<Rc<VideoPlayer>>,
    shared: Rc<RefCell<ThumbState>>,
    frozen: bool,
}

impl VideoThumbnail {
    pub fn new(url: impl Into<SharedString>, width: f32, height: f32, cx: &mut Context<Self>) -> Self {
        let url = url.into();
        let player = VideoPlayer::open(url.as_ref(), decode_size(width, height))
            .ok()
            .map(Rc::new);
        if let Some(player) = player.as_ref() {
            player.set_muted(true);
            player.play();
        }
        Self::register_teardown(cx);
        Self {
            player,
            shared: Rc::new(RefCell::new(ThumbState::default())),
            frozen: false,
        }
    }

    fn poll_frame(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        let failed = player.failed();
        let new_frame = player.copy_frame();
        let previous = {
            let mut shared = self.shared.borrow_mut();
            shared.failed = failed;
            let had_frame = shared.frame.is_some();
            let prev = new_frame.and_then(|frame| shared.frame.replace(frame));
            if !had_frame && shared.frame.is_some() && !self.frozen {
                player.pause();
                self.frozen = true;
            }
            prev
        };
        Self::release_frame(previous, cx);
    }

    fn needs_animation(&self) -> bool {
        let shared = self.shared.borrow();
        self.player.is_some() && !shared.failed && shared.frame.is_none()
    }

    fn has_frame(&self) -> bool {
        self.shared.borrow().frame.is_some()
    }

    #[cfg(target_os = "macos")]
    fn release_frame(_previous: Option<VideoFrame>, _cx: &mut Context<Self>) {}

    #[cfg(not(target_os = "macos"))]
    fn release_frame(previous: Option<VideoFrame>, cx: &mut Context<Self>) {
        if let Some(previous) = previous {
            cx.drop_image(previous, None);
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
    fn frame_child(&self) -> Option<AnyElement> {
        self.shared
            .borrow()
            .frame
            .clone()
            .map(|frame| gpui::surface(frame).size_full().object_fit(ObjectFit::Cover).into_any_element())
    }

    #[cfg(not(target_os = "macos"))]
    fn frame_child(&self) -> Option<AnyElement> {
        self.shared
            .borrow()
            .frame
            .clone()
            .map(|frame| img(frame).size_full().object_fit(ObjectFit::Cover).into_any_element())
    }
}

impl Render for VideoThumbnail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.poll_frame(cx);
        let theme = cx.theme();
        let failed = self.shared.borrow().failed;

        if self.player.is_none() || failed {
            return div()
                .size_full()
                .bg(theme.bg_tertiary)
                .into_any_element();
        }

        if self.needs_animation() && window.is_window_active() {
            window.request_animation_frame();
        }

        div()
            .size_full()
            .overflow_hidden()
            .bg(theme.bg_tertiary)
            .when(self.has_frame(), |el| el.children(self.frame_child()))
            .into_any_element()
    }
}

fn decode_size(width: f32, height: f32) -> Option<(u32, u32)> {
    let width = width.round();
    let height = height.round();
    if width >= 1.0 && height >= 1.0 {
        Some((width as u32, height as u32))
    } else {
        None
    }
}
