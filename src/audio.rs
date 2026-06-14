//! Voice capture/playback core. No UI dependencies: the network worker drives it
//! and reads back an `AudioState` snapshot, so a future Tauri front-end reuses it
//! unchanged.

use crate::proto::AUDIO_PREFIX;
use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

/// Opus-supported sample rate we run at; both CoreAudio and PipeWire provide it
/// on request, so we never resample.
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
}

impl AudioHandle {
    /// Decode a received Opus payload and enqueue it for playback.
    pub fn play(&self, payload: &[u8]) {
        let mut dec = self.decoder.lock().unwrap();
        let mut out = [0i16; FRAME_SAMPLES];
        if let Ok(n) = dec.decode(payload, &mut out, false) {
            let mut jb = self.jitter.lock().unwrap();
            jb.extend(out[..n].iter().copied());
            while jb.len() > JITTER_MAX {
                jb.pop_front();
            }
        }
    }

    pub fn toggle_mute(&self) -> AudioState {
        let m = !self.controls.muted.load(Ordering::Relaxed);
        self.controls.muted.store(m, Ordering::Relaxed);
        self.controls.snapshot()
    }

    pub fn adjust_input_volume(&self, delta: i8) -> AudioState {
        let cur = self.controls.input_vol.load(Ordering::Relaxed) as i32;
        self.controls
            .input_vol
            .store(clamp_vol(cur + delta as i32), Ordering::Relaxed);
        self.controls.snapshot()
    }

    pub fn adjust_output_volume(&self, delta: i8) -> AudioState {
        let cur = self.controls.output_vol.load(Ordering::Relaxed) as i32;
        self.controls
            .output_vol
            .store(clamp_vol(cur + delta as i32), Ordering::Relaxed);
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

fn encoder_loop(
    sock: UdpSocket,
    peer: SocketAddr,
    controls: Arc<Controls>,
    pcm_rx: Receiver<Vec<i16>>,
) {
    let mut enc = match opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)
    {
        Ok(e) => e,
        Err(e) => {
            log::warn!("audio: encoder init failed: {e}");
            return;
        }
    };
    let mut buf: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES * 4);
    let mut out = [0u8; 4000];
    let mut pkt = Vec::with_capacity(AUDIO_PREFIX.len() + 400);

    while let Ok(chunk) = pcm_rx.recv() {
        buf.extend_from_slice(&chunk);
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

    // Capture → encoder thread → socket.
    let in_cfg = in_dev.default_input_config()?;
    let in_channels = in_cfg.channels().max(1) as usize;
    let in_sc = StreamConfig {
        channels: in_cfg.channels(),
        sample_rate: SAMPLE_RATE,
        buffer_size: BufferSize::Default,
    };
    let (pcm_tx, pcm_rx) = channel::<Vec<i16>>();
    let input = build_input(&in_dev, in_sc, in_channels, in_cfg.sample_format(), pcm_tx)?;
    {
        let controls = controls.clone();
        thread::spawn(move || encoder_loop(sock, peer, controls, pcm_rx));
    }

    // Playback ← jitter buffer ← decoder (via AudioHandle::play).
    let out_cfg = out_dev.default_output_config()?;
    let out_channels = out_cfg.channels().max(1) as usize;
    let out_sc = StreamConfig {
        channels: out_cfg.channels(),
        sample_rate: SAMPLE_RATE,
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
        "audio: started — in {}ch/{:?}, out {}ch/{:?}, peer {peer}",
        in_channels,
        in_cfg.sample_format(),
        out_channels,
        out_cfg.sample_format()
    );

    let decoder = Arc::new(Mutex::new(opus::Decoder::new(
        SAMPLE_RATE,
        opus::Channels::Mono,
    )?));
    Ok((
        AudioStreams {
            _input: input,
            _output: output,
        },
        AudioHandle {
            controls,
            jitter,
            decoder,
        },
    ))
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
