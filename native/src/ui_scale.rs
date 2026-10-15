//! Whole-interface scale (#277): one process-wide factor that every design-time size goes through.
//!
//! Why a leaf module with its own tests: a scale step that skips a value, drifts after repeated in / out, or
//! cannot return to exactly 1.0 shows up as a UI that never quite goes back to normal. The layout code only
//! calls `px` (a drop-in for `gpui::px`) and `scaled`, so the whole window follows one number.
//!
//! What does NOT go through here: coordinates and offsets that come from the window (mouse positions, scroll
//! deltas, measured viewport sizes) are already real pixels and must stay `gpui::px`. The terminal grid has
//! its own font zoom and is independent.

use std::sync::atomic::{AtomicU32, Ordering};

/// Allowed factors, smallest to largest. 1.0 is the design size.
pub const STEPS: [f32; 7] = [0.8, 0.9, 1.0, 1.1, 1.25, 1.4, 1.6];
pub const DEFAULT: f32 = 1.0;

static SCALE: AtomicU32 = AtomicU32::new(0x3F80_0000); // 1.0f32

pub fn get() -> f32 {
    f32::from_bits(SCALE.load(Ordering::Relaxed))
}

/// Set the factor (snapped to the nearest step, so an odd value from a hand-edited state file stays sane).
pub fn set(v: f32) {
    SCALE.store(snap(v).to_bits(), Ordering::Relaxed);
}

/// Nearest allowed step; NaN / non-positive → 1.0.
pub fn snap(v: f32) -> f32 {
    if !v.is_finite() || v <= 0.0 {
        return DEFAULT;
    }
    STEPS.iter().copied().min_by(|a, b| (a - v).abs().total_cmp(&(b - v).abs())).unwrap_or(DEFAULT)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    In,
    Out,
    Reset,
}

/// The factor after one zoom action. Ends are sticky (no wrap-around).
pub fn next(cur: f32, step: Step) -> f32 {
    let cur = snap(cur);
    let i = STEPS.iter().position(|s| *s == cur).unwrap_or(2);
    match step {
        Step::Reset => DEFAULT,
        Step::In => STEPS[(i + 1).min(STEPS.len() - 1)],
        Step::Out => STEPS[i.saturating_sub(1)],
    }
}

/// A design-time size in logical pixels, scaled.
pub fn scaled(v: f32) -> f32 {
    scaled_with(v, get())
}

/// `scaled` with an explicit factor (also what the tests use, so they never touch the process-wide value).
pub fn scaled_with(v: f32, factor: f32) -> f32 {
    v * factor
}

/// Drop-in for `gpui::px` for design-time sizes.
pub fn px(v: f32) -> gpui::Pixels {
    gpui::px(scaled(v))
}

/// Label for the settings / toast: `110%`.
pub fn percent(v: f32) -> String {
    format!("{}%", (snap(v) * 100.0).round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_sorted_and_contain_one() {
        assert!(STEPS.windows(2).all(|w| w[0] < w[1]));
        assert!(STEPS.contains(&1.0));
    }

    #[test]
    fn zoom_in_and_out_walk_the_steps_and_stop_at_the_ends() {
        assert_eq!(next(1.0, Step::In), 1.1);
        assert_eq!(next(1.1, Step::In), 1.25);
        assert_eq!(next(1.0, Step::Out), 0.9);
        assert_eq!(next(1.6, Step::In), 1.6, "no wrap past the largest");
        assert_eq!(next(0.8, Step::Out), 0.8, "no wrap past the smallest");
    }

    #[test]
    fn in_then_out_returns_to_the_start_and_reset_is_exactly_one() {
        let mut v = 1.0;
        for _ in 0..3 {
            v = next(v, Step::In);
        }
        for _ in 0..3 {
            v = next(v, Step::Out);
        }
        assert_eq!(v, 1.0, "no float drift: values come from a table");
        assert_eq!(next(1.4, Step::Reset), 1.0);
    }

    #[test]
    fn odd_values_snap_to_the_nearest_step() {
        assert_eq!(snap(1.12), 1.1);
        assert_eq!(snap(9.0), 1.6);
        assert_eq!(snap(0.1), 0.8);
        assert_eq!(snap(f32::NAN), 1.0);
        assert_eq!(snap(-2.0), 1.0);
        assert_eq!(next(1.12, Step::In), 1.25, "an off-table value is snapped first");
    }

    #[test]
    fn scaled_sizes_follow_the_factor() {
        assert_eq!(scaled_with(12.0, 1.25), 15.0);
        assert_eq!(scaled_with(12.0, 1.0), 12.0);
        assert_eq!(percent(1.25), "125%");
    }
}
