use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, Context, ObjectFit, SharedString, Task, Window, div, img, prelude::*, px,
};
use mezon_video::{VideoFrame, VideoPlayer};

use crate::theme::ActiveTheme;

const FRAME_INTERVAL_MS: u64 = 16;
const LOOP_EPSILON_SECONDS: f64 = 0.08;

#[derive(Default)]
struct GifPlayback {
    frame: Option<VideoFrame>,
    failed: bool,
}

pub struct GifVideoView {
    fallback_gif: SharedString,
    width: f32,
    height: f32,
    player: Option<Rc<VideoPlayer>>,
    shared: Rc<RefCell<GifPlayback>>,
    _pump: Option<Task<()>>,
}

impl GifVideoView {
    pub fn new(
        mp4_url: SharedString,
        fallback_gif: SharedString,
        width: f32,
        height: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        let player = VideoPlayer::open(mp4_url.as_ref()).ok().map(Rc::new);
        if let Some(player) = player.as_ref() {
            player.set_muted(true);
            player.play();
        }
        let shared = Rc::new(RefCell::new(GifPlayback::default()));
        let pump = player.as_ref().map(|_| Self::spawn_pump(cx));
        Self::register_teardown(cx);
        Self {
            fallback_gif,
            width,
            height,
            player,
            shared,
            _pump: pump,
        }
    }

    fn spawn_pump(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(FRAME_INTERVAL_MS))
                    .await;
                if this.update(cx, |view, cx| view.tick(cx)).is_err() {
                    break;
                }
            }
        })
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        let failed = player.failed();
        let duration = player.duration();
        let current_time = player.current_time();
        if duration > 0.0 && current_time >= duration - LOOP_EPSILON_SECONDS {
            player.seek(0.0);
            player.play();
        }
        let new_frame = player.copy_frame();
        let mut changed = false;
        {
            let mut shared = self.shared.borrow_mut();
            if failed != shared.failed {
                shared.failed = failed;
                changed = true;
            }
            if let Some(frame) = new_frame {
                let previous = shared.frame.replace(frame);
                Self::release_frame(previous, cx);
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
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
}

impl Render for GifVideoView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let failed = self.shared.borrow().failed;
        let root = div()
            .w(px(self.width))
            .h(px(self.height))
            .max_w_full()
            .rounded_md()
            .overflow_hidden()
            .bg(theme.bg_tertiary);

        if (self.player.is_none() || failed) && !self.fallback_gif.is_empty() {
            return root.child(
                img(self.fallback_gif.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            );
        }

        root.children(if failed { None } else { self.frame_child() })
    }
}
