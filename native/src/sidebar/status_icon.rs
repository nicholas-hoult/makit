//! Status look shared by the sidebar rows and the pane tabs (#245, redesigned in #290 / #278).
//!
//! Why a leaf module with its own tests: the two places used to speak different status languages, and the
//! later shared 8px dots told four states apart by colour alone (waiting / busy / idle were three similar
//! small blobs, the one that needs you was the least noticeable). Now every state has its own silhouette:
//!   waiting = filled disc with three dots cut out, busy = ring with a bright arc,
//!   idle = ring with a tick, stopped = dashed ring.
//! Nothing moves: any motion has to be a repaint of the whole window (GPUI has no partial redraw, and an animated
//! image is replayed by GPUI itself with a repaint per display frame), which measured ~28% of a core in the shipped
//! app with a few running sessions. The silhouettes already carry the meaning.
//! A wrong table here shows up as tabs and sidebar disagreeing about the same session, or as two states
//! drawing the same shape.

use gpui::{px, svg, AnyElement, Hsla, IntoElement, Styled};

use super::groups::RunState;
use crate::theme::Theme;

/// Asset path of the icon for a state (registered in `crate::assets`)
pub fn path(rs: RunState) -> &'static str {
    match rs {
        RunState::Waiting => "icons/status-waiting.svg",
        RunState::Busy => "icons/status-busy.svg",
        RunState::Idle => "icons/status-idle.svg",
        RunState::Stopped => "icons/status-stopped.svg",
    }
}

pub fn color(rs: RunState, t: &Theme) -> Hsla {
    let mut c = match rs {
        RunState::Waiting => t.warning,
        RunState::Busy => t.info,
        RunState::Idle => t.success,
        RunState::Stopped => t.fg_muted,
    };
    if rs == RunState::Stopped {
        c.a *= 0.8;
    }
    c
}

/// The icon itself, `size` px square. Sidebar rows use 14, tabs 12.
pub fn render(rs: RunState, size: f32, t: &Theme) -> AnyElement {
    svg().path(path(rs)).size(px(size)).flex_none().text_color(color(rs, t)).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AssetSource;

    const ALL: [RunState; 4] = [RunState::Waiting, RunState::Busy, RunState::Idle, RunState::Stopped];

    #[test]
    fn every_state_has_its_own_registered_icon() {
        let mut seen = std::collections::HashSet::new();
        for rs in ALL {
            assert!(seen.insert(path(rs)), "two states share the icon {}", path(rs));
            assert!(crate::assets::Assets.load(path(rs)).unwrap().is_some(), "{} is not registered in assets.rs", path(rs));
        }
    }

    #[test]
    fn nothing_requests_a_repaint_tick() {
        // The pulse clock repaints the whole window when anyone asks for ticks. Drawing an icon must never ask (#290).
        let _ = crate::pulse::take_want();
        let t = crate::theme::Theme::by_id("embark", &[]);
        for rs in ALL {
            let _ = render(rs, 14., &t);
        }
        assert!(!crate::pulse::take_want(), "drawing a status icon asked for repaint ticks");
    }
}
