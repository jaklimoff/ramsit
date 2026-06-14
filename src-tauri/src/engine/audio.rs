//! Voice capture/playback core. No UI dependencies: the network worker drives it
//! and reads back an `AudioState` snapshot, so a future Tauri front-end reuses it
//! unchanged.

use crate::proto::AUDIO_PREFIX;
use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use rubato::{FftFixedIn, Resampler};
use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

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
/// Hard cap on the jitter buffer (≈500 ms); older samples are dropped past this.
const JITTER_MAX: usize = FRAME_SAMPLES * 25;

/// Snapshot of audio control state, sent to the UI after every change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioState {
    pub muted: bool,
    pub input_vol: u8,  // percent, 0..=200
    pub output_vol: u8, // percent, 0..=200
}

/// Atomics the realtime callbacks and encoder thread read each tick. Owned behind
/// an `Arc` shared by the streams, the encoder thread, and the `AudioHandle`.
struct Controls {
    muted: AtomicBool,
    input_vol: AtomicU32,  // percent
    output_vol: AtomicU32, // percent
}

impl Controls {
    fn snapshot(&self) -> AudioState {
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
    _input: cpal::Stream,
    _output: cpal::Stream,
}

/// Send + Sync control/data handle the network session uses: feed received Opus
/// in via `play`, adjust controls, read state. Holds no cpal stream.
#[derive(Clone)]
pub struct AudioHandle {
    controls: Arc<Controls>,
    jitter: Arc<Mutex<VecDeque<i16>>>,
    decoder: Arc<Mutex<opus::Decoder>>,
    /// Resamples decoded 48 kHz audio to the output device rate; `None` when the
    /// device already runs at `SAMPLE_RATE`.
    out_resampler: Option<Arc<Mutex<MonoResampler>>>,
}

impl AudioHandle {
    /// Decode a received Opus payload and enqueue it for playback.
    pub fn play(&self, payload: &[u8]) {
        let mut dec = self.decoder.lock().unwrap();
        let mut out = [0i16; FRAME_SAMPLES];
        match dec.decode(payload, &mut out, false) {
            Ok(n) => {
                let samples = match &self.out_resampler {
                    Some(r) => r.lock().unwrap().process(&out[..n]),
                    None => out[..n].to_vec(),
                };
                let mut jb = self.jitter.lock().unwrap();
                jb.extend(samples);
                while jb.len() > JITTER_MAX {
                    jb.pop_front();
                }
            }
            Err(e) => log::warn!("audio: decode failed: {e}"),
        }
    }

    pub fn toggle_mute(&self) -> AudioState {
        let m = !self.controls.muted.load(Ordering::Relaxed);
        self.controls.muted.store(m, Ordering::Relaxed);
        self.controls.snapshot()
    }

    pub fn set_input_volume(&self, pct: u8) -> AudioState {
        self.controls
            .input_vol
            .store(clamp_vol(pct as i32), Ordering::Relaxed);
        self.controls.snapshot()
    }

    pub fn set_output_volume(&self, pct: u8) -> AudioState {
        self.controls
            .output_vol
            .store(clamp_vol(pct as i32), Ordering::Relaxed);
        self.controls.snapshot()
    }

    pub fn state(&self) -> AudioState {
        self.controls.snapshot()
    }
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
fn prefers_48k<I: Iterator<Item = cpal::SupportedStreamConfigRange>>(supported: Option<I>) -> bool {
    supported
        .map(|mut c| {
            c.any(|r| r.min_sample_rate() <= SAMPLE_RATE && r.max_sample_rate() >= SAMPLE_RATE)
        })
        .unwrap_or(false)
}

fn encoder_loop(
    sock: UdpSocket,
    peer: SocketAddr,
    controls: Arc<Controls>,
    pcm_rx: Receiver<Vec<i16>>,
    in_rate: u32,
) {
    let mut enc =
        match opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("audio: encoder init failed: {e}");
                return;
            }
        };
    let mut resampler = if in_rate != SAMPLE_RATE {
        match MonoResampler::new(in_rate, SAMPLE_RATE) {
            Ok(r) => Some(r),
            Err(e) => {
                log::warn!("audio: input resampler init failed: {e}");
                return;
            }
        }
    } else {
        None
    };
    let mut buf: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES * 4);
    let mut out = [0u8; 4000];
    let mut pkt = Vec::with_capacity(AUDIO_PREFIX.len() + 400);

    while let Ok(chunk) = pcm_rx.recv() {
        match resampler.as_mut() {
            Some(r) => buf.extend_from_slice(&r.process(&chunk)),
            None => buf.extend_from_slice(&chunk),
        }
        while buf.len() >= FRAME_SAMPLES {
            let mut frame: Vec<i16> = buf.drain(..FRAME_SAMPLES).collect();
            if controls.muted.load(Ordering::Relaxed) {
                continue; // frame already drained; nothing sent
            }
            apply_gain(&mut frame, controls.input_vol.load(Ordering::Relaxed));
            match enc.encode(&frame, &mut out) {
                Ok(n) => {
                    pkt.clear();
                    pkt.extend_from_slice(AUDIO_PREFIX);
                    pkt.extend_from_slice(&out[..n]);
                    let _ = sock.send_to(&pkt, peer);
                }
                Err(e) => log::warn!("audio: encode failed: {e}"),
            }
        }
    }
}

fn build_input(
    dev: &cpal::Device,
    cfg: StreamConfig,
    channels: usize,
    fmt: SampleFormat,
    pcm_tx: Sender<Vec<i16>>,
) -> Result<cpal::Stream> {
    match fmt {
        SampleFormat::F32 => input_stream::<f32>(dev, cfg, channels, pcm_tx),
        SampleFormat::I16 => input_stream::<i16>(dev, cfg, channels, pcm_tx),
        other => Err(anyhow!("unsupported input sample format {other:?}")),
    }
}

fn input_stream<T>(
    dev: &cpal::Device,
    cfg: StreamConfig,
    channels: usize,
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
            let _ = pcm_tx.send(downmix(&pcm, channels));
        },
        |e| log::warn!("audio: input stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

fn build_output(
    dev: &cpal::Device,
    cfg: StreamConfig,
    channels: usize,
    fmt: SampleFormat,
    jitter: Arc<Mutex<VecDeque<i16>>>,
    controls: Arc<Controls>,
) -> Result<cpal::Stream> {
    match fmt {
        SampleFormat::F32 => output_stream::<f32>(dev, cfg, channels, jitter, controls),
        SampleFormat::I16 => output_stream::<i16>(dev, cfg, channels, jitter, controls),
        other => Err(anyhow!("unsupported output sample format {other:?}")),
    }
}

fn output_stream<T>(
    dev: &cpal::Device,
    cfg: StreamConfig,
    channels: usize,
    jitter: Arc<Mutex<VecDeque<i16>>>,
    controls: Arc<Controls>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<i16>,
{
    let mut playing = false; // owned by this FnMut; latches once primed
    let stream = dev.build_output_stream(
        cfg,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let gain = controls.output_vol.load(Ordering::Relaxed);
            let mut jb = jitter.lock().unwrap();
            if !playing && jb.len() >= JITTER_PRIME {
                playing = true;
            }
            for frame in data.chunks_mut(channels) {
                let sample = if playing {
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
                let out = T::from_sample(sample);
                for o in frame.iter_mut() {
                    *o = out;
                }
            }
        },
        |e| log::warn!("audio: output stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

/// Start capture+playback on the system default devices. `sock` is a clone of the
/// connected UDP socket (used only to send voice to `peer`). Returns the stream
/// guard (keep alive for the call) and a Send handle for the network session.
pub fn start(sock: UdpSocket, peer: SocketAddr) -> Result<(AudioStreams, AudioHandle)> {
    let host = cpal::default_host();
    let in_dev = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no default input device"))?;
    let out_dev = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no default output device"))?;

    let controls = Arc::new(Controls {
        muted: AtomicBool::new(false),
        input_vol: AtomicU32::new(100),
        output_vol: AtomicU32::new(100),
    });
    let jitter = Arc::new(Mutex::new(VecDeque::<i16>::new()));

    // Capture → encoder thread → socket. Prefer 48 kHz (no resampling); otherwise
    // fall back to the device's native rate and resample to SAMPLE_RATE.
    let in_cfg = in_dev.default_input_config()?;
    let in_rate = if prefers_48k(in_dev.supported_input_configs().ok()) {
        SAMPLE_RATE
    } else {
        in_cfg.sample_rate()
    };
    let in_channels = in_cfg.channels().max(1) as usize;
    let in_sc = StreamConfig {
        channels: in_cfg.channels(),
        sample_rate: in_rate,
        buffer_size: BufferSize::Default,
    };
    let (pcm_tx, pcm_rx) = channel::<Vec<i16>>();
    let input = build_input(&in_dev, in_sc, in_channels, in_cfg.sample_format(), pcm_tx)?;
    {
        let controls = controls.clone();
        thread::spawn(move || encoder_loop(sock, peer, controls, pcm_rx, in_rate));
    }

    // Playback ← jitter buffer ← decoder (via AudioHandle::play).
    let out_cfg = out_dev.default_output_config()?;
    let out_rate = if prefers_48k(out_dev.supported_output_configs().ok()) {
        SAMPLE_RATE
    } else {
        out_cfg.sample_rate()
    };
    let out_channels = out_cfg.channels().max(1) as usize;
    let out_sc = StreamConfig {
        channels: out_cfg.channels(),
        sample_rate: out_rate,
        buffer_size: BufferSize::Default,
    };
    let output = build_output(
        &out_dev,
        out_sc,
        out_channels,
        out_cfg.sample_format(),
        jitter.clone(),
        controls.clone(),
    )?;

    input.play()?;
    output.play()?;
    log::info!(
        "audio: started — in {}ch/{:?}@{in_rate}Hz, out {}ch/{:?}@{out_rate}Hz, peer {peer}",
        in_channels,
        in_cfg.sample_format(),
        out_channels,
        out_cfg.sample_format()
    );

    let decoder = Arc::new(Mutex::new(opus::Decoder::new(
        SAMPLE_RATE,
        opus::Channels::Mono,
    )?));
    let out_resampler = if out_rate != SAMPLE_RATE {
        Some(Arc::new(Mutex::new(MonoResampler::new(SAMPLE_RATE, out_rate)?)))
    } else {
        None
    };
    Ok((
        AudioStreams {
            _input: input,
            _output: output,
        },
        AudioHandle {
            controls,
            jitter,
            decoder,
            out_resampler,
        },
    ))
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
