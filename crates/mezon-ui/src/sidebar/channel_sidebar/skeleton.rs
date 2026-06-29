pub(super) const THRESHOLD_MS: u64 = 500;
pub(super) const FADE_IN_MS: u64 = 150;
pub(super) const SETTLE_MS: u64 = 250;
pub(super) const FADE_OUT_MS: u64 = 180;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum Phase {
    #[default]
    Hidden,
    Pending,
    Showing,
    Settling,
    FadingOut,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Timer {
    Keep,
    Drop,
    Threshold,
    Settle,
    Unmount,
}

#[derive(Clone, Copy)]
pub(super) enum Input {
    Sync { loading: bool, same_clan: bool },
    ThresholdElapsed,
    SettleElapsed,
    UnmountElapsed,
}

pub(super) enum TimerAction {
    Keep,
    Clear,
    Arm(Input, u64),
}

pub(super) fn timer_action(timer: Timer) -> TimerAction {
    match timer {
        Timer::Keep => TimerAction::Keep,
        Timer::Drop => TimerAction::Clear,
        Timer::Threshold => TimerAction::Arm(Input::ThresholdElapsed, THRESHOLD_MS),
        Timer::Settle => TimerAction::Arm(Input::SettleElapsed, SETTLE_MS),
        Timer::Unmount => TimerAction::Arm(Input::UnmountElapsed, FADE_OUT_MS),
    }
}

pub(super) fn transition(phase: Phase, input: Input) -> (Phase, Timer) {
    match input {
        Input::Sync { loading, same_clan } => {
            let phase = if same_clan { phase } else { Phase::Hidden };
            match (phase, loading) {
                (Phase::Hidden, true) => (Phase::Pending, Timer::Threshold),
                (Phase::Hidden, false) => (
                    Phase::Hidden,
                    if same_clan { Timer::Keep } else { Timer::Drop },
                ),
                (Phase::Pending, true) => (Phase::Pending, Timer::Keep),
                (Phase::Pending, false) => (Phase::Hidden, Timer::Drop),
                (Phase::Showing, true) => (Phase::Showing, Timer::Keep),
                (Phase::Showing, false) => (Phase::Settling, Timer::Settle),
                (Phase::Settling, true) => (Phase::Showing, Timer::Drop),
                (Phase::Settling, false) => (Phase::Settling, Timer::Keep),
                (Phase::FadingOut, true) => (Phase::Showing, Timer::Drop),
                (Phase::FadingOut, false) => (Phase::FadingOut, Timer::Keep),
            }
        }
        Input::ThresholdElapsed => match phase {
            Phase::Pending => (Phase::Showing, Timer::Keep),
            other => (other, Timer::Keep),
        },
        Input::SettleElapsed => match phase {
            Phase::Settling => (Phase::FadingOut, Timer::Unmount),
            other => (other, Timer::Keep),
        },
        Input::UnmountElapsed => match phase {
            Phase::FadingOut => (Phase::Hidden, Timer::Keep),
            other => (other, Timer::Keep),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sync(loading: bool, same_clan: bool) -> Input {
        Input::Sync { loading, same_clan }
    }

    #[test]
    fn fast_load_arms_then_cancels_without_showing() {
        assert_eq!(
            transition(Phase::Hidden, sync(true, true)),
            (Phase::Pending, Timer::Threshold)
        );
        assert_eq!(
            transition(Phase::Pending, sync(false, true)),
            (Phase::Hidden, Timer::Drop)
        );
    }

    #[test]
    fn slow_load_shows_after_threshold() {
        assert_eq!(
            transition(Phase::Pending, sync(true, true)),
            (Phase::Pending, Timer::Keep)
        );
        assert_eq!(
            transition(Phase::Pending, Input::ThresholdElapsed),
            (Phase::Showing, Timer::Keep)
        );
    }

    #[test]
    fn loaded_settles_then_fades_then_unmounts() {
        assert_eq!(
            transition(Phase::Showing, sync(false, true)),
            (Phase::Settling, Timer::Settle)
        );
        assert_eq!(
            transition(Phase::Settling, Input::SettleElapsed),
            (Phase::FadingOut, Timer::Unmount)
        );
        assert_eq!(
            transition(Phase::FadingOut, Input::UnmountElapsed),
            (Phase::Hidden, Timer::Keep)
        );
    }

    #[test]
    fn settle_tick_arms_unmount() {
        let (next, timer) = transition(Phase::Settling, Input::SettleElapsed);
        assert_eq!(next, Phase::FadingOut);
        assert!(matches!(
            timer_action(timer),
            TimerAction::Arm(Input::UnmountElapsed, _)
        ));
    }

    #[test]
    fn clan_change_resets_then_rethresholds() {
        assert_eq!(
            transition(Phase::Showing, sync(true, false)),
            (Phase::Pending, Timer::Threshold)
        );
    }
}
