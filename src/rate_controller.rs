use std::time::{Duration, Instant};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::types::RateProfile;

pub struct RateController {
    profile: RateProfile,
    base_rate: f64,
    max_rate: f64,
    rng: StdRng,
    start: Instant, // Instant::now() = std::chrono::high_resolution_clock::now()
}

impl RateController {
    pub fn new(profile: RateProfile, base_rate: f64, max_rate: f64) -> Self {
        Self {
            profile,
            base_rate,
            max_rate,
            rng: StdRng::seed_from_u64(42),
            start: Instant::now(),
        }
    }

    pub fn sleep_duration(&mut self) -> Duration {
        let elapsed = self.start.elapsed().as_secs_f64();

        let rate = match self.profile {
            RateProfile::Constant => self.base_rate,

            RateProfile::SineWave => {
                const PERIOD: f64 = 30.0;
                let amp = (self.max_rate - self.base_rate) / 2.0;
                let mid = self.base_rate + amp;
                mid + amp * (2.0 * std::f64::consts::PI * elapsed / PERIOD).sin()
            }

            RateProfile::Burst => {
                // 5% chance of max burst — same logic as C++
                if self.rng.gen_bool(0.05) { self.max_rate } else { self.base_rate }
            }

            RateProfile::Ramp => {
                const RAMP_SECS: f64 = 60.0;
                let t = (elapsed / RAMP_SECS).min(1.0);
                self.base_rate + t * (self.max_rate - self.base_rate)
            }
        };

        let rate = rate.max(1.0);
        Duration::from_micros((1_000_000.0 / rate) as u64)
    }
}
