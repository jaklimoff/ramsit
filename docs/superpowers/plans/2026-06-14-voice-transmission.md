# Voice Transmission Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add real-time two-way Opus voice over the existing P2P UDP link, live on connect, with mute/unmute and adjustable mic/speaker volume, on Linux and macOS using the system default devices.

**Architecture:** A new device-and-UI-agnostic `audio.rs` core captures from the default mic via `cpal`, Opus-encodes 20 ms frames, and sends them over a cloned UDP socket; received frames decode into a jitter buffer the output callback drains. The engine splits into a `!Send` stream guard (held on the network-worker thread) and a `Send` `AudioHandle` (passed to `session`). The UI layer (`app.rs`/`ui.rs`) only sends intent `Command`s and renders the authoritative `AudioState` the core emits — so the planned Tauri migration replaces just those two files.

**Tech Stack:** Rust, `cpal = "0.18"` (ALSA on Linux, CoreAudio on macOS), `opus = "0.3"` (system libopus via pkg-config), existing `ratatui`/`anyhow`/`log`.

**Cross-platform note:** `opus` links system libopus. Build prerequisite — macOS: `brew install opus pkg-config`; Debian/Ubuntu: `sudo apt install libopus-dev pkg-config`; Fedora: `sudo dnf install opus-devel pkgconf-pkg-config`. Documented in README in Task 7.

**Verified facts (cpal 0.18, opus 0.3.1, rustc 1.96):**
- `cpal::SampleRate` is a `u32` alias; `StreamConfig { channels: u16, sample_rate: u32, buffer_size: BufferSize::Default }`.
- `build_input_stream`/`build_output_stream` take `StreamConfig` **by value**, callback, error-callback, and `None: Option<Duration>`.
- Sample conversion via `i16::from_sample(t)` / `T::from_sample(i16)` (traits `cpal::{Sample, FromSample, SizedSample}`).
- `opus::Encoder`/`Decoder` are `Send`; `cpal::Stream` is `!Send`.
- `opus::Encoder::new(48000, Channels::Mono, Application::Voip)`, `enc.encode(&[i16], &mut [u8]) -> Result<usize>`; `Decoder::new(48000, Channels::Mono)`, `dec.decode(&[u8], &mut [i16], false) -> Result<usize>` (returns samples). A 960-sample silent frame encodes to ~57 bytes.

---

## Files

| File | Responsibility |
|---|---|
| `Cargo.toml` | add `cpal`, `opus` deps (Task 2) |
| `src/proto.rs` | `AUDIO_PREFIX`, `PacketKind::Audio`, `classify` prefix check |
| `src/punch.rs` | add `Audio` arm to its `PacketKind` match (ignore mid-punch) |
| `src/audio.rs` | **new** — pure helpers + types (Task 2), then cpal/opus engine (Task 3) |
| `src/net.rs` | new `Command`/`Event` variants, start audio on connect, route Audio packets, `Option<AudioHandle>` in `session`, Instant-based keepalive |
| `src/app.rs` | `Screen::Chat` audio mirror fields, apply audio events, audio keybindings |
| `src/ui.rs` | render audio status segment + key hint |
| `src/main.rs` | `mod audio;` |
| `README.md` | voice usage, controls, libopus prerequisite |

---

## Task 1: Audio packet kind in the protocol

**Files:**
- Modify: `src/proto.rs`
- Modify: `src/punch.rs` (match exhaustiveness)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/proto.rs`:

```rust
    #[test]
    fn classifies_audio_prefix() {
        // Opus payloads can contain sentinel/null bytes; the prefix still wins.
        let mut pkt = AUDIO_PREFIX.to_vec();
        pkt.extend_from_slice(&[0x00, 0x12, 0xff, 0x00]);
        assert_eq!(classify(&pkt), PacketKind::Audio);
    }

    #[test]
    fn audio_prefix_does_not_disturb_chat_or_control() {
        assert_eq!(classify(b"hello bro"), PacketKind::Chat);
        assert_eq!(classify(PUNCH), PacketKind::Punch);
        assert_eq!(classify(BYE), PacketKind::Bye);
        // Bare sentinel that is not the audio prefix and not a known control.
        assert_eq!(classify(b"\x00AU"), PacketKind::Chat);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib proto:: 2>&1 | tail -20`
Expected: FAIL — `AUDIO_PREFIX` not found / `PacketKind::Audio` not found.

- [ ] **Step 3: Implement in `src/proto.rs`**

Add the constant next to the other control byte-strings (after the `BYE` line):

```rust
/// Prefix marking a voice frame: `\x00AUD` + Opus payload. Distinguishes binary
/// Opus data (which may contain `0x00`) from raw-UTF-8 chat.
pub const AUDIO_PREFIX: &[u8] = b"\x00AUD";
```

Add the variant to the enum (before `Chat`):

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum PacketKind {
    Punch,
    PunchAck,
    Keepalive,
    Bye,
    Audio,
    Chat,
}
```

Add the prefix check in `classify`, immediately after the leading-sentinel short-circuit and before the exact-match `match`:

```rust
pub fn classify(buf: &[u8]) -> PacketKind {
    // Chat text never starts with the sentinel, so short-circuit it.
    if buf.first() != Some(&SENTINEL) {
        return PacketKind::Chat;
    }
    if buf.starts_with(AUDIO_PREFIX) {
        return PacketKind::Audio;
    }
    match buf {
        PUNCH => PacketKind::Punch,
        PUNCH_ACK => PacketKind::PunchAck,
        KEEPALIVE => PacketKind::Keepalive,
        BYE => PacketKind::Bye,
        _ => PacketKind::Chat,
    }
}
```

- [ ] **Step 4: Fix the now-non-exhaustive match in `src/punch.rs`**

In `punch()`, the `match kind` arm `PacketKind::Keepalive | PacketKind::Bye => {}` must also ignore `Audio` (voice frames can't arrive before connection, but the match must stay exhaustive):

```rust
                    PacketKind::Keepalive | PacketKind::Bye | PacketKind::Audio => {}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib proto:: 2>&1 | tail -20`
Expected: PASS (all proto tests, including the two new ones).

- [ ] **Step 6: Build the whole crate to confirm punch.rs is exhaustive**

Run: `cargo build 2>&1 | tail -10`
Expected: compiles with no errors.

- [ ] **Step 7: Commit**

```bash
git add src/proto.rs src/punch.rs
git commit -m "feat(proto): add AUDIO packet kind and prefix classification"
```

---

## Task 2: Audio module — pure helpers, types, and constants

This task adds the dependencies and the device-free, fully unit-testable core of `audio.rs`. The cpal/opus engine itself comes in Task 3.

**Files:**
- Modify: `Cargo.toml`
- Create: `src/audio.rs`
- Modify: `src/main.rs` (add `mod audio;`)

- [ ] **Step 1: Add dependencies**

Run:
```bash
cargo add cpal@0.18 opus@0.3
```
Expected: `Cargo.toml` `[dependencies]` now includes `cpal = "0.18"` and `opus = "0.3"`.

- [ ] **Step 2: Register the module in `src/main.rs`**

Add `mod audio;` to the module list at the top (keep alphabetical with the others):

```rust
mod app;
mod audio;
mod net;
mod proto;
mod punch;
mod ui;
```

- [ ] **Step 3: Write `src/audio.rs` with helpers, types, and failing tests**

Create `src/audio.rs` with exactly this content (the engine is added in Task 3):

```rust
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
        // decode output buffer sizing in Task 3 (FRAME_SAMPLES) is correct.
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib audio:: 2>&1 | tail -20`
Expected: PASS — 5 tests. (If linking fails with "cannot find -lopus", install libopus per the prerequisite note above, then re-run.)

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs src/audio.rs
git commit -m "feat(audio): add cpal/opus deps and pure audio helpers"
```

---

## Task 3: Audio engine — cpal streams, encoder thread, handle

Adds the device-dependent engine to `src/audio.rs`. The realtime/device code is integration-only (no audio hardware in CI), so there are no new unit tests here; correctness of the pure logic it calls is already covered by Task 2. The acceptance check is a clean `cargo build` and `cargo clippy`.

**Files:**
- Modify: `src/audio.rs`

- [ ] **Step 1: Add imports at the top of `src/audio.rs`**

Insert below the module doc comment (above the `pub const SAMPLE_RATE` line):

```rust
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
```

- [ ] **Step 2: Add jitter-buffer constants next to the existing constants**

After the `pub const VOL_MAX: i32 = 200;` line:

```rust
/// Decoded samples buffered before playback starts (≈40 ms) to absorb jitter.
const JITTER_PRIME: usize = FRAME_SAMPLES * 2;
/// Hard cap on the jitter buffer (≈500 ms); older samples are dropped past this.
const JITTER_MAX: usize = FRAME_SAMPLES * 25;
```

- [ ] **Step 3: Add the shared control state**

After the `AudioState` struct:

```rust
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
```

- [ ] **Step 4: Add the stream guard and the Send handle**

After the `Controls` impl. `AudioStreams` is `!Send` (holds `cpal::Stream`) and lives on the worker thread; `AudioHandle` is `Send + Sync` and is what `session` holds.

```rust
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
```

- [ ] **Step 5: Add the encoder thread**

The mic callback sends mono i16 chunks over a channel; this thread accumulates 960-sample frames, applies gain, encodes, and sends `\x00AUD`+payload to the peer. Muted ⇒ drain the frame but skip encode/send (silence suppression). Append to `src/audio.rs`:

```rust
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
```

- [ ] **Step 6: Add the format-agnostic stream builders**

Append to `src/audio.rs`. The mic callback converts device samples to i16, downmixes to mono, and forwards. The output callback primes the jitter buffer, then drains it with output gain, upmixing to the device channel count; underrun re-primes.

```rust
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
```

- [ ] **Step 7: Add `start()` that wires everything together**

Append to `src/audio.rs`. Returns the `!Send` stream guard and the `Send` handle separately.

```rust
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
```

- [ ] **Step 8: Verify it compiles and lints clean**

Run: `cargo build 2>&1 | tail -15`
Expected: compiles, no errors.

Run: `cargo clippy --all-targets 2>&1 | tail -15`
Expected: no warnings in `audio.rs`.

- [ ] **Step 9: Confirm the pure tests still pass**

Run: `cargo test --lib audio:: 2>&1 | tail -10`
Expected: PASS — 5 tests.

- [ ] **Step 10: Commit**

```bash
git add src/audio.rs
git commit -m "feat(audio): cpal capture/playback engine with Opus and jitter buffer"
```

---

## Task 4: Wire voice into the network worker

**Files:**
- Modify: `src/net.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/net.rs`. It drives `session` with `None` audio and sends an AUDIO packet to it from a separate socket. Because the session IP filter only checks `from.ip() == peer.ip()` (both `127.0.0.1`), the packet passes the filter and exercises the `Audio` arm — which must drop it, never emitting `Incoming`:

```rust
    #[test]
    fn session_does_not_surface_audio_as_chat() {
        use crate::proto::AUDIO_PREFIX;
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b_addr = b.local_addr().unwrap();
        let peer = "127.0.0.1:9".parse().unwrap(); // session B's notion of its peer

        let (_b_cmd_tx, b_cmd_rx) = channel();
        let (b_evt_tx, b_evt_rx) = channel();
        let hb = thread::spawn(move || session(b, peer, b_cmd_rx, b_evt_tx, None));

        let spoof = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut pkt = AUDIO_PREFIX.to_vec();
        pkt.extend_from_slice(&[0x10, 0x20, 0x30]);
        spoof.send_to(&pkt, b_addr).unwrap();

        match b_evt_rx.recv_timeout(Duration::from_millis(400)) {
            Err(_) => {}                                          // good: dropped
            Ok(Event::Incoming(s)) => panic!("audio leaked to chat: {s:?}"),
            Ok(other) => panic!("unexpected event: {other:?}"),
        }
        drop(hb); // detached; process ends it
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib net::tests::session_does_not_surface_audio_as_chat 2>&1 | tail -20`
Expected: FAIL to compile — `session` takes 4 args, not 5.

- [ ] **Step 3: Add the new Command and Event variants**

In `src/net.rs`, extend the enums:

```rust
#[derive(Debug)]
pub enum Command {
    PeerCode(SocketAddr),
    Send(String),
    ToggleMute,
    AdjustInputVolume(i8),
    AdjustOutputVolume(i8),
    Quit,
}

#[derive(Debug)]
pub enum Event {
    Discovered(SocketAddr),
    Connected(SocketAddr),
    Incoming(String),
    AudioState(crate::audio::AudioState),
    AudioUnavailable(String),
    PeerLeft,
    Fatal(String),
}
```

- [ ] **Step 4: Update imports and start audio after connect in `worker`**

At the top of `src/net.rs`, extend the proto import to include the audio prefix and bring in the audio module:

```rust
use crate::audio::{self, AudioHandle};
use crate::proto::{classify, would_block, PacketKind, AUDIO_PREFIX, BYE, KEEPALIVE, MAX_CHAT_BYTES, RECV_BUF};
```

In `worker`, replace the tail (the `let _ = events.send(Event::Connected(peer));` block through the `session(...)` call) with:

```rust
    info!("connected to {peer}");
    let _ = events.send(Event::Connected(peer));
    for m in early {
        let _ = events.send(Event::Incoming(m));
    }

    // Start voice on the system default devices. Failure is non-fatal: chat
    // continues. The stream guard must outlive the session, so hold it here.
    let (_streams, audio) = match sock.try_clone() {
        Ok(clone) => match audio::start(clone, peer) {
            Ok((streams, handle)) => {
                let _ = events.send(Event::AudioState(handle.state()));
                (Some(streams), Some(handle))
            }
            Err(e) => {
                warn!("audio: unavailable: {e}");
                let _ = events.send(Event::AudioUnavailable(e.to_string()));
                (None, None)
            }
        },
        Err(e) => {
            let _ = events.send(Event::AudioUnavailable(format!("socket clone failed: {e}")));
            (None, None)
        }
    };

    session(sock, peer, cmds, events, audio);
```

- [ ] **Step 5: Change the `session` signature and handle audio commands + packets + keepalive timing**

Replace the whole `session` function in `src/net.rs` with:

```rust
/// The post-connect bridge loop: forward outgoing `Send`s onto the socket,
/// surface incoming chat/BYE as events, route voice frames to the audio engine,
/// apply audio control commands, and refresh the NAT mapping on a timer.
/// Factored out so a loopback test can drive it without STUN/punch (pass `None`).
pub fn session(
    sock: UdpSocket,
    peer: SocketAddr,
    cmds: Receiver<Command>,
    events: Sender<Event>,
    audio: Option<AudioHandle>,
) {
    let _ = sock.set_read_timeout(Some(POLL));
    let mut buf = [0u8; RECV_BUF];
    let mut last_keepalive = std::time::Instant::now();

    loop {
        // Drain outgoing commands.
        loop {
            match cmds.try_recv() {
                Ok(Command::Send(line)) => {
                    let b = encode_chat(&line);
                    debug!("session: -> {peer} chat ({} bytes)", b.len());
                    let _ = sock.send_to(&b, peer);
                }
                Ok(Command::ToggleMute) => {
                    if let Some(a) = &audio {
                        let _ = events.send(Event::AudioState(a.toggle_mute()));
                    }
                }
                Ok(Command::AdjustInputVolume(d)) => {
                    if let Some(a) = &audio {
                        let _ = events.send(Event::AudioState(a.adjust_input_volume(d)));
                    }
                }
                Ok(Command::AdjustOutputVolume(d)) => {
                    if let Some(a) = &audio {
                        let _ = events.send(Event::AudioState(a.adjust_output_volume(d)));
                    }
                }
                Ok(Command::Quit) => {
                    debug!("session: -> {peer} BYE (local quit)");
                    let _ = sock.send_to(BYE, peer);
                    return;
                }
                Ok(Command::PeerCode(_)) => {} // already connected; ignore
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = sock.send_to(BYE, peer);
                    return;
                }
            }
        }

        // One inbound read (peer IP filter, as in the old chat loop).
        match sock.recv_from(&mut buf) {
            Ok((n, from)) if from.ip() == peer.ip() => {
                let kind = classify(&buf[..n]);
                if kind != PacketKind::Audio {
                    debug!("session: <- {from} {kind:?} ({n} bytes)");
                }
                match kind {
                    PacketKind::Chat => {
                        if let Ok(s) = std::str::from_utf8(&buf[..n]) {
                            if events.send(Event::Incoming(s.to_string())).is_err() {
                                return; // UI gone
                            }
                        }
                    }
                    PacketKind::Audio => {
                        if let Some(a) = &audio {
                            a.play(&buf[AUDIO_PREFIX.len()..n]);
                        }
                    }
                    PacketKind::Bye => {
                        let _ = events.send(Event::PeerLeft);
                        // Keep running so the user can read history and quit cleanly.
                    }
                    _ => {}
                }
            }
            Ok((_, from)) => debug!("session: ignoring packet from unrelated {from}"),
            Err(e) if would_block(&e) => {}
            Err(_) => return,
        }

        if last_keepalive.elapsed() >= KEEPALIVE_INTERVAL {
            let _ = sock.send_to(KEEPALIVE, peer);
            last_keepalive = std::time::Instant::now();
        }
    }
}
```

- [ ] **Step 6: Replace the tick constant with a duration**

In `src/net.rs`, replace the line `const KEEPALIVE_TICKS: u32 = 75; // 75 * 200ms = 15s` with:

```rust
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
```

- [ ] **Step 7: Update the existing loopback test call**

In `session_delivers_message_over_loopback`, both `session(...)` calls now need the 5th argument `None`:

```rust
        let ha = thread::spawn(move || session(a, b_addr, a_cmd_rx, a_evt_tx, None));
        let hb = thread::spawn(move || session(b, a_addr, b_cmd_rx, b_evt_tx, None));
```

- [ ] **Step 8: Run the net tests to verify they pass**

Run: `cargo test --lib net:: 2>&1 | tail -20`
Expected: PASS — including `session_delivers_message_over_loopback` and `session_does_not_surface_audio_as_chat`.

- [ ] **Step 9: Build to confirm `worker`/`main` still compile**

Run: `cargo build 2>&1 | tail -10`
Expected: compiles.

- [ ] **Step 10: Commit**

```bash
git add src/net.rs
git commit -m "feat(net): start voice on connect, route audio packets and controls"
```

---

## Task 5: App state — mirror audio state, audio keybindings

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/app.rs`:

```rust
    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }
    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    #[test]
    fn audio_state_event_updates_mirror() {
        let mut app = chat_app();
        app.apply(Event::AudioState(crate::audio::AudioState {
            muted: true,
            input_vol: 80,
            output_vol: 120,
        }));
        if let Screen::Chat {
            muted,
            input_vol,
            output_vol,
            voice,
            ..
        } = &app.screen
        {
            assert!(*muted && *voice);
            assert_eq!((*input_vol, *output_vol), (80, 120));
        } else {
            panic!("expected Chat");
        }
    }

    #[test]
    fn audio_unavailable_clears_voice_and_notes_it() {
        let mut app = chat_app();
        app.apply(Event::AudioUnavailable("no mic".into()));
        if let Screen::Chat {
            voice, messages, ..
        } = &app.screen
        {
            assert!(!*voice);
            assert!(messages.iter().any(|m| m.contains("voice unavailable")));
        } else {
            panic!("expected Chat");
        }
    }

    #[test]
    fn ctrl_t_toggles_mute_without_typing() {
        let mut app = chat_app();
        let cmd = app.on_key(ctrl(KeyCode::Char('t')));
        assert!(matches!(cmd, Some(Command::ToggleMute)));
        if let Screen::Chat { input, .. } = &app.screen {
            assert!(input.is_empty(), "Ctrl-T must not insert 't'");
        }
    }

    #[test]
    fn volume_keys_emit_adjust_commands() {
        let mut app = chat_app();
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Up)),
            Some(Command::AdjustInputVolume(10))
        ));
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Down)),
            Some(Command::AdjustInputVolume(-10))
        ));
        assert!(matches!(
            app.on_key(alt(KeyCode::Up)),
            Some(Command::AdjustOutputVolume(10))
        ));
        assert!(matches!(
            app.on_key(alt(KeyCode::Down)),
            Some(Command::AdjustOutputVolume(-10))
        ));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib app:: 2>&1 | tail -20`
Expected: FAIL to compile — `Screen::Chat` has no `muted`/`voice` fields; `Event::AudioState` arms missing.

- [ ] **Step 3: Add audio mirror fields to `Screen::Chat`**

In the `Screen` enum, extend the `Chat` variant:

```rust
    Chat {
        peer: std::net::SocketAddr,
        messages: Vec<String>,
        input: String,
        scroll: usize, // lines from the bottom; 0 = pinned
        connected: bool,
        muted: bool,
        input_vol: u8,
        output_vol: u8,
        voice: bool, // whether the audio engine started
    },
```

- [ ] **Step 4: Initialize the fields when entering Chat**

In `apply`, the `Event::Connected(peer)` arm sets the new fields (mic starts live; `voice` stays false until `AudioState` arrives):

```rust
            Event::Connected(peer) => {
                self.screen = Screen::Chat {
                    peer,
                    messages: Vec::new(),
                    input: String::new(),
                    scroll: 0,
                    connected: true,
                    muted: false,
                    input_vol: 100,
                    output_vol: 100,
                    voice: false,
                };
            }
```

- [ ] **Step 5: Handle the new audio events in `apply`**

Add these two arms to the `match ev` in `apply` (e.g. after the `Event::Incoming` arm):

```rust
            Event::AudioState(st) => {
                if let Screen::Chat {
                    muted,
                    input_vol,
                    output_vol,
                    voice,
                    ..
                } = &mut self.screen
                {
                    *muted = st.muted;
                    *input_vol = st.input_vol;
                    *output_vol = st.output_vol;
                    *voice = true;
                }
            }
            Event::AudioUnavailable(msg) => {
                if let Screen::Chat {
                    messages, voice, ..
                } = &mut self.screen
                {
                    *voice = false;
                    messages.push(format!("* voice unavailable: {msg} *"));
                }
            }
```

- [ ] **Step 6: Add the audio keybindings in `on_key`**

In the `Screen::Chat { .. }` arm of `on_key`, replace the existing `match key.code { ... }` with one that checks modified keys first. Note the `muted`/`voice` fields are not needed here, so keep destructuring `messages, input, scroll`:

```rust
            Screen::Chat {
                messages,
                input,
                scroll,
                ..
            } => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let alt = key.modifiers.contains(KeyModifiers::ALT);
                match key.code {
                    KeyCode::Char('t') if ctrl => cmd = Some(Command::ToggleMute),
                    KeyCode::Up if ctrl => cmd = Some(Command::AdjustInputVolume(10)),
                    KeyCode::Down if ctrl => cmd = Some(Command::AdjustInputVolume(-10)),
                    KeyCode::Up if alt => cmd = Some(Command::AdjustOutputVolume(10)),
                    KeyCode::Down if alt => cmd = Some(Command::AdjustOutputVolume(-10)),
                    KeyCode::Char(c) => input.push(c),
                    KeyCode::Backspace => {
                        input.pop();
                    }
                    KeyCode::Enter => {
                        if !input.is_empty() {
                            let line = std::mem::take(input);
                            messages.push(format!("you> {line}"));
                            *scroll = 0;
                            cmd = Some(Command::Send(line));
                        }
                    }
                    KeyCode::PageUp => {
                        let max = messages.len().saturating_sub(1);
                        *scroll = (*scroll + PAGE).min(max);
                    }
                    KeyCode::PageDown => {
                        *scroll = scroll.saturating_sub(PAGE);
                    }
                    _ => {}
                }
            }
```

- [ ] **Step 7: Update the `chat_app` test helper**

In the `tests` module, update `chat_app()` to construct the new fields:

```rust
    fn chat_app() -> App {
        App {
            screen: Screen::Chat {
                peer: addr(),
                messages: Vec::new(),
                input: String::new(),
                scroll: 0,
                connected: true,
                muted: false,
                input_vol: 100,
                output_vol: 100,
                voice: true,
            },
            should_quit: false,
        }
    }
```

- [ ] **Step 8: Run app tests to verify they pass**

Run: `cargo test --lib app:: 2>&1 | tail -20`
Expected: PASS — existing tests plus the four new audio tests.

- [ ] **Step 9: Commit**

```bash
git add src/app.rs
git commit -m "feat(app): mirror audio state and add mute/volume keybindings"
```

---

## Task 6: Render the audio status

**Files:**
- Modify: `src/ui.rs`

- [ ] **Step 1: Update the failing test**

Replace the `chat_screen_shows_messages_and_status` test in `src/ui.rs` with a version that constructs the new `Screen::Chat` fields and asserts the audio segment renders:

```rust
    #[test]
    fn chat_screen_shows_messages_and_status() {
        use crate::app::Screen;
        let app = App {
            screen: Screen::Chat {
                peer: "203.0.113.5:54213".parse().unwrap(),
                messages: vec!["peer> yo".into(), "you> hey".into()],
                input: "typing".into(),
                scroll: 0,
                connected: true,
                muted: false,
                input_vol: 100,
                output_vol: 100,
                voice: true,
            },
            should_quit: false,
        };
        let s = render(&app, 80, 10);
        assert!(s.contains("peer> yo"));
        assert!(s.contains("you> hey"));
        assert!(s.contains("connected"));
        assert!(s.contains("> typing"));
        assert!(s.contains("LIVE"));
        assert!(s.contains("mic 100%"));
        assert!(s.contains("spk 100%"));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib ui:: 2>&1 | tail -20`
Expected: FAIL to compile — `Screen::Chat` literal missing the new fields (and, once compiling, missing "LIVE").

- [ ] **Step 3: Pass the new fields into `draw_chat` from `draw`**

In `draw`, update the `Screen::Chat` arm:

```rust
        Screen::Chat {
            peer,
            messages,
            input,
            scroll,
            connected,
            muted,
            input_vol,
            output_vol,
            voice,
        } => draw_chat(
            f,
            &peer.to_string(),
            messages,
            input,
            *scroll,
            *connected,
            *muted,
            *input_vol,
            *output_vol,
            *voice,
        ),
```

- [ ] **Step 4: Update `draw_chat` to render the audio segment and key hint**

Replace the `draw_chat` function in `src/ui.rs` with:

```rust
#[allow(clippy::too_many_arguments)]
fn draw_chat(
    f: &mut Frame,
    peer: &str,
    messages: &[String],
    input: &str,
    scroll: usize,
    connected: bool,
    muted: bool,
    input_vol: u8,
    output_vol: u8,
    voice: bool,
) {
    let areas = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(f.area());
    let (history_area, input_area) = (areas[0], areas[1]);

    // Status bar = title of the history block.
    let (label, color) = if connected {
        ("connected", Color::Green)
    } else {
        ("disconnected", Color::Red)
    };
    let (audio_seg, audio_color) = if !voice {
        (" [no voice] ".to_string(), Color::DarkGray)
    } else if muted {
        (
            format!(" [MUTED] mic {input_vol}% spk {output_vol}% "),
            Color::Yellow,
        )
    } else {
        (
            format!(" [LIVE] mic {input_vol}% spk {output_vol}% "),
            Color::Cyan,
        )
    };
    let title = Line::from(vec![
        Span::raw(format!(" ramsit — peer {peer} ")),
        Span::styled("●", Style::default().fg(color)),
        Span::raw(format!(" {label} ")),
        Span::styled(audio_seg, Style::default().fg(audio_color)),
    ]);

    let inner_h = history_area.height.saturating_sub(2) as usize; // minus borders
    let lines = visible_lines(messages, inner_h, scroll);
    let history = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    f.render_widget(history, history_area);

    let prompt = format!("> {input}");
    let input_p = Paragraph::new(prompt.as_str()).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" message · Ctrl-T mute · Ctrl-Up/Down mic · Alt-Up/Down spk "),
    );
    f.render_widget(input_p, input_area);
    place_cursor(f, input_area, input.chars().count());
}
```

- [ ] **Step 5: Run the ui tests to verify they pass**

Run: `cargo test --lib ui:: 2>&1 | tail -20`
Expected: PASS — both ui tests.

- [ ] **Step 6: Commit**

```bash
git add src/ui.rs
git commit -m "feat(ui): show mute state and mic/speaker volume in chat status"
```

---

## Task 7: Docs and full verification

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Document voice in `README.md`**

Add a "Voice" section. Locate the existing controls/usage area and insert:

```markdown
## Voice

Once two peers connect, the microphone goes live automatically and voice flows
both ways over the same P2P UDP link (Opus, 48 kHz mono). It uses your system
**default** input and output devices.

Controls (in the chat screen):

| Key | Action |
| --- | --- |
| `Ctrl-T` | Toggle mute (mic) |
| `Ctrl-Up` / `Ctrl-Down` | Mic volume +/- 10% |
| `Alt-Up` / `Alt-Down` | Speaker volume +/- 10% |

Volume ranges 0–200% (digital gain). The status bar shows `[LIVE]`/`[MUTED]` and
current mic/speaker levels. If no audio device is available the call still works
as text chat and shows `[no voice]`.

### Build prerequisite: libopus

Voice links the system Opus library:

- macOS: `brew install opus pkg-config`
- Debian/Ubuntu: `sudo apt install libopus-dev pkg-config`
- Fedora: `sudo dnf install opus-devel pkgconf-pkg-config`
```

- [ ] **Step 2: Run the full test suite**

Run: `cargo test 2>&1 | tail -25`
Expected: PASS — all tests across `proto`, `audio`, `net`, `app`, `ui`.

- [ ] **Step 3: Lint and format**

Run: `cargo clippy --all-targets 2>&1 | tail -20`
Expected: no warnings.

Run: `cargo fmt --all && git diff --stat`
Expected: formatting clean (no or only trivial diffs).

- [ ] **Step 4: Manual integration check (two terminals, same machine)**

Run two instances and connect them (loopback/LAN per README). Confirm:
- Speaking into the mic is audible on the other instance.
- `Ctrl-T` mutes/unmutes (status flips `[LIVE]`/`[MUTED]`; peer stops/starts hearing you).
- `Ctrl-Up/Down` and `Alt-Up/Down` change the displayed percentages and audibly change levels.
- Chat text still sends and receives during a call.

Expected: all behaviors verified. (This step needs real audio hardware; it is the only non-automated check.)

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: document voice transmission, controls, and libopus prerequisite"
```

---

## Self-review notes

- **Spec coverage:** wire format (T1), engine/codec/jitter/gain (T2–T3), live-on-connect + non-fatal failure (T4), intent commands + core-owned state via `AudioState` (T3–T5), keybindings + status render (T5–T6), cross-platform deps + docs (T2, T7). Core files (`proto`/`punch`/`net`/`audio`) carry no `ratatui`/`crossterm` imports; only `app.rs`/`ui.rs` do — the Tauri boundary holds.
- **Type consistency:** `AudioHandle` methods `play`, `toggle_mute`, `adjust_input_volume`, `adjust_output_volume`, `state` are used identically in `net.rs`. `Event::AudioState(crate::audio::AudioState)` is constructed in `net.rs` and destructured in `app.rs`. `Screen::Chat` field set (`muted`, `input_vol`, `output_vol`, `voice`) is consistent across `app.rs` construction/tests and `ui.rs` render/test.
- **Out of scope (unchanged from spec):** push-to-talk, sequence numbers/FEC/PLC, arbitrary-rate resampling, device selection, echo cancellation, Windows.
