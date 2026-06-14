# Audio Engine Phase 1 (Self-Test + VU Meters) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make audio runnable without a peer (a solo self-test with input/output VU meters and a test tone) by extracting a long-lived `AudioEngine` actor that owns capture/playback for both self-test and calls.

**Architecture:** A long-lived `AudioEngine` actor (its own thread, owns the `cpal` streams) is created at app startup and stored in Tauri state. It has three modes — Idle/Test/Call. The realtime callbacks update lock-free peak meters; a level-emitter thread reports them at ~30 Hz via a UI-agnostic callback that the bridge maps to a `levels` event. The net worker no longer owns audio: on connect it calls `start_call(socket, peer)`, routes inbound packets to `play()`, and calls `end_call()` on disconnect.

**Tech Stack:** Rust (cpal 0.18, opus 0.3, rubato 0.16, anyhow), Tauri 2, React 18 + TypeScript, vitest. Rust tests run with `cargo test` from `src-tauri/`. Frontend tests run with `pnpm test`.

**Spec:** `docs/superpowers/specs/2026-06-14-audio-device-selection-and-vu-meters-design.md` (this is Phase 1 of three).

**Scope note — what Phase 1 does NOT include:** device enumeration/selection dropdowns, settings persistence, and live device hot-swap (Phases 2 and 3). Phase 1 operates on the **system default** devices.

---

## File Structure

**New Rust files:**
- `src-tauri/src/engine/meter.rs` — `Meters` (lock-free peak atomics) + `frame_peak`. Pure, unit-tested.
- `src-tauri/src/engine/tone.rs` — `ToneGen` (phase-accumulator sine). Pure, unit-tested.
- `src-tauri/src/engine/audio_engine.rs` — the actor: `AudioCmd`, `AudioEvent`, `Shared`, `CallSink`, `AudioEngineHandle`, `spawn`, engine thread, capture pump, level emitter.

**Modified Rust files:**
- `src-tauri/src/engine/audio.rs` — make `Controls` public; add meter + tone taps to the stream builders; remove the old `start`/`encoder_loop`/`AudioHandle` (their roles move to `audio_engine.rs`). Keeps the DSP primitives (`MonoResampler`, `gain_sample`, `downmix`, `prefers_48k`, `AudioState`, constants) and their tests.
- `src-tauri/src/engine/mod.rs` — declare `meter`, `tone`, `audio_engine`.
- `src-tauri/src/lib.rs` — re-export the new modules.
- `src-tauri/src/engine/net.rs` — `spawn`/`worker`/`session` take an `AudioEngineHandle`; drop the `ToggleMute`/`SetInputVolume`/`SetOutputVolume` commands and the `AudioState`/`AudioUnavailable` events (audio events now originate in the engine).
- `src-tauri/src/bridge.rs` — create the engine in `setup`, store its handle in `AppState`, add `start_audio_test`/`stop_audio_test`/`play_test_tone` commands, route mute/volume to the engine, map `AudioEvent` to JSON.

**New frontend files:**
- `src/levels.ts` — an external store fed by the `levels` event (kept out of the reducer).
- `src/components/VuMeter.tsx` — the meter bar.
- `src/components/AudioTest.tsx` — the self-test panel (meters + Start/Stop test + test-tone button).

**Modified frontend files:**
- `src/engine.ts` — add `levels` to `EngineEvent`; add `startAudioTest`/`stopAudioTest`/`playTestTone` wrappers.
- `src/App.tsx` — filter `levels` out before dispatching to the reducer.
- `src/screens/Exchange.tsx` — mount `<AudioTest />`.
- `src/screens/Chat.tsx` — mount two `<VuMeter />`s.

**Build/compile note:** The crate stays green through Task 4 (new modules are additive). Tasks 5–6 are a single switchover (net + bridge) that removes the old audio path; the crate is expected to be red **between** Task 5 and Task 6 and green again at the end of Task 6. Do not commit a red tree — commit Task 5 and Task 6 together if needed (the step commits say so).

---

## Task 1: Peak meters (`meter.rs`)

**Files:**
- Create: `src-tauri/src/engine/meter.rs`
- Modify: `src-tauri/src/engine/mod.rs`, `src-tauri/src/lib.rs`

- [ ] **Step 1: Declare the module**

In `src-tauri/src/engine/mod.rs`, add the line (keep the existing lines):

```rust
pub mod audio;
pub mod meter;
pub mod net;
pub mod proto;
pub mod punch;
```

In `src-tauri/src/lib.rs`, change the re-export line to include `meter`:

```rust
pub use engine::{audio, meter, net, proto, punch};
```

- [ ] **Step 2: Write the failing test**

Create `src-tauri/src/engine/meter.rs` with ONLY the tests first:

```rust
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
        // Second take after no writes is zero (read-and-reset).
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
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml meter::`
Expected: FAIL — `cannot find type Meters` / `frame_peak`.

- [ ] **Step 4: Write the implementation**

Prepend to `src-tauri/src/engine/meter.rs` (above the `#[cfg(test)]` block):

```rust
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
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml meter::`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/engine/meter.rs src-tauri/src/engine/mod.rs src-tauri/src/lib.rs
git commit -m "feat(audio): add lock-free peak meters"
```

---

## Task 2: Test-tone generator (`tone.rs`)

**Files:**
- Create: `src-tauri/src/engine/tone.rs`
- Modify: `src-tauri/src/engine/mod.rs`, `src-tauri/src/lib.rs`

- [ ] **Step 1: Declare the module**

In `src-tauri/src/engine/mod.rs` add `pub mod tone;` (alphabetical order is fine but not required). In `src-tauri/src/lib.rs` add `tone` to the re-export:

```rust
pub use engine::{audio, meter, net, proto, punch, tone};
```

- [ ] **Step 2: Write the failing test**

Create `src-tauri/src/engine/tone.rs` with ONLY the tests:

```rust
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
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml tone::`
Expected: FAIL — `cannot find type ToneGen`.

- [ ] **Step 4: Write the implementation**

Prepend to `src-tauri/src/engine/tone.rs`:

```rust
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
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml tone::`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/engine/tone.rs src-tauri/src/engine/mod.rs src-tauri/src/lib.rs
git commit -m "feat(audio): add phase-accumulator test-tone generator"
```

---

## Task 3: Meter + tone taps in the stream builders

Modify `audio.rs` so the input stream records its peak and the output stream records its peak and can emit the test tone. Make `Controls` public so the new engine module can own it. The old `start`/`encoder_loop`/`AudioHandle` are temporarily updated to keep compiling; they are removed in Task 5.

**Files:**
- Modify: `src-tauri/src/engine/audio.rs`

- [ ] **Step 1: Make `Controls` public and import the new helpers**

In `src-tauri/src/engine/audio.rs`, add imports near the top (after the existing `use` lines):

```rust
use crate::meter::{frame_peak, Meters};
use crate::tone::ToneGen;
```

Change the `Controls` declaration from `struct Controls {` to:

```rust
pub struct Controls {
    pub muted: AtomicBool,
    pub input_vol: AtomicU32,  // percent
    pub output_vol: AtomicU32, // percent
}
```

Keep the existing `impl Controls { fn snapshot ... }` but make it public:

```rust
impl Controls {
    pub fn snapshot(&self) -> AudioState {
```

- [ ] **Step 2: Add the meter tap to the input callback**

Replace `input_stream<T>` (currently around lines 275–295) so it takes a `meters` arg and records the mono-frame peak. The mono `Vec` is already produced for the encoder send, so the meter adds no new allocation:

```rust
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
```

Update `build_input` to thread `meters` through:

```rust
fn build_input(
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
```

- [ ] **Step 3: Add the meter + tone to the output callback**

Replace `output_stream<T>` (currently around lines 312–353) so it takes `meters` and a `tone_active` flag, builds a `ToneGen` at the device rate, and either synthesizes the tone or plays the jitter buffer — recording the emitted peak either way:

```rust
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
            let mut peak = 0u32;
            if tone_active.load(Ordering::Relaxed) {
                for frame in data.chunks_mut(channels) {
                    let s = tone.next_sample();
                    peak = peak.max((s as i32).unsigned_abs());
                    let out = T::from_sample(s);
                    for o in frame.iter_mut() {
                        *o = out;
                    }
                }
            } else {
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
                    peak = peak.max((sample as i32).unsigned_abs());
                    let out = T::from_sample(sample);
                    for o in frame.iter_mut() {
                        *o = out;
                    }
                }
            }
            meters.record_output(peak);
        },
        |e| log::warn!("audio: output stream error: {e}"),
        None,
    )?;
    Ok(stream)
}
```

Update `build_output` to thread the new args:

```rust
fn build_output(
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
```

- [ ] **Step 4: Make the builders and primitives reachable from the engine module**

The engine module (Task 4) calls these and the device helpers. Make them and `AudioStreams`' fields visible to the sibling module by marking them `pub(crate)`:

- Change `fn build_input` → `pub(crate) fn build_input`.
- Change `fn build_output` → `pub(crate) fn build_output`.
- Change `fn prefers_48k` → `pub(crate) fn prefers_48k`.
- Change `pub struct AudioStreams { _input: ..., _output: ... }` field visibility so the engine can construct it:

```rust
pub struct AudioStreams {
    pub(crate) _input: cpal::Stream,
    pub(crate) _output: cpal::Stream,
}
```

- [ ] **Step 5: Keep the old `start` compiling (temporary)**

`start` and `encoder_loop` still exist and are removed in Task 5. Update `start`'s two builder calls so the crate compiles. In `start`, just before `build_input` is called, add a throwaway meters value and pass the new args:

Find the input build call (around line 389) and replace:

```rust
    let (pcm_tx, pcm_rx) = channel::<Vec<i16>>();
    let input = build_input(&in_dev, in_sc, in_channels, in_cfg.sample_format(), pcm_tx)?;
```

with:

```rust
    let meters = Arc::new(Meters::default());
    let tone_active = Arc::new(AtomicBool::new(false));
    let (pcm_tx, pcm_rx) = channel::<Vec<i16>>();
    let input = build_input(
        &in_dev,
        in_sc,
        in_channels,
        in_cfg.sample_format(),
        meters.clone(),
        pcm_tx,
    )?;
```

Find the output build call (around line 408) and replace:

```rust
    let output = build_output(
        &out_dev,
        out_sc,
        out_channels,
        out_cfg.sample_format(),
        jitter.clone(),
        controls.clone(),
    )?;
```

with:

```rust
    let output = build_output(
        &out_dev,
        out_sc,
        out_channels,
        out_cfg.sample_format(),
        out_rate,
        jitter.clone(),
        controls.clone(),
        meters.clone(),
        tone_active.clone(),
    )?;
```

- [ ] **Step 6: Build and run existing audio tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml audio::`
Expected: PASS — the existing DSP tests (`gain_*`, `downmix_*`, `resampler_*`, `opus_roundtrip_*`, `clamp_*`) still pass; the crate compiles with the new builder signatures.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/engine/audio.rs
git commit -m "feat(audio): tap peak meters and test tone in the cpal streams"
```

---

## Task 4: The `AudioEngine` actor (`audio_engine.rs`)

Add the actor, its handle, the per-call encoder sink, the capture pump, and the level-emitter. This module is additive — nothing calls it yet — so the crate stays green. Two pieces are unit-tested (`CallSink` framing and `play` enqueue); the thread/stream wiring is verified by `cargo build` here and manually later.

**Files:**
- Create: `src-tauri/src/engine/audio_engine.rs`
- Modify: `src-tauri/src/engine/mod.rs`, `src-tauri/src/lib.rs`

- [ ] **Step 1: Declare the module**

In `src-tauri/src/engine/mod.rs` add `pub mod audio_engine;`. In `src-tauri/src/lib.rs` add it to the re-export:

```rust
pub use engine::{audio, audio_engine, meter, net, proto, punch, tone};
```

- [ ] **Step 2: Write the failing tests**

Create `src-tauri/src/engine/audio_engine.rs` with ONLY the tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    fn test_controls() -> Arc<Controls> {
        Arc::new(Controls {
            muted: AtomicBool::new(false),
            input_vol: AtomicU32::new(100),
            output_vol: AtomicU32::new(100),
        })
    }

    #[test]
    fn callsink_emits_opus_for_a_full_frame_at_48k() {
        let got = Arc::new(Mutex::new(Vec::<usize>::new()));
        let sink_got = got.clone();
        let mut sink = CallSink::new(
            SAMPLE_RATE,
            test_controls(),
            Box::new(move |bytes: &[u8]| sink_got.lock().unwrap().push(bytes.len())),
        )
        .unwrap();
        // One 20 ms mono frame at 48 kHz is exactly FRAME_SAMPLES; feed two frames.
        sink.process(&vec![0i16; FRAME_SAMPLES * 2]);
        let lens = got.lock().unwrap();
        assert_eq!(lens.len(), 2, "expected two encoded packets");
        assert!(lens.iter().all(|&n| n > 0), "encoded packets must be non-empty");
    }

    #[test]
    fn callsink_sends_nothing_when_muted() {
        let controls = test_controls();
        controls.muted.store(true, Ordering::Relaxed);
        let got = Arc::new(Mutex::new(0usize));
        let sink_got = got.clone();
        let mut sink = CallSink::new(
            SAMPLE_RATE,
            controls,
            Box::new(move |_b: &[u8]| *sink_got.lock().unwrap() += 1),
        )
        .unwrap();
        sink.process(&vec![1234i16; FRAME_SAMPLES * 2]);
        assert_eq!(*got.lock().unwrap(), 0);
    }

    #[test]
    fn play_decodes_and_enqueues_into_jitter() {
        // Encode one silent frame, then verify play() decodes it into the jitter buffer.
        let mut enc =
            opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let mut buf = [0u8; 4000];
        let n = enc.encode(&[0i16; FRAME_SAMPLES], &mut buf).unwrap();

        let shared = Arc::new(Shared::new().unwrap());
        let handle = AudioEngineHandle {
            cmd_tx: channel().0, // unused by play()
            shared: shared.clone(),
        };
        handle.play(&buf[..n]);
        assert_eq!(shared.jitter.lock().unwrap().len(), FRAME_SAMPLES);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml audio_engine::`
Expected: FAIL — `CallSink`, `Shared`, `AudioEngineHandle` not found.

- [ ] **Step 4: Write the module implementation**

Prepend to `src-tauri/src/engine/audio_engine.rs` (above the test block):

```rust
//! The long-lived audio engine actor. Owns the cpal streams on its own thread and
//! services `AudioCmd`s. Capture peaks and playback peaks are reported at ~30 Hz via
//! a UI-agnostic `AudioEvent` callback (the bridge maps it to a Tauri event). The
//! engine is the single owner of audio for both the solo self-test and live calls.

use crate::audio::{
    build_input, build_output, clamp_vol, prefers_48k, AudioState, AudioStreams, Controls,
    MonoResampler, FRAME_SAMPLES, SAMPLE_RATE,
};
use crate::meter::Meters;
use crate::proto::AUDIO_PREFIX;
use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, StreamConfig};
use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// UI-agnostic events the engine reports; the bridge maps these to the frontend
/// `engine-event` JSON. Keeping this Tauri-free preserves audio.rs's "no UI deps" rule.
pub enum AudioEvent {
    Levels { input: f32, output: f32 },
    State(AudioState),
    Unavailable(String),
}

/// Commands to the engine thread. Mute/volume are NOT here — they are plain atomic
/// stores done directly on `Shared` by the handle (see `AudioEngineHandle`).
pub enum AudioCmd {
    StartTest,
    StopTest,
    Tone(bool),
    StartCall { sock: UdpSocket, peer: SocketAddr },
    EndCall,
    Shutdown,
}

/// Shared, `Send + Sync` audio state. Held by the engine thread, the realtime
/// callbacks (via the Arc fields), the capture pump, and the network worker (`play`).
pub struct Shared {
    pub controls: Arc<Controls>,
    pub meters: Arc<Meters>,
    pub jitter: Arc<Mutex<VecDeque<i16>>>,
    pub decoder: Mutex<opus::Decoder>,
    /// Resamples decoded 48 kHz audio to the output device rate; `None` at 48 kHz.
    /// Rewritten whenever the output stream (re)opens.
    pub out_resampler: Mutex<Option<MonoResampler>>,
    pub tone_active: Arc<AtomicBool>,
    /// True while streams are open (Test or Call) — gates the level emitter.
    pub active: AtomicBool,
}

impl Shared {
    pub fn new() -> Result<Self> {
        Ok(Self {
            controls: Arc::new(Controls {
                muted: AtomicBool::new(false),
                input_vol: std::sync::atomic::AtomicU32::new(100),
                output_vol: std::sync::atomic::AtomicU32::new(100),
            }),
            meters: Arc::new(Meters::default()),
            jitter: Arc::new(Mutex::new(VecDeque::new())),
            decoder: Mutex::new(opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono)?),
            out_resampler: Mutex::new(None),
            tone_active: Arc::new(AtomicBool::new(false)),
            active: AtomicBool::new(false),
        })
    }
}

/// Per-call encoder: resamples to 48 kHz, frames, applies input gain, encodes Opus,
/// and hands the bytes to `send`. Protocol-agnostic (the caller prepends the audio
/// prefix and transmits). Lives in the capture-pump thread for a call's duration.
pub struct CallSink {
    enc: opus::Encoder,
    resampler: Option<MonoResampler>,
    controls: Arc<Controls>,
    buf: Vec<i16>,
    enc_buf: [u8; 4000],
    send: Box<dyn FnMut(&[u8]) + Send>,
}

impl CallSink {
    pub fn new(
        in_rate: u32,
        controls: Arc<Controls>,
        send: Box<dyn FnMut(&[u8]) + Send>,
    ) -> Result<Self> {
        let enc = opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)
            .map_err(|e| anyhow!("encoder init: {e}"))?;
        let resampler = if in_rate != SAMPLE_RATE {
            Some(MonoResampler::new(in_rate, SAMPLE_RATE)?)
        } else {
            None
        };
        Ok(Self {
            enc,
            resampler,
            controls,
            buf: Vec::with_capacity(FRAME_SAMPLES * 4),
            enc_buf: [0u8; 4000],
            send,
        })
    }

    pub fn process(&mut self, chunk: &[i16]) {
        match self.resampler.as_mut() {
            Some(r) => self.buf.extend_from_slice(&r.process(chunk)),
            None => self.buf.extend_from_slice(chunk),
        }
        while self.buf.len() >= FRAME_SAMPLES {
            let mut frame: Vec<i16> = self.buf.drain(..FRAME_SAMPLES).collect();
            if self.controls.muted.load(Ordering::Relaxed) {
                continue;
            }
            crate::audio::apply_gain(&mut frame, self.controls.input_vol.load(Ordering::Relaxed));
            match self.enc.encode(&frame, &mut self.enc_buf) {
                Ok(n) => (self.send)(&self.enc_buf[..n]),
                Err(e) => log::warn!("audio: encode failed: {e}"),
            }
        }
    }
}

/// `Send + Sync` handle to the engine. Mute/volume act directly on the shared atomics
/// (no round-trip through the engine thread); everything else is a non-blocking command.
#[derive(Clone)]
pub struct AudioEngineHandle {
    pub(crate) cmd_tx: Sender<AudioCmd>,
    pub(crate) shared: Arc<Shared>,
}

impl AudioEngineHandle {
    pub fn start_test(&self) {
        let _ = self.cmd_tx.send(AudioCmd::StartTest);
    }
    pub fn stop_test(&self) {
        let _ = self.cmd_tx.send(AudioCmd::StopTest);
    }
    pub fn set_tone(&self, on: bool) {
        let _ = self.cmd_tx.send(AudioCmd::Tone(on));
    }
    pub fn start_call(&self, sock: UdpSocket, peer: SocketAddr) {
        let _ = self.cmd_tx.send(AudioCmd::StartCall { sock, peer });
    }
    pub fn end_call(&self) {
        let _ = self.cmd_tx.send(AudioCmd::EndCall);
    }
    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(AudioCmd::Shutdown);
    }

    pub fn toggle_mute(&self) -> AudioState {
        let m = !self.shared.controls.muted.load(Ordering::Relaxed);
        self.shared.controls.muted.store(m, Ordering::Relaxed);
        self.shared.controls.snapshot()
    }
    pub fn set_input_volume(&self, pct: u8) -> AudioState {
        self.shared
            .controls
            .input_vol
            .store(clamp_vol(pct as i32), Ordering::Relaxed);
        self.shared.controls.snapshot()
    }
    pub fn set_output_volume(&self, pct: u8) -> AudioState {
        self.shared
            .controls
            .output_vol
            .store(clamp_vol(pct as i32), Ordering::Relaxed);
        self.shared.controls.snapshot()
    }
    pub fn state(&self) -> AudioState {
        self.shared.controls.snapshot()
    }

    /// Decode a received Opus payload and enqueue it for playback (called by the net
    /// worker on inbound audio). No-op cost when no output stream is consuming.
    pub fn play(&self, payload: &[u8]) {
        let mut dec = self.shared.decoder.lock().unwrap();
        let mut out = [0i16; FRAME_SAMPLES];
        match dec.decode(payload, &mut out, false) {
            Ok(n) => {
                let samples = match self.shared.out_resampler.lock().unwrap().as_mut() {
                    Some(r) => r.process(&out[..n]),
                    None => out[..n].to_vec(),
                };
                let mut jb = self.shared.jitter.lock().unwrap();
                jb.extend(samples);
                while jb.len() > FRAME_SAMPLES * 25 {
                    jb.pop_front();
                }
            }
            Err(e) => log::warn!("audio: decode failed: {e}"),
        }
    }
}

/// Spawn the engine (boots Idle) and the level-emitter thread. `on_event` is invoked
/// off the realtime path for levels/state/errors.
pub fn spawn(on_event: impl Fn(AudioEvent) + Send + Sync + 'static) -> Result<AudioEngineHandle> {
    let shared = Arc::new(Shared::new()?);
    let (cmd_tx, cmd_rx) = channel::<AudioCmd>();
    let handle = AudioEngineHandle {
        cmd_tx,
        shared: shared.clone(),
    };
    let on_event = Arc::new(on_event);

    // Level emitter: ~30 Hz while streams are open.
    {
        let shared = shared.clone();
        let on_event = on_event.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(33));
            if shared.active.load(Ordering::Relaxed) {
                let (input, output) = shared.meters.take();
                on_event(AudioEvent::Levels { input, output });
            }
        });
    }

    // Engine thread: owns the streams (cpal streams are !Send), services commands.
    {
        let shared = shared.clone();
        let play_handle = handle.clone();
        thread::spawn(move || engine_loop(cmd_rx, shared, play_handle, on_event));
    }

    Ok(handle)
}

/// Open the default input+output streams and a capture pump. Returns the stream guard
/// and the pump's PCM sender-side ownership via the engine's `sink_slot`.
fn open_streams(
    shared: &Arc<Shared>,
    sink_slot: &Arc<Mutex<Option<CallSink>>>,
) -> Result<(AudioStreams, u32)> {
    let host = cpal::default_host();
    let in_dev = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no default input device"))?;
    let out_dev = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no default output device"))?;

    let in_cfg = in_dev.default_input_config()?;
    // NOTE: in cpal 0.18.1 `sample_rate` is a plain `u32` (mirror the old `start()`).
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
    let input = build_input(
        &in_dev,
        in_sc,
        in_channels,
        in_cfg.sample_format(),
        shared.meters.clone(),
        pcm_tx,
    )?;

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
        out_rate,
        shared.jitter.clone(),
        shared.controls.clone(),
        shared.meters.clone(),
        shared.tone_active.clone(),
    )?;

    // Output rate determines whether decoded 48 kHz audio needs resampling on play().
    *shared.out_resampler.lock().unwrap() = if out_rate != SAMPLE_RATE {
        Some(MonoResampler::new(SAMPLE_RATE, out_rate)?)
    } else {
        None
    };

    input.play()?;
    output.play()?;

    // Capture pump: drains PCM frames; forwards to the active CallSink when in a call.
    {
        let sink_slot = sink_slot.clone();
        thread::spawn(move || {
            while let Ok(chunk) = pcm_rx.recv() {
                if let Some(sink) = sink_slot.lock().unwrap().as_mut() {
                    sink.process(&chunk);
                }
            }
        });
    }

    log::info!("audio: streams open — in {in_rate}Hz, out {out_rate}Hz");
    Ok((AudioStreams { _input: input, _output: output }, in_rate))
}

fn engine_loop(
    cmd_rx: Receiver<AudioCmd>,
    shared: Arc<Shared>,
    play_handle: AudioEngineHandle,
    on_event: Arc<dyn Fn(AudioEvent) + Send + Sync>,
) {
    let _ = play_handle; // reserved for symmetry; play() is called via the bridge handle
    let mut streams: Option<AudioStreams> = None;
    let mut in_rate: u32 = SAMPLE_RATE;
    let sink_slot: Arc<Mutex<Option<CallSink>>> = Arc::new(Mutex::new(None));

    // Ensure streams are open; report failure once. Returns true if open afterwards.
    let mut ensure_open = |streams: &mut Option<AudioStreams>, in_rate: &mut u32| -> bool {
        if streams.is_some() {
            return true;
        }
        match open_streams(&shared, &sink_slot) {
            Ok((s, rate)) => {
                *streams = Some(s);
                *in_rate = rate;
                shared.active.store(true, Ordering::Relaxed);
                true
            }
            Err(e) => {
                log::warn!("audio: open failed: {e}");
                on_event(AudioEvent::Unavailable(e.to_string()));
                false
            }
        }
    };

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            AudioCmd::StartTest => {
                ensure_open(&mut streams, &mut in_rate);
            }
            AudioCmd::StopTest => {
                // Closing streams releases the mic; dropping the channel ends the pump.
                *sink_slot.lock().unwrap() = None;
                shared.tone_active.store(false, Ordering::Relaxed);
                shared.active.store(false, Ordering::Relaxed);
                streams = None;
            }
            AudioCmd::Tone(on) => {
                shared.tone_active.store(on, Ordering::Relaxed);
            }
            AudioCmd::StartCall { sock, peer } => {
                shared.tone_active.store(false, Ordering::Relaxed); // Call supersedes test tone
                if !ensure_open(&mut streams, &mut in_rate) {
                    continue;
                }
                let prefix = AUDIO_PREFIX.to_vec();
                let mut pkt = Vec::with_capacity(prefix.len() + 400);
                let send = Box::new(move |bytes: &[u8]| {
                    pkt.clear();
                    pkt.extend_from_slice(&prefix);
                    pkt.extend_from_slice(bytes);
                    let _ = sock.send_to(&pkt, peer);
                });
                match CallSink::new(in_rate, shared.controls.clone(), send) {
                    Ok(sink) => {
                        *sink_slot.lock().unwrap() = Some(sink);
                        on_event(AudioEvent::State(shared.controls.snapshot()));
                    }
                    Err(e) => on_event(AudioEvent::Unavailable(e.to_string())),
                }
            }
            AudioCmd::EndCall => {
                // Drop the encoder sink; fall back to Test (streams stay open, meters live).
                *sink_slot.lock().unwrap() = None;
            }
            AudioCmd::Shutdown => {
                *sink_slot.lock().unwrap() = None;
                shared.active.store(false, Ordering::Relaxed);
                streams = None;
                return;
            }
        }
    }
}
```

- [ ] **Step 5: Expose `apply_gain` to the engine module**

`CallSink::process` calls `crate::audio::apply_gain`, which is already `pub`. Confirm `apply_gain`, `clamp_vol`, `MonoResampler`, `FRAME_SAMPLES`, `SAMPLE_RATE`, `AudioState` are all `pub` in `audio.rs` (they are). No change needed if so; if `cargo build` reports any as private, mark it `pub`.

- [ ] **Step 6: Run the tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml audio_engine::`
Expected: PASS (3 tests). Warnings about unused `AudioEngineHandle`/`spawn` are fine — they are wired in Task 5.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/engine/audio_engine.rs src-tauri/src/engine/mod.rs src-tauri/src/lib.rs
git commit -m "feat(audio): add AudioEngine actor with self-test, tone, and call sink"
```

---

## Task 5: Switch the net worker onto the engine

Replace the net worker's self-owned audio (`audio::start` + `AudioHandle`) with the engine handle, and trim the now-engine-owned commands/events. **The crate will not compile after this task until Task 6 is also done** (the bridge must supply the handle). Commit Task 5 + Task 6 together.

**Files:**
- Modify: `src-tauri/src/engine/net.rs`
- Modify: `src-tauri/src/engine/audio.rs` (remove dead `start`/`encoder_loop`/`AudioHandle`)

- [ ] **Step 1: Trim the `Command` and `Event` enums**

In `src-tauri/src/engine/net.rs`, replace the `Command` and `Event` enums (lines 14–35) with:

```rust
/// Messages from the UI thread to the network worker.
#[derive(Debug)]
pub enum Command {
    PeerCode(SocketAddr),
    Send(String),
    Quit,
}

/// Messages from the network worker to the UI thread. Audio state/levels/errors are
/// emitted by the AudioEngine directly (see bridge), not through this channel.
#[derive(Debug)]
pub enum Event {
    Discovered(SocketAddr),
    Connected(SocketAddr),
    Incoming(String),
    PeerLeft,
    Fatal(String),
}
```

- [ ] **Step 2: Update imports and `spawn` to take the engine handle**

At the top of `net.rs`, replace `use crate::audio::{self, AudioHandle};` with:

```rust
use crate::audio_engine::AudioEngineHandle;
```

Replace `spawn` (lines 74–79):

```rust
pub fn spawn(
    stun: SocketAddr,
    audio: AudioEngineHandle,
) -> (JoinHandle<()>, Sender<Command>, Receiver<Event>) {
    let (cmd_tx, cmd_rx) = channel::<Command>();
    let (evt_tx, evt_rx) = channel::<Event>();
    let handle = thread::spawn(move || worker(stun, audio, cmd_rx, evt_tx));
    (handle, cmd_tx, evt_rx)
}
```

Change `worker`'s signature (line 81) to `fn worker(stun: SocketAddr, audio: AudioEngineHandle, cmds: Receiver<Command>, events: Sender<Event>)`.

- [ ] **Step 3: Replace the audio bring-up on connect**

In `worker`, replace the audio start block (lines 128–146) with:

```rust
    // Start voice via the engine. The net worker owns the socket; the engine gets a
    // clone for sending and emits audio state/levels itself.
    match sock.try_clone() {
        Ok(clone) => audio.start_call(clone, peer),
        Err(e) => log::warn!("audio: socket clone failed, voice disabled: {e}"),
    }

    session(sock, peer, cmds, events, Some(audio));
}
```

- [ ] **Step 4: Update `session` to use the engine handle**

Change `session`'s signature (line 155) from `audio: Option<AudioHandle>` to `audio: Option<AudioEngineHandle>`.

Remove the `ToggleMute`/`SetInputVolume`/`SetOutputVolume` match arms in the command drain loop (lines 175–189) — those commands no longer exist.

In the inbound packet handling, the `PacketKind::Audio` arm (lines 219–223) stays the same — it already calls `a.play(...)`, which `AudioEngineHandle` provides:

```rust
                    PacketKind::Audio => {
                        if let Some(a) = &audio {
                            a.play(&buf[AUDIO_PREFIX.len()..n]);
                        }
                    }
```

Add an `end_call` on the two session-exit paths. In the `Command::Quit` arm and the `Disconnected` arm (which send `BYE` and `return`), call `if let Some(a) = &audio { a.end_call(); }` before returning. For example the `Quit` arm becomes:

```rust
                Ok(Command::Quit) => {
                    debug!("session: -> {peer} BYE (local quit)");
                    let _ = sock.send_to(BYE, peer);
                    if let Some(a) = &audio {
                        a.end_call();
                    }
                    return;
                }
```

Apply the same `end_call()` call before `return` in the `Err(TryRecvError::Disconnected)` arm.

- [ ] **Step 5: Fix the net.rs tests' `session` calls**

The two `session(...)` test calls pass `None` as the last arg. They still typecheck because `None` is `Option<AudioEngineHandle>`. No change needed. (If the compiler needs a type hint, write `None::<AudioEngineHandle>`.)

- [ ] **Step 6: Remove the dead audio code**

In `src-tauri/src/engine/audio.rs`, delete:
- the entire `AudioHandle` struct and its `impl` (lines ~65–121),
- the `encoder_loop` function (lines ~207–259),
- the `start` function (lines ~355–448).

Keep `Controls`, `AudioState`, `MonoResampler`, `gain_sample`, `apply_gain`, `downmix`, `clamp_vol`, `prefers_48k`, `build_input`, `build_output`, `input_stream`, `output_stream`, `AudioStreams`, the constants, and the `#[cfg(test)]` DSP tests.

Remove now-unused imports from `audio.rs`: `use std::net::{SocketAddr, UdpSocket};` and `use crate::proto::AUDIO_PREFIX;` (the engine module owns those now). Leave `Receiver`/`Sender` only if still referenced by the builders (they are, via `pcm_tx: Sender<Vec<i16>>`).

- [ ] **Step 7: (No standalone build here — proceed to Task 6, then build.)**

The crate references `net::spawn(stun, audio)` from the bridge, which is updated in Task 6. Do the build verification at the end of Task 6.

---

## Task 6: Wire the engine into the bridge

Create the engine in `setup`, store its handle in `AppState`, pass it to `net::spawn`, add the self-test commands, route mute/volume to the engine, and map `AudioEvent` to JSON.

**Files:**
- Modify: `src-tauri/src/bridge.rs`

- [ ] **Step 1: Update imports and `AppState`**

In `src-tauri/src/bridge.rs`, update the top imports:

```rust
use crate::audio::AudioState;
use crate::audio_engine::{self, AudioEngineHandle, AudioEvent};
use crate::net::{self, Command, Event};
use crate::proto::parse_code;
use serde_json::{json, Value};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
```

Change `AppState`:

```rust
struct AppState {
    stun: SocketAddr,
    cmd_tx: Mutex<Option<Sender<Command>>>,
    audio: AudioEngineHandle,
}
```

- [ ] **Step 2: Add JSON mappers for audio**

Add a helper next to `event_to_json` and remove the `AudioState`/`AudioUnavailable` arms from `event_to_json` (those `Event` variants no longer exist):

```rust
fn event_to_json(ev: &Event) -> Value {
    match ev {
        Event::Discovered(addr) => json!({ "type": "discovered", "code": addr.to_string() }),
        Event::Connected(addr) => json!({ "type": "connected", "peer": addr.to_string() }),
        Event::Incoming(s) => json!({ "type": "incoming", "text": s }),
        Event::PeerLeft => json!({ "type": "peerLeft" }),
        Event::Fatal(s) => json!({ "type": "fatal", "message": s }),
    }
}

fn audio_state_json(st: &AudioState) -> Value {
    json!({
        "type": "audioState",
        "muted": st.muted,
        "inputVol": st.input_vol,
        "outputVol": st.output_vol,
    })
}

fn audio_event_json(ev: &AudioEvent) -> Value {
    match ev {
        AudioEvent::Levels { input, output } => {
            json!({ "type": "levels", "input": input, "output": output })
        }
        AudioEvent::State(st) => audio_state_json(st),
        AudioEvent::Unavailable(reason) => json!({ "type": "audioUnavailable", "reason": reason }),
    }
}
```

- [ ] **Step 3: Pass the engine handle into `net::spawn`**

In the `start` command, change the spawn call:

```rust
    let (_handle, cmd_tx, evt_rx) = net::spawn(state.stun, state.audio.clone());
```

- [ ] **Step 4: Route mute/volume to the engine and emit the new state**

Replace `toggle_mute`, `set_input_volume`, `set_output_volume`:

```rust
#[tauri::command]
fn toggle_mute(app: AppHandle, state: State<AppState>) {
    let st = state.audio.toggle_mute();
    let _ = app.emit(EVENT_CHANNEL, audio_state_json(&st));
}

#[tauri::command]
fn set_input_volume(app: AppHandle, pct: u8, state: State<AppState>) {
    let st = state.audio.set_input_volume(pct);
    let _ = app.emit(EVENT_CHANNEL, audio_state_json(&st));
}

#[tauri::command]
fn set_output_volume(app: AppHandle, pct: u8, state: State<AppState>) {
    let st = state.audio.set_output_volume(pct);
    let _ = app.emit(EVENT_CHANNEL, audio_state_json(&st));
}
```

- [ ] **Step 5: Add the self-test commands**

```rust
#[tauri::command]
fn start_audio_test(state: State<AppState>) {
    state.audio.start_test();
}

#[tauri::command]
fn stop_audio_test(state: State<AppState>) {
    state.audio.stop_test();
}

#[tauri::command]
fn play_test_tone(on: bool, state: State<AppState>) {
    state.audio.set_tone(on);
}
```

- [ ] **Step 6: Create the engine in `setup` and register the commands**

Replace the `tauri::Builder` block in `run()`. Remove the `.manage(AppState {...})` call and instead build the engine + state in `.setup()`, where `AppHandle` is available for the level/state/error callback:

```rust
    tauri::Builder::default()
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let audio = audio_engine::spawn(move |ev: AudioEvent| {
                let _ = app_handle.emit(EVENT_CHANNEL, audio_event_json(&ev));
            })
            .expect("failed to start audio engine");
            app.manage(AppState {
                stun,
                cmd_tx: Mutex::new(None),
                audio,
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                if let Some(state) = window.try_state::<AppState>() {
                    if let Some(tx) = state.cmd_tx.lock().unwrap().as_ref() {
                        let _ = tx.send(Command::Quit);
                    }
                    state.audio.shutdown();
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        })
        .invoke_handler(tauri::generate_handler![
            start,
            submit_peer_code,
            send_message,
            toggle_mute,
            set_input_volume,
            set_output_volume,
            start_audio_test,
            stop_audio_test,
            play_test_tone,
            quit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
```

Note: `stun` is moved into the `setup` closure, so the closure must be `move`. `resolve_stun` is called before the builder as today.

- [ ] **Step 7: Fix the bridge tests**

The `event_to_json` test `maps_audio_state_with_camel_case_keys` references `Event::AudioState`, which no longer exists. Replace that test with one for the new mapper:

```rust
    #[test]
    fn maps_audio_state_with_camel_case_keys() {
        use crate::bridge::audio_state_json;
        let v = audio_state_json(&AudioState {
            muted: true,
            input_vol: 80,
            output_vol: 120,
        });
        assert_eq!(v["type"], "audioState");
        assert_eq!(v["muted"], true);
        assert_eq!(v["inputVol"], 80);
        assert_eq!(v["outputVol"], 120);
    }
```

Make `audio_state_json` reachable from the test module: it is a private fn in the same file, so `use super::audio_state_json;` in the test `mod tests` works (replace the existing `use super::event_to_json;` with `use super::{audio_state_json, event_to_json};`). Update the `use crate::audio::AudioState;` import in the test module if not already present.

- [ ] **Step 8: Build and run the full Rust test suite**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: PASS — all DSP, meter, tone, audio_engine, net, and bridge tests compile and pass. Resolve any leftover unused-import warnings flagged as errors.

- [ ] **Step 9: Commit (Task 5 + Task 6 together)**

```bash
git add src-tauri/src/engine/net.rs src-tauri/src/engine/audio.rs src-tauri/src/bridge.rs
git commit -m "refactor(audio): route calls and controls through the AudioEngine"
```

---

## Task 7: Frontend engine bindings

**Files:**
- Modify: `src/engine.ts`
- Modify: `src/App.tsx`

- [ ] **Step 1: Add the `levels` event type and command wrappers**

In `src/engine.ts`, add `levels` to `EngineEvent` and the new wrappers:

```typescript
export type EngineEvent =
  | { type: "discovered"; code: string }
  | { type: "connected"; peer: string }
  | { type: "incoming"; text: string }
  | { type: "audioState"; muted: boolean; inputVol: number; outputVol: number }
  | { type: "audioUnavailable"; reason: string }
  | { type: "levels"; input: number; output: number }
  | { type: "peerLeft" }
  | { type: "fatal"; message: string };
```

Add to the `engine` object:

```typescript
export const engine = {
  start: () => invoke<void>("start"),
  submitPeerCode: (code: string) => invoke<void>("submit_peer_code", { code }),
  sendMessage: (text: string) => invoke<void>("send_message", { text }),
  toggleMute: () => invoke<void>("toggle_mute"),
  setInputVolume: (pct: number) => invoke<void>("set_input_volume", { pct }),
  setOutputVolume: (pct: number) => invoke<void>("set_output_volume", { pct }),
  startAudioTest: () => invoke<void>("start_audio_test"),
  stopAudioTest: () => invoke<void>("stop_audio_test"),
  playTestTone: (on: boolean) => invoke<void>("play_test_tone", { on }),
  quit: () => invoke<void>("quit"),
};
```

- [ ] **Step 2: Keep `levels` out of the reducer**

In `src/App.tsx`, filter `levels` before dispatching (line 17):

```tsx
    onEngineEvent((e) => {
      if (e.type !== "levels") dispatch(e as Action);
    }).then((fn) => {
      unlisten = fn;
      engine.start(); // start AFTER the listener is attached
    });
```

- [ ] **Step 3: Type-check**

Run: `pnpm build`
Expected: `tsc` passes (the Vite build runs after). If you only want the type check, run `pnpm exec tsc --noEmit`.

- [ ] **Step 4: Commit**

```bash
git add src/engine.ts src/App.tsx
git commit -m "feat(ui): add levels event and audio self-test command bindings"
```

---

## Task 8: Levels store + VuMeter component

**Files:**
- Create: `src/levels.ts`
- Create: `src/levels.test.ts`
- Create: `src/components/VuMeter.tsx`

- [ ] **Step 1: Write the failing test for the levels store**

Create `src/levels.test.ts`:

```typescript
import { describe, it, expect, beforeEach } from "vitest";
import { __setForTest, getInput, getOutput, subscribe } from "./levels";

describe("levels store", () => {
  beforeEach(() => __setForTest(0, 0));

  it("starts at zero", () => {
    expect(getInput()).toBe(0);
    expect(getOutput()).toBe(0);
  });

  it("updates snapshots and notifies subscribers", () => {
    let notified = 0;
    const unsub = subscribe(() => notified++);
    __setForTest(0.5, 0.25);
    expect(getInput()).toBe(0.5);
    expect(getOutput()).toBe(0.25);
    expect(notified).toBe(1);
    unsub();
    __setForTest(0.1, 0.1);
    expect(notified).toBe(1); // no longer subscribed
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `pnpm exec vitest run src/levels.test.ts`
Expected: FAIL — cannot resolve `./levels`.

- [ ] **Step 3: Implement the levels store**

Create `src/levels.ts`. It listens to the `levels` event directly (outside the reducer), tracks the latest input/output as primitives, and exposes a `useSyncExternalStore`-compatible API. `__setForTest` lets tests drive it without Tauri.

```typescript
import { listen } from "@tauri-apps/api/event";

let input = 0;
let output = 0;
const subscribers = new Set<() => void>();

function emit() {
  for (const cb of subscribers) cb();
}

export function subscribe(cb: () => void): () => void {
  subscribers.add(cb);
  return () => subscribers.delete(cb);
}

export function getInput(): number {
  return input;
}

export function getOutput(): number {
  return output;
}

/** Test-only setter; bypasses the Tauri event listener. */
export function __setForTest(i: number, o: number): void {
  input = i;
  output = o;
  emit();
}

// Subscribe once at module load. The reducer ignores `levels`, so this is the only
// consumer — keeping 30 Hz updates off the React reducer path.
listen<{ type: string; input: number; output: number }>("engine-event", (ev) => {
  const p = ev.payload;
  if (p && p.type === "levels") {
    input = p.input;
    output = p.output;
    emit();
  }
});
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `pnpm exec vitest run src/levels.test.ts`
Expected: PASS (2 tests). (The `listen` call is a no-op in the test environment because no event fires; if vitest errors on the missing Tauri internals, the test still passes since `listen` only registers a handler.)

- [ ] **Step 5: Implement the VuMeter component**

Create `src/components/VuMeter.tsx`. It reads the selected channel via `useSyncExternalStore` and renders a bar whose width eases via CSS (the smoothing/decay the spec calls for). Accessible via `role="meter"`.

```tsx
import { useSyncExternalStore } from "react";
import { subscribe, getInput, getOutput } from "../levels";

export default function VuMeter({
  channel,
  label,
}: {
  channel: "input" | "output";
  label: string;
}) {
  const level = useSyncExternalStore(
    subscribe,
    channel === "input" ? getInput : getOutput,
  );
  const pct = Math.round(Math.min(1, Math.max(0, level)) * 100);
  return (
    <div className="vu">
      <span className="vu-label">{label}</span>
      <div
        className="vu-track"
        role="meter"
        aria-label={`${label} level`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
      >
        <div className="vu-fill" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}
```

- [ ] **Step 6: Add minimal styles**

Append to `src/styles.css` (or the app's existing global stylesheet — check `src/main.tsx` for the imported CSS file and use that):

```css
.vu { display: flex; align-items: center; gap: 8px; margin: 4px 0; }
.vu-label { width: 64px; font-size: 12px; opacity: 0.8; }
.vu-track { flex: 1; height: 10px; background: #222; border-radius: 5px; overflow: hidden; }
.vu-fill { height: 100%; background: linear-gradient(90deg, #3a3, #cc3, #c33); transition: width 80ms linear; }
```

- [ ] **Step 7: Commit**

```bash
git add src/levels.ts src/levels.test.ts src/components/VuMeter.tsx src/styles.css
git commit -m "feat(ui): add levels store and VuMeter component"
```

---

## Task 9: Self-test panel + wiring into screens

**Files:**
- Create: `src/components/AudioTest.tsx`
- Modify: `src/screens/Exchange.tsx`
- Modify: `src/screens/Chat.tsx`

- [ ] **Step 1: Implement the self-test panel**

Create `src/components/AudioTest.tsx`. It starts/stops the engine's Test mode, toggles the test tone, and shows both meters. It stops the test on unmount so the mic is released when leaving the screen.

```tsx
import { useEffect, useState } from "react";
import { engine } from "../engine";
import VuMeter from "./VuMeter";

export default function AudioTest() {
  const [testing, setTesting] = useState(false);
  const [tone, setTone] = useState(false);

  // Release the mic when this panel unmounts (e.g. the call connects).
  useEffect(() => {
    return () => {
      engine.playTestTone(false);
      engine.stopAudioTest();
    };
  }, []);

  function toggleTest() {
    if (testing) {
      engine.playTestTone(false);
      engine.stopAudioTest();
      setTone(false);
      setTesting(false);
    } else {
      engine.startAudioTest();
      setTesting(true);
    }
  }

  function toggleTone() {
    const next = !tone;
    engine.playTestTone(next);
    setTone(next);
  }

  return (
    <section className="audio-test">
      <div className="audio-test-controls">
        <button onClick={toggleTest}>
          {testing ? "Stop audio test" : "Test audio devices"}
        </button>
        <button disabled={!testing} onClick={toggleTone}>
          {tone ? "Stop test tone" : "Play test tone"}
        </button>
      </div>
      <VuMeter channel="input" label="Mic" />
      <VuMeter channel="output" label="Speaker" />
    </section>
  );
}
```

- [ ] **Step 2: Mount the panel on the Exchange screen**

In `src/screens/Exchange.tsx`, import and render `<AudioTest />` below the form. Add the import at the top:

```tsx
import AudioTest from "../components/AudioTest";
```

Insert before the closing `</main>` (after the `{error && ...}` line):

```tsx
      <AudioTest />
```

- [ ] **Step 3: Add live meters to the Chat screen**

In `src/screens/Chat.tsx`, import `VuMeter` and render the two meters inside the `.voice` block (so users see live levels during a call; they keep working after `peerLeft` because the engine falls back to Test mode). Add the import:

```tsx
import VuMeter from "../components/VuMeter";
```

Insert at the end of the `<div className="voice">` block, just before its closing `</div>` (after the Speaker `<label>`):

```tsx
        <VuMeter channel="input" label="Mic" />
        <VuMeter channel="output" label="Speaker" />
```

- [ ] **Step 4: Type-check and run frontend tests**

Run: `pnpm exec tsc --noEmit && pnpm test`
Expected: type-check passes; vitest passes (reducer + levels tests).

- [ ] **Step 5: Commit**

```bash
git add src/components/AudioTest.tsx src/screens/Exchange.tsx src/screens/Chat.tsx
git commit -m "feat(ui): add audio self-test panel and live call meters"
```

---

## Task 10: End-to-end manual verification

Automated tests cannot exercise real device I/O. Verify the diagnostic goal by hand.

**Files:** none (verification only).

- [ ] **Step 1: Launch the app**

Run: `pnpm tauri dev`
Expected: the app builds and opens. The Discovering screen appears, then Exchange (with the Audio self-test panel) once STUN resolves.

- [ ] **Step 2: Verify the input meter**

On the Exchange screen, click **Test audio devices**, then speak/tap the mic. Expected: the **Mic** VU bar moves with your voice. If it stays flat, the default input device is the problem (Phase 2 adds selection).

- [ ] **Step 3: Verify the output meter + tone**

Click **Play test tone**. Expected: you hear a 440 Hz tone AND the **Speaker** VU bar lights. If the bar lights but you hear nothing, the default output device/routing is the problem; if neither, the output stream failed (check logs for `audio: open failed`).

- [ ] **Step 4: Verify Stop releases the mic**

Click **Stop audio test**. Expected: meters fall to zero and stop updating; the OS mic indicator turns off.

- [ ] **Step 5: Verify a call still works and meters run**

Connect two instances (or your usual loopback peer). Expected: voice works as before; both meters move during the call. After the peer leaves, the Chat-screen meters keep updating (engine fell back to Test mode), confirming `EndCall → Test`.

- [ ] **Step 6: Confirm no regressions in the suite**

Run: `cargo test --manifest-path src-tauri/Cargo.toml && pnpm test`
Expected: all green.

- [ ] **Step 7: Final commit (if any verification tweaks were needed)**

```bash
git add -A
git commit -m "chore(audio): phase 1 self-test verification"
```

---

## Self-Review Notes

- **Spec coverage:** VU meters (Tasks 1, 3, 8, 9) ✓; solo self-test without a peer (Task 4 Test mode, Task 9 panel) ✓; test tone (Tasks 2, 3, 9) ✓; long-lived engine + StartCall/EndCall handoff (Tasks 4–6) ✓; EndCall→Test fallback (Task 4 `EndCall` arm, verified Task 10 Step 5) ✓; mute/volume routed through the engine and working in Test mode (Task 6 Step 4, since controls are shared atomics independent of mode) ✓; `levels` kept out of the reducer (Tasks 7, 8) ✓; alloc/lock-free realtime callbacks — tone via phase accumulator, peak via atomic `fetch_max`, no new per-callback allocation (Tasks 2, 3) ✓; accessibility `role="meter"` (Task 8) ✓. Deferred to Phase 2/3 by design: device enumeration/selection, persistence, hot-swap.
- **Type consistency:** `frame_peak`/`record_input`/`record_output`/`take` (meter.rs) used consistently in Task 3 and Task 4. `CallSink::new`/`process`, `Shared::new`, `AudioEngineHandle.{start_test,stop_test,set_tone,start_call,end_call,shutdown,toggle_mute,set_input_volume,set_output_volume,state,play}` are defined in Task 4 and called identically in Tasks 5–6. Frontend `getInput`/`getOutput`/`subscribe`/`__setForTest` consistent across Tasks 8–9.
- **`on_event` call site:** it is an `Arc<dyn Fn(AudioEvent) + Send + Sync>` shared by the engine thread and the level-emitter. The plan calls it as `on_event(ev)`. If the compiler does not resolve the call through the `Arc`, use `on_event.as_ref()(ev)` (`&dyn Fn` implements `Fn`). The `let _ = play_handle;` in `engine_loop` intentionally silences the unused-binding lint for the reserved handle.
- **cpal 0.18.1 API note:** in this version `sample_rate` is a plain `u32` (verified against the existing `start()`, which assigns `SAMPLE_RATE`/`in_cfg.sample_rate()` straight into `StreamConfig.sample_rate` and passes it to `u32` params). `open_streams` mirrors that exactly — no `cpal::SampleRate(...)` wrapping, no `.0`. If a future cpal bump changes this, match whatever the (then-updated) builders expect.
