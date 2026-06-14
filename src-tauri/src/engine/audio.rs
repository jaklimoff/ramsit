//! Voice capture/playback core. No UI dependencies: the network worker drives it
//! and reads back an `AudioState` snapshot, so a future Tauri front-end reuses it
//! unchanged.

use crate::meter::{frame_peak, Meters};
use crate::tone::ToneGen;
use anyhow::{anyhow, Result};
use cpal::traits::DeviceTrait;
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use rubato::{FftFixedIn, Resampler};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// Opus-supported sample rate the codec and wire protocol run at. Devices that
/// don't offer 48 kHz are driven at their native rate and resampled to/from this
/// at the capture/playback boundary (see `MonoResampler`).
pub const SAMPLE_RATE: u32 = 48_000;
/// Samples per 20 ms mono frame at `SAMPLE_RATE`.
pub const FRAME_SAMPLES: usize = 960;
/// Volume percent bounds applied as digital gain.
pub const VOL_MIN: i32 = 0;
pub const VOL_MAX: i32 = 200;

/// Decoded samples buffered before playback starts (≈40 ms) to absorb jitter.
const JITTER_PRIME: usize = FRAME_SAMPLES * 2;

/// Snapshot of audio control state, sent to the UI after every change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioState {
    pub muted: bool,
    pub input_vol: u8,  // percent, 0..=200
    pub output_vol: u8, // percent, 0..=200
}

/// Atomics the realtime callbacks and encoder thread read each tick. Owned behind
/// an `Arc` shared by the streams and the audio engine.
pub(crate) struct Controls {
    pub(crate) muted: AtomicBool,
    pub(crate) input_vol: AtomicU32,  // percent
    pub(crate) output_vol: AtomicU32, // percent
}

impl Controls {
    pub(crate) fn snapshot(&self) -> AudioState {
        AudioState {
            muted: self.muted.load(Ordering::Relaxed),
            input_vol: self.input_vol.load(Ordering::Relaxed) as u8,
            output_vol: self.output_vol.load(Ordering::Relaxed) as u8,
        }
    }
}

/// Keeps the cpal streams alive. `!Send` (cpal streams are not Send), so it stays
/// on the thread that built it and is dropped when the call ends.
pub struct AudioStreams {
    pub(crate) _input: cpal::Stream,
    pub(crate) _output: cpal::Stream,
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

/// Streaming mono i16 resampler between two sample rates, used at the device
/// boundary so the Opus pipeline always runs at `SAMPLE_RATE`. Buffers input so
/// callers may push arbitrary-length chunks; runs off the realtime audio thread.
pub struct MonoResampler {
    inner: FftFixedIn<f32>,
    chunk_in: usize,
    pending: Vec<f32>,
}

impl MonoResampler {
    /// Build a resampler from `from_hz` to `to_hz`. The input chunk is ~20 ms so
    /// the FFT sizes align to the rate gcd and latency stays low.
    pub fn new(from_hz: u32, to_hz: u32) -> Result<Self> {
        let chunk_in = (from_hz as usize / 50).max(1);
        let inner = FftFixedIn::<f32>::new(from_hz as usize, to_hz as usize, chunk_in, 1, 1)
            .map_err(|e| anyhow!("resampler init {from_hz}->{to_hz}: {e}"))?;
        let chunk_in = inner.input_frames_next();
        Ok(Self {
            inner,
            chunk_in,
            pending: Vec::new(),
        })
    }

    /// Resample `input`, returning whatever full output frames are ready. Samples
    /// that don't fill a chunk are retained for the next call.
    pub fn process(&mut self, input: &[i16]) -> Vec<i16> {
        self.pending
            .extend(input.iter().map(|&s| s as f32 / 32768.0));
        let mut out = Vec::new();
        while self.pending.len() >= self.chunk_in {
            let chunk: Vec<f32> = self.pending.drain(..self.chunk_in).collect();
            match self.inner.process(&[chunk], None) {
                Ok(res) => out.extend(res[0].iter().map(|&s| {
                    (s * 32768.0).clamp(i16::MIN as f32, i16::MAX as f32) as i16
                })),
                Err(e) => log::warn!("audio: resample failed: {e}"),
            }
        }
        out
    }
}

/// True if the device advertises a config range covering `SAMPLE_RATE`, so we can
/// run it at 48 kHz and skip resampling entirely.
pub(crate) fn prefers_48k<I: Iterator<Item = cpal::SupportedStreamConfigRange>>(
    supported: Option<I>,
) -> bool {
    supported
        .map(|mut c| {
            c.any(|r| r.min_sample_rate() <= SAMPLE_RATE && r.max_sample_rate() >= SAMPLE_RATE)
        })
        .unwrap_or(false)
}

pub(crate) fn build_input(
    dev: &cpal::Device,
    cfg: StreamConfig,
    channels: usize,
    fmt: SampleFormat,
    meters: Arc<Meters>,
    pcm_tx: Sender<Vec<i16>>,
) -> Result<cpal::Stream> {
    match fmt {
        SampleFormat::F32 => input_stream::<f32>(dev, cfg, channels, meters, pcm_tx),
        SampleFormat::I16 => input_stream::<i16>(dev, cfg, channels, meters, pcm_tx),
        other => Err(anyhow!("unsupported input sample format {other:?}")),
    }
}

fn input_stream<T>(
    dev: &cpal::Device,
    cfg: StreamConfig,
    channels: usize,
    meters: Arc<Meters>,
    pcm_tx: Sender<Vec<i16>>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    i16: FromSample<T>,
{
    let stream = dev.build_input_stream(
        cfg,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let pcm: Vec<i16> = data.iter().map(|&s| i16::from_sample(s)).collect();
            let mono = downmix(&pcm, channels);
            meters.record_input(frame_peak(&mono));
            let _ = pcm_tx.send(mono);
        },
        |e| log::warn!("audio: input stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_output(
    dev: &cpal::Device,
    cfg: StreamConfig,
    channels: usize,
    fmt: SampleFormat,
    rate: u32,
    jitter: Arc<Mutex<VecDeque<i16>>>,
    controls: Arc<Controls>,
    meters: Arc<Meters>,
    tone_active: Arc<AtomicBool>,
) -> Result<cpal::Stream> {
    match fmt {
        SampleFormat::F32 => {
            output_stream::<f32>(dev, cfg, channels, rate, jitter, controls, meters, tone_active)
        }
        SampleFormat::I16 => {
            output_stream::<i16>(dev, cfg, channels, rate, jitter, controls, meters, tone_active)
        }
        other => Err(anyhow!("unsupported output sample format {other:?}")),
    }
}

#[allow(clippy::too_many_arguments)]
fn output_stream<T>(
    dev: &cpal::Device,
    cfg: StreamConfig,
    channels: usize,
    rate: u32,
    jitter: Arc<Mutex<VecDeque<i16>>>,
    controls: Arc<Controls>,
    meters: Arc<Meters>,
    tone_active: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<i16>,
{
    let mut playing = false; // owned by this FnMut; latches once primed
    let mut tone = ToneGen::new(440.0, rate);
    let stream = dev.build_output_stream(
        cfg,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let gain = controls.output_vol.load(Ordering::Relaxed);
            let toning = tone_active.load(Ordering::Relaxed);
            let mut jb = jitter.lock().unwrap();
            if !playing && jb.len() >= JITTER_PRIME {
                playing = true;
            }
            let mut peak = 0u32;
            for frame in data.chunks_mut(channels) {
                let sample = if toning {
                    gain_sample(tone.next_sample(), gain)
                } else if playing {
                    match jb.pop_front() {
                        Some(s) => gain_sample(s, gain),
                        None => {
                            playing = false; // underran; re-prime before next burst
                            0
                        }
                    }
                } else {
                    0
                };
                peak = peak.max((sample as i32).unsigned_abs());
                let out = T::from_sample(sample);
                for o in frame.iter_mut() {
                    *o = out;
                }
            }
            meters.record_output(peak);
        },
        |e| log::warn!("audio: output stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_volume_clamps_into_range() {
        // set_*_volume stores clamp_vol(pct as i32); verify the clamp contract.
        assert_eq!(clamp_vol(250), VOL_MAX as u32);
        assert_eq!(clamp_vol(-5), VOL_MIN as u32);
        assert_eq!(clamp_vol(80), 80);
    }

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
    fn resampler_scales_sample_count_by_ratio() {
        // 48k -> 24k halves the stream; feed 10 full input chunks (~200 ms).
        let mut r = MonoResampler::new(48_000, 24_000).unwrap();
        let input = vec![0i16; r.chunk_in * 10];
        let out = r.process(&input);
        assert_eq!(out.len(), input.len() / 2);
    }

    #[test]
    fn resampler_buffers_partial_chunks() {
        // Fewer samples than one chunk produce no output yet; the rest is retained.
        let mut r = MonoResampler::new(44_100, 48_000).unwrap();
        assert!(r.process(&[0i16; 10]).is_empty());
        assert_eq!(r.pending.len(), 10);
    }

    #[test]
    fn resampler_preserves_dc_after_warmup() {
        // A constant signal must stay constant through resampling once the FFT
        // overlap buffers have primed (skip the leading transient).
        let mut r = MonoResampler::new(44_100, 48_000).unwrap();
        let input = vec![10_000i16; r.chunk_in * 8];
        let out = r.process(&input);
        assert!(!out.is_empty());
        let tail = &out[out.len() / 2..];
        for &s in tail {
            assert!((s as i32 - 10_000).abs() < 500, "sample {s} drifted from DC");
        }
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
