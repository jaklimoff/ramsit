# Audio Device Selection, VU Meters & Self-Test — Design

**Date:** 2026-06-14
**Status:** Approved

## Problem

The user hears nothing during testing and suspects device selection. Today the audio
engine:

- Opens the **system default** input/output devices only, with no way to choose
  (`audio::start`, `src-tauri/src/engine/audio.rs:358`).
- Is created **inside the net worker on connect** and torn down with it
  (`src-tauri/src/engine/net.rs:131`), so audio cannot run without a peer.
- Exposes no signal of whether capture or playback is actually moving samples.

This makes it impossible to verify devices in isolation, switch away from a broken
device, or see whether audio data is registering.

## Goals

1. **Device selection** — choose input and output devices, with **live hot-swap**
   (change mid-test or mid-call; the affected stream rebuilds in place).
2. **VU (peak) meters** — live input and output level meters so the user can see
   samples registering.
3. **Solo self-test mode** — exercise devices **without** a peer connection: input
   VU from the mic and a **test-tone button** that plays to the selected output and
   lights the output VU (no mic dependency — isolates the output path).
4. **Persistence** — remember the chosen devices across app restarts; fall back to
   the system default if a saved device is gone.

Non-goals: mic loopback / hear-yourself, codec-in-the-loop loopback, per-call device
profiles, multi-channel metering.

## Architecture

The self-test requires audio to run independently of the call, so the engine is
decoupled from the call lifecycle into a **long-lived `AudioEngine` actor** created at
app startup. This mirrors the existing net-worker actor pattern and sidesteps
`cpal::Stream` `Send`/`Sync` constraints in Tauri state (the streams never leave the
engine thread).

```
AppHandle ──┐
            ▼
       AudioEngine actor (own thread; owns cpal input+output Streams)
        ▲   │   reads/writes
   AudioCmd │   ▼
   channel  │  Arc<Shared>  ── controls, meters, jitter, decoder, flags
            │      ▲
 net worker ┘      │ reads (30 Hz)
 (StartCall/       │
  EndCall +     level-emitter thread ── emits `levels` via AppHandle
  push packets)
```

### Modes

The engine is in exactly one mode:

- **Idle** — no streams open; the mic is released.
- **Test** — input + output streams open; input feeds the meter, output is silent
  until the test tone is toggled on.
- **Call** — input + output streams open; captured PCM is encoded and sent to the
  peer, received packets are decoded into the jitter buffer and played out.

Streams open on entering Test or Call and close on returning to Idle. Entering Call
supersedes Test.

### `AudioCmd` (engine command channel)

`SetInputDevice(Option<String>)`, `SetOutputDevice(Option<String>)`, `StartTest`,
`StopTest`, `Tone(bool)`, `StartCall { socket, peer }`, `EndCall`, `ToggleMute`,
`SetInputVol(u8)`, `SetOutputVol(u8)`, `Shutdown`.

`None` device = system default.

### `Shared` (Arc, `Send + Sync`, stored in Tauri `State`)

- `controls`: existing `muted` / `input_vol` / `output_vol` atomics.
- `meters`: `input_peak: AtomicU32`, `output_peak: AtomicU32` (running max of
  `|sample|`, read-and-reset by the emitter).
- `jitter`: `Mutex<VecDeque<i16>>` (playback buffer, unchanged).
- `decoder`: `Mutex<opus::Decoder>` (unchanged).
- `call_active: AtomicBool`, `tone_active: AtomicBool`.

The net worker holds a clone of the bits it needs (jitter + decoder for inbound
packets) and sends `StartCall`/`EndCall` to the engine.

### Hot-swap

`SetInputDevice` / `SetOutputDevice` rebuild **only** the affected stream in place;
`controls`, `jitter`, `meters` survive. Applies in Test and Call mode. On open
failure the previous stream is kept and an error event is emitted.

## Metering

- **Input callback:** downmix to mono → update `input_peak` (running max) → if
  `call_active`, forward PCM to the encoder path.
- **Output callback:** fill the buffer from the test tone (if `tone_active`) or the
  jitter buffer → apply output gain → update `output_peak`.
- **Level-emitter thread:** every ~33 ms (30 Hz) reads-and-resets both peaks,
  normalizes to `f32` 0–1 (`peak / 32768.0`), and emits
  `Levels { input, output }`. Runs while the engine is in Test or Call. The frontend
  applies decay for smooth VU behavior.

## Test tone

`Tone(true)` sets `tone_active`; the output callback synthesizes a 440 Hz sine at the
device sample rate until `Tone(false)`. Metered like any other output, so the output
VU lights without needing the mic.

## Device enumeration & persistence

- `list_audio_devices()` → `{ inputs: string[], outputs: string[], currentInput,
  currentOutput }`, queried directly from cpal (no running engine required).
- `Settings { input_device: Option<String>, output_device: Option<String> }`
  serialized as JSON in the Tauri app-config dir. Loaded at startup and applied when
  streams open. A saved device that is no longer present → fall back to system default
  and emit a notice. Saved on every device change.

## Tauri commands & events

**New commands** (registered in `src-tauri/src/bridge.rs`):

- `list_audio_devices() -> DeviceList`
- `set_input_device(name: Option<String>)`
- `set_output_device(name: Option<String>)`
- `start_audio_test()`
- `stop_audio_test()`
- `play_test_tone(on: bool)`

**Changed:** `toggle_mute`, `set_input_volume`, `set_output_volume` route to the
`AudioEngine` and now also work in Test mode.

**New event:** `levels` (`{ type: "levels", input: f32, output: f32 }`) on the
existing `engine-event` channel. `AudioUnavailable` is reused for device-open and
fallback notices.

## Frontend (React)

Reusable components:

- `<DeviceSelect>` — input and output dropdowns populated from `list_audio_devices()`;
  changing a selection invokes `set_input_device` / `set_output_device`.
- `<VuMeter>` — animated horizontal bar driven by the `levels` event with peak-decay
  smoothing.

Placement:

- **Exchange screen** — an Audio self-test panel: the two device dropdowns, two VU
  meters, a Start/Stop test toggle, and a Play-test-tone button.
- **Chat screen** — the same dropdowns + two live meters for hot-swap during a call.

`reducer.ts` gains a `levels` case feeding meter state; `engine.ts` gains typed
wrappers for the new commands.

## Error handling

- Device open / hot-swap failure: keep the prior stream, emit `AudioUnavailable` with
  the reason; the UI surfaces it without crashing.
- Saved device missing at startup: silently fall back to default, emit a notice.
- All engine commands are non-blocking sends; a dead engine thread degrades to "no
  audio" rather than hanging the UI.

## Testing

Unit tests (Rust):

- Peak → normalized `f32` conversion and running-max read-and-reset.
- Test-tone generation (frequency/amplitude sanity at a given rate).
- `Settings` load/save round-trip and missing-device fallback.
- `AudioCmd` mode transitions: idle → test → call → end → idle, including stream
  open/close expectations.

Manual (hardware-dependent): actual device capture/playback, hot-swap, and that the
meters and test tone behave on real devices.
