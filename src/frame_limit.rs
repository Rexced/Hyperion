//! Caps how often the UI redraws. Vsync is off (it froze the app on hidden Hyprland
//! workspaces), and without it egui draws the next frame as soon as anything asks
//! for one — hundreds of times a second during animations. This does the waiting
//! vsync used to do, without depending on the compositor.

use std::time::{Duration, Instant};

/// Used when the display's refresh rate is unknown.
const FALLBACK_HZ: f32 = 60.0;

pub struct FrameLimiter {
    budget: Duration,
    last: Option<Instant>,
}

impl FrameLimiter {
    pub fn new(hz: Option<f32>) -> Self {
        let hz = hz
            .filter(|hz| (30.0..=1000.0).contains(hz))
            .unwrap_or(FALLBACK_HZ);
        Self {
            budget: Duration::from_secs_f32(1.0 / hz),
            last: None,
        }
    }

    /// The fastest refresh rate among the monitors, where the platform tells us.
    pub fn display_hz() -> Option<f32> {
        crate::hypr::Hypr::detect()?
            .monitors()
            .iter()
            .map(|m| m.refresh_rate)
            .reduce(f32::max)
    }

    /// Call at the start of every frame: sleeps until one frame period has passed
    /// since the previous frame started.
    pub fn wait(&mut self) {
        if let Some(last) = self.last {
            let left = remaining(last.elapsed(), self.budget);
            if !left.is_zero() {
                std::thread::sleep(left);
            }
        }
        self.last = Some(Instant::now());
    }
}

fn remaining(elapsed: Duration, budget: Duration) -> Duration {
    budget.saturating_sub(elapsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_follows_the_display_and_ignores_nonsense() {
        let ms = |l: FrameLimiter| l.budget.as_secs_f32() * 1000.0;
        assert!((ms(FrameLimiter::new(Some(75.0))) - 13.33).abs() < 0.01);
        assert!((ms(FrameLimiter::new(None)) - 16.67).abs() < 0.01);
        assert!((ms(FrameLimiter::new(Some(0.0))) - 16.67).abs() < 0.01);
    }

    #[test]
    fn waits_only_for_the_rest_of_the_frame() {
        let budget = Duration::from_millis(13);
        assert_eq!(
            remaining(Duration::from_millis(5), budget),
            Duration::from_millis(8)
        );
        assert_eq!(remaining(Duration::from_millis(20), budget), Duration::ZERO);
    }
}
