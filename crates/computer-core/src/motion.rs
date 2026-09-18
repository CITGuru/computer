//! A pointer path: where the pointer goes, step by step, and how long it rests at each.

use computer_types::{Motion, Point};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub at: Point,
    /// After arriving, before the next step.
    pub pause: Duration,
}

/// A step every twelve pixels and at least one a frame, eased so the pointer
/// starts and stops gently. `Human` bends the path to one side by up to eight
/// percent of the distance, capped at 36 px, with the side and the size drawn
/// from `seed`, so the same seed draws the same path. The last step is `to`.
pub fn path(from: Point, to: Point, motion: Motion, seed: u64) -> Vec<Step> {
    let dx = f64::from(to.x) - f64::from(from.x);
    let dy = f64::from(to.y) - f64::from(from.y);
    let distance = dx.hypot(dy);

    if motion.is_instant() || distance < 1.0 {
        return vec![Step {
            at: to,
            pause: Duration::ZERO,
        }];
    }

    let duration_ms = match motion {
        Motion::Human => (80.0 + distance * 0.35).clamp(100.0, 700.0),
        _ => (40.0 + distance * 0.2).clamp(60.0, 350.0),
    };
    let spatial = ((distance / 12.0).ceil() as usize).clamp(1, 60);
    let steps = spatial.max((duration_ms / 16.0).ceil() as usize).min(240);
    let bend = match motion {
        Motion::Human => unit(seed) * (distance * 0.08).min(36.0),
        _ => 0.0,
    };
    let (across_x, across_y) = (-dy / distance, dx / distance);
    let pause = Duration::from_secs_f64(duration_ms / 1000.0 / steps as f64);

    (1..=steps)
        .map(|step| {
            if step == steps {
                return Step { at: to, pause };
            }
            let t = step as f64 / steps as f64;
            let eased = t * t * (3.0 - 2.0 * t);
            let curve = 4.0 * t * (1.0 - t) * bend;
            Step {
                at: Point {
                    x: (f64::from(from.x) + dx * eased + across_x * curve)
                        .round()
                        .max(0.0) as u32,
                    y: (f64::from(from.y) + dy * eased + across_y * curve)
                        .round()
                        .max(0.0) as u32,
                },
                pause,
            }
        })
        .collect()
}

/// A number in [-1, 1] from the seed, the same every time.
fn unit(seed: u64) -> f64 {
    let mixed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((mixed >> 11) as f64) / ((1_u64 << 53) as f64) * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_to() -> (Point, Point) {
        (Point { x: 100, y: 100 }, Point { x: 700, y: 400 })
    }

    fn off_line(at: Point, from: Point, to: Point) -> f64 {
        let (dx, dy) = (
            f64::from(to.x) - f64::from(from.x),
            f64::from(to.y) - f64::from(from.y),
        );
        let (px, py) = (
            f64::from(at.x) - f64::from(from.x),
            f64::from(at.y) - f64::from(from.y),
        );
        (dx * py - dy * px).abs() / dx.hypot(dy)
    }

    #[test]
    fn test_instant_is_one_step_with_no_pause() {
        let (from, to) = from_to();
        let steps = path(from, to, Motion::Instant, 7);

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].at, to);
        assert_eq!(steps[0].pause, Duration::ZERO);
    }

    #[test]
    fn test_every_path_ends_exactly_on_the_target() {
        let (from, to) = from_to();
        for motion in [Motion::Smooth, Motion::Human] {
            for seed in [0, 1, 42, u64::MAX] {
                assert_eq!(path(from, to, motion, seed).last().map(|s| s.at), Some(to));
            }
        }
    }

    #[test]
    fn test_a_seed_draws_the_same_path_and_another_seed_a_different_one() {
        let (from, to) = from_to();
        assert_eq!(
            path(from, to, Motion::Human, 42),
            path(from, to, Motion::Human, 42)
        );
        assert_ne!(
            path(from, to, Motion::Human, 42),
            path(from, to, Motion::Human, 43)
        );
    }

    #[test]
    fn test_smooth_stays_on_the_line_and_human_bends_within_bounds() {
        let (from, to) = from_to();
        for step in path(from, to, Motion::Smooth, 3) {
            assert!(
                off_line(step.at, from, to) < 1.0,
                "smooth is straight: {step:?}"
            );
        }
        let mut widest: f64 = 0.0;
        for seed in 0..50 {
            for step in path(from, to, Motion::Human, seed) {
                let off = off_line(step.at, from, to);
                assert!(off <= 36.6, "the bend is capped at 36 px: {off}");
                widest = widest.max(off);
            }
        }
        assert!(widest > 5.0, "and some seed bends it: {widest}");
    }

    #[test]
    fn test_steps_and_time_scale_with_distance_and_stay_bounded() {
        let near = path(
            Point { x: 0, y: 0 },
            Point { x: 20, y: 0 },
            Motion::Human,
            1,
        );
        let far = path(
            Point { x: 0, y: 0 },
            Point { x: 3000, y: 0 },
            Motion::Human,
            1,
        );
        let total = |steps: &[Step]| steps.iter().map(|s| s.pause).sum::<Duration>();

        assert!(near.len() < far.len());
        assert!(far.len() <= 240);
        assert!(
            total(&near) >= Duration::from_millis(99),
            "{:?}",
            total(&near)
        );
        assert!(
            total(&far) <= Duration::from_millis(701),
            "{:?}",
            total(&far)
        );
    }

    #[test]
    fn test_a_path_never_leaves_the_screen_on_the_near_side() {
        let steps = path(
            Point { x: 2, y: 2 },
            Point { x: 300, y: 2 },
            Motion::Human,
            5,
        );
        assert!(
            steps.iter().all(|s| s.at.y < 1_000),
            "a bend near the edge is clamped, not wrapped"
        );
    }
}
