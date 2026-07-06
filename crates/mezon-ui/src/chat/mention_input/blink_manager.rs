use std::time::Duration;

use gpui::Context;

use super::MentionInputState;

const BLINK_INTERVAL: Duration = Duration::from_millis(500);
const BLINK_PAUSE: Duration = Duration::from_millis(500);
const MAX_IDLE_TICKS: usize = 10;

pub(crate) struct CaretBlink {
    blink_epoch: usize,
    idle_ticks: usize,
    visible: bool,
    enabled: bool,
    window_active: bool,
}

impl CaretBlink {
    pub fn new(window_active: bool) -> Self {
        Self {
            blink_epoch: 0,
            idle_ticks: 0,
            visible: true,
            enabled: false,
            window_active,
        }
    }

    pub fn visible(&self) -> bool {
        self.enabled && self.visible
    }

    pub fn sync_focused(&mut self, cx: &mut Context<MentionInputState>) {
        if self.enabled {
            return;
        }
        self.enabled = true;
        self.visible = true;
        self.idle_ticks = 0;
        cx.notify();
        if self.window_active {
            self.schedule_blink(cx);
        }
    }

    pub fn sync_window_active(&mut self, active: bool, cx: &mut Context<MentionInputState>) {
        if self.window_active == active {
            return;
        }
        self.window_active = active;
        if !self.enabled {
            return;
        }
        if active {
            self.visible = true;
            self.idle_ticks = 0;
            cx.notify();
            self.schedule_blink(cx);
        } else {
            self.bump_epoch();
            self.visible = true;
        }
    }

    pub fn sync_blurred(&mut self, cx: &mut Context<MentionInputState>) {
        if !self.enabled {
            return;
        }
        self.enabled = false;
        self.visible = false;
        self.bump_epoch();
        cx.notify();
    }

    pub fn pause_blinking(&mut self, cx: &mut Context<MentionInputState>) {
        if !self.enabled {
            return;
        }
        self.visible = true;
        self.idle_ticks = 0;
        cx.notify();

        let epoch = self.bump_epoch();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BLINK_PAUSE).await;
            this.update(cx, |this, cx| {
                if this.caret_blink.enabled
                    && this.caret_blink.window_active
                    && this.caret_blink.blink_epoch == epoch
                {
                    this.caret_blink.schedule_blink(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn schedule_blink(&mut self, cx: &mut Context<MentionInputState>) {
        let epoch = self.bump_epoch();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BLINK_INTERVAL).await;
            this.update(cx, |this, cx| this.caret_blink.blink_tick(epoch, cx))
                .ok();
        })
        .detach();
    }

    fn blink_tick(&mut self, epoch: usize, cx: &mut Context<MentionInputState>) {
        if !self.enabled || !self.window_active || epoch != self.blink_epoch {
            return;
        }
        self.idle_ticks = self.idle_ticks.saturating_add(1);
        if self.idle_ticks >= MAX_IDLE_TICKS {
            if !self.visible {
                self.visible = true;
                cx.notify();
            }
            return;
        }
        self.visible = !self.visible;
        cx.notify();
        self.schedule_blink(cx);
    }

    fn bump_epoch(&mut self) -> usize {
        self.blink_epoch = self.blink_epoch.wrapping_add(1);
        self.blink_epoch
    }
}
