//! Lock-free peak meters for the realtime audio callbacks. Each meter holds the
//! integer magnitude |sample| (0..=32768); the level-emitter thread reads-and-resets
//! and normalizes to 0.0..=1.0. No locks or allocation in the write path.

use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Default)]
pub struct Meters {
    input_peak: AtomicU32,
    output_peak: AtomicU32,
}

impl Meters {
    /// Record an input-frame peak (running max until the next `take`).
    pub fn record_input(&self, peak: u32) {
        self.input_peak.fetch_max(peak, Ordering::Relaxed);
    }

    /// Record an output-frame peak (running max until the next `take`).
    pub fn record_output(&self, peak: u32) {
        self.output_peak.fetch_max(peak, Ordering::Relaxed);
    }

    /// Read-and-reset both peaks, normalized to 0.0..=1.0.
    pub fn take(&self) -> (f32, f32) {
        let i = self.input_peak.swap(0, Ordering::Relaxed);
        let o = self.output_peak.swap(0, Ordering::Relaxed);
        (norm(i), norm(o))
    }
}

fn norm(mag: u32) -> f32 {
    (mag as f32 / 32768.0).min(1.0)
}

/// Peak magnitude of a mono i16 frame, allocation-free. `s as i32` avoids the
/// `i16::abs` overflow at `i16::MIN`.
pub fn frame_peak(samples: &[i16]) -> u32 {
    samples
        .iter()
        .map(|&s| (s as i32).unsigned_abs())
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_peak_is_max_magnitude() {
        assert_eq!(frame_peak(&[0, 100, -200, 50]), 200);
        assert_eq!(frame_peak(&[]), 0);
        assert_eq!(frame_peak(&[i16::MIN]), 32768); // |−32768| must not overflow i16
    }

    #[test]
    fn record_then_take_normalizes_and_resets() {
        let m = Meters::default();
        m.record_input(32768);
        m.record_output(16384);
        let (i, o) = m.take();
        assert!((i - 1.0).abs() < 1e-6);
        assert!((o - 0.5).abs() < 1e-3);
        assert_eq!(m.take(), (0.0, 0.0));
    }

    #[test]
    fn record_keeps_running_max_between_takes() {
        let m = Meters::default();
        m.record_input(100);
        m.record_input(300);
        m.record_input(50);
        let (i, _) = m.take();
        assert!((i - 300.0 / 32768.0).abs() < 1e-6);
    }
}
