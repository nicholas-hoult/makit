//! Status dot look shared by the sidebar rows and the pane tabs (#245).
//!
//! Why a leaf module with its own tests: the two places used to speak different status languages
//! (amber/blue/green dots in the sidebar, red ● / yellow ⚡ emoji / grey ○ on tabs). Shape now only says
//! alive vs stopped; colour and breathing say what the session is doing. A wrong table here shows up as
//! tabs and sidebar disagreeing about the same session.

use gpui::Hsla;

use super::groups::RunState;
use crate::theme::Theme;

/// Breathing of the dot as `(period_ms, min_opacity)`. Waiting needs an answer, so it breathes faster and
/// deeper than busy; idle and stopped stay still.
pub fn pulse(rs: RunState) -> Option<(u64, f32)> {
    match rs {
        RunState::Waiting => Some((1200, 0.3)),
        RunState::Busy => Some((2000, 0.4)),
        RunState::Idle | RunState::Stopped => None,
    }
}

pub fn color(rs: RunState, t: &Theme) -> Hsla {
    let mut c = match rs {
        RunState::Waiting => t.warning,
        RunState::Busy => t.info,
        RunState::Idle => {
            let mut c = t.success;
            c.a *= 0.7;
            c
        }
        RunState::Stopped => t.fg_muted,
    };
    if rs == RunState::Stopped {
        c.a *= 0.4;
    }
    c
}

/// Opacity for the current frame; registers with the shared low-rate pulse clock when the dot breathes
pub fn opacity(rs: RunState) -> f32 {
    match pulse(rs) {
        Some((period, min)) => {
            crate::pulse::want_ticks();
            crate::pulse::opacity(crate::pulse::now_ms(), period, min)
        }
        None => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_states_that_need_attention_breathe() {
        assert!(pulse(RunState::Waiting).is_some());
        assert!(pulse(RunState::Busy).is_some());
        assert_eq!(pulse(RunState::Idle), None);
        assert_eq!(pulse(RunState::Stopped), None);
    }

    #[test]
    fn waiting_breathes_faster_and_deeper_than_busy() {
        let (wp, wmin) = pulse(RunState::Waiting).unwrap();
        let (bp, bmin) = pulse(RunState::Busy).unwrap();
        assert!(wp < bp, "waiting period must be shorter");
        assert!(wmin < bmin, "waiting must dip lower");
    }
}
