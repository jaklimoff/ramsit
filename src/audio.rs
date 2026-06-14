//! Voice capture/playback core. No UI dependencies: the network worker drives it
//! and reads back an `AudioState` snapshot, so a future Tauri front-end reuses it
//! unchanged.

/// Opus-supported sample rate we run at; both CoreAudio and PipeWire provide it
/// on request, so we never resample.
pub const SAMPLE_RATE: u32 = 48_000;
/// Samples per 20 ms mono frame at `SAMPLE_RATE`.
pub const FRAME_SAMPLES: usize = 960;
/// Volume percent bounds applied as digital gain.
pub const VOL_MIN: i32 = 0;
pub const VOL_MAX: i32 = 200;

/// Snapshot of audio control state, sent to the UI after every change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioState {
    pub muted: bool,
    pub input_vol: u8,  // percent, 0..=200
    pub output_vol: u8, // percent, 0..=200
}

/// Clamp a volume percent into the supported range.
pub fn clamp_vol(v: i32) -> u32 {
    v.clamp(VOL_MIN, VOL_MAX) as u32
}

/// Scale one sample by `pct`/100, saturating at i16 bounds.
pub fn gain_sample(s: i16, pct: u32) -> i16 {
    if pct == 100 {
        return s;
    }
    ((s as i32 * pct as i32) / 100).clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// Apply `gain_sample` across a buffer in place.
pub fn apply_gain(samples: &mut [i16], pct: u32) {
    for s in samples.iter_mut() {
        *s = gain_sample(*s, pct);
    }
}

/// Downmix interleaved `channels`-channel audio to mono by averaging each frame.
pub fn downmix(interleaved: &[i16], channels: usize) -> Vec<i16> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(channels)
        .map(|f| (f.iter().map(|&s| s as i32).sum::<i32>() / channels as i32) as i16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_vol_bounds() {
        assert_eq!(clamp_vol(-30), 0);
        assert_eq!(clamp_vol(250), 200);
        assert_eq!(clamp_vol(110), 110);
    }

    #[test]
    fn gain_unity_is_noop() {
        assert_eq!(gain_sample(123, 100), 123);
        let mut s = [123i16, -456];
        apply_gain(&mut s, 100);
        assert_eq!(s, [123, -456]);
    }

    #[test]
    fn gain_scales_and_saturates() {
        assert_eq!(gain_sample(100, 200), 200);
        assert_eq!(gain_sample(20_000, 200), i16::MAX); // 40000 saturates
        assert_eq!(gain_sample(-20_000, 200), i16::MIN);
        assert_eq!(gain_sample(100, 50), 50);
    }

    #[test]
    fn downmix_averages_and_passes_mono() {
        assert_eq!(downmix(&[10, 30, -10, 10], 2), vec![20, 0]);
        assert_eq!(downmix(&[5, 7], 1), vec![5, 7]);
    }

    #[test]
    fn opus_roundtrip_at_frame_size() {
        // Validates SAMPLE_RATE/FRAME_SAMPLES are a legal Opus frame and that the
        // decode output buffer sizing in the engine task (FRAME_SAMPLES) is correct.
        let mut enc =
            opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let mut dec = opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono).unwrap();
        let pcm = vec![0i16; FRAME_SAMPLES];
        let mut enc_buf = [0u8; 4000];
        let n = enc.encode(&pcm, &mut enc_buf).unwrap();
        assert!(n > 0);
        let mut dec_buf = [0i16; FRAME_SAMPLES];
        let m = dec.decode(&enc_buf[..n], &mut dec_buf, false).unwrap();
        assert_eq!(m, FRAME_SAMPLES);
    }
}
