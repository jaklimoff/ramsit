//! Test-tone source for the audio self-test. Uses a persistent phase accumulator
//! (never `sin` of an absolute, overflow-prone sample index) and is allocation-free
//! so it can run inside the realtime output callback.

use std::f32::consts::TAU;

/// ~0.3 of i16 full-scale — clearly audible without being startling.
pub const AMPLITUDE: i16 = 9830;

pub struct ToneGen {
    pub(crate) phase: f32,
    pub(crate) step: f32,
}

impl ToneGen {
    pub fn new(freq_hz: f32, sample_rate: u32) -> Self {
        Self {
            phase: 0.0,
            step: TAU * freq_hz / sample_rate as f32,
        }
    }

    /// Next mono sample; advances and wraps the phase.
    pub fn next_sample(&mut self) -> i16 {
        let s = (self.phase.sin() * AMPLITUDE as f32) as i16;
        self.phase += self.step;
        if self.phase >= TAU {
            self.phase -= TAU;
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn step_matches_frequency_and_rate() {
        let g = ToneGen::new(480.0, 48_000);
        assert!((g.step - TAU * 480.0 / 48_000.0).abs() < 1e-6);
    }

    #[test]
    fn samples_stay_within_amplitude() {
        let mut g = ToneGen::new(440.0, 48_000);
        for _ in 0..10_000 {
            let s = g.next_sample();
            assert!(s.abs() <= AMPLITUDE, "sample {s} exceeded amplitude {AMPLITUDE}");
        }
    }

    #[test]
    fn phase_wraps_and_output_oscillates() {
        // A 1 kHz tone at 8 kHz: 8 samples per period. Over 800 samples the sign
        // must change many times (not a stuck/constant signal), and phase stays bounded.
        let mut g = ToneGen::new(1_000.0, 8_000);
        let mut sign_changes = 0;
        let mut prev = g.next_sample();
        for _ in 0..800 {
            let s = g.next_sample();
            if (s >= 0) != (prev >= 0) {
                sign_changes += 1;
            }
            prev = s;
            assert!(g.phase >= 0.0 && g.phase < TAU);
        }
        assert!(sign_changes > 100, "expected an oscillating tone, got {sign_changes} sign changes");
    }
}
