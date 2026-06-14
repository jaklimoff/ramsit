# Audio Device Selection, VU Meters & Self-Test — Design

**Date:** 2026-06-14
**Status:** Approved (revised after architecture review)

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

1. **VU (peak) meters** — live input and output level meters so the user can see
   samples registering.
2. **Solo self-test mode** — exercise devices **without** a peer connection: input
   VU from the mic and a **test-tone button** that plays to the selected output and
   lights the output VU (no mic dependency — isolates the output path).
3. **Device selection** — choose input and output devices.
4. **Live hot-swap** — change a device mid-test or mid-call; the affected stream
   rebuilds in place.
5. **Persistence** — remember the chosen devices across app restarts; fall back to
   the system default if a saved device is gone.

These are sequenced into phases (see **Phasing**): goals 1–2 + the enabling refactor
ship first to diagnose "I hear nothing"; selection/persistence and hot-swap follow.

Non-goals: mic loopback / hear-yourself, codec-in-the-loop loopback, per-call device
profiles, multi-channel metering.

## Architecture

The self-test requires audio to run independently of the call, so the engine is
decoupled from the call lifecycle into a **long-lived `AudioEngine` actor** created at
app startup (peerless). This mirrors the existing net-worker actor pattern and
sidesteps `cpal::Stream` `Send`/`Sync` constraints in Tauri state (the streams never
leave the engine thread).

```
AppHandle ──┐
            ▼
       AudioEngine actor (own thread; owns cpal input+output Streams)
        ▲   │   reads/writes
   AudioCmd │   ▼
   channel  │  Arc<Shared>  ── controls, meters, jitter, decoder, flags
            │      ▲
 net worker ┘      │ reads (≤30 Hz)
 (StartCall/       │
  EndCall +     level-emitter thread ── emits `levels` via AppHandle
  push packets)
```

**Ownership of the socket.** The net worker owns the bound `UdpSocket` (it needs it
for chat, keepalive, and recv — `net.rs:82`). On connect it sends
`StartCall { socket: try_clone(), peer }` to the engine; the engine's clone is dropped
on `EndCall`, so a subsequent call gets a fresh clone for a possibly-new peer. The net
worker continues to call `play()` for inbound audio (it holds a clone of the playback
bits — see `Shared`).

### Modes

The engine is in exactly one mode:

- **Idle** — no streams open; the mic is released. The engine boots into Idle.
- **Test** — input + output streams open; input feeds the meter, output is silent
  until the test tone is toggled on.
- **Call** — input + output streams open; captured PCM is encoded and sent to the
  peer, received packets are decoded into the jitter buffer and played out.

Transitions: `StartTest` → Test, `StopTest` → Idle. `StartCall` → Call (supersedes
Test; if a test tone is active it is force-stopped). **`EndCall` → Test** (streams
stay open and meters keep running, so the Chat screen still shows levels after the
peer leaves); from Test the user can `StopTest` to release the mic.

### `AudioCmd` (engine command channel)

`ListDevices(reply)`, `SetInputDevice(Option<String>)`, `SetOutputDevice(Option<String>)`,
`StartTest`, `StopTest`, `Tone(bool)`, `StartCall { socket, peer }`, `EndCall`,
`ToggleMute`, `SetInputVol(u8)`, `SetOutputVol(u8)`, `Shutdown`.

`None` device = system default. **All `cpal` host/device access is serialized on the
engine thread**, including enumeration: `ListDevices` carries a reply channel and the
engine answers from its own thread, so enumeration never interleaves with stream
operations on another thread.

### Encoder thread & capture path (StartCall/EndCall lifecycle)

The input stream is long-lived (open in Test or Call), but the encoder only exists
during a call:

- The input callback **always** sends mono PCM on a single fixed `Sender` (no
  swappable sender in the realtime path).
- On `StartCall { socket, peer }` the engine spawns the encoder thread, **moving** the
  cloned socket and peer into it (exactly as `audio::start` does today). The encoder
  thread is the sole consumer of the PCM `Receiver`.
- On `EndCall` the engine drops its handle to the encoder so the encoder's
  `recv()` returns `Err` and the thread exits, dropping the socket clone. When no
  encoder exists, PCM frames are simply consumed and discarded (or the channel is
  drained) — capture keeps running for the meter, but nothing is sent.

### `Shared` (Arc, `Send + Sync`, stored in Tauri `State`)

- `controls`: existing `muted` / `input_vol` / `output_vol` atomics.
- `meters`: `input_peak: AtomicU32`, `output_peak: AtomicU32`. Each holds the **integer
  magnitude** `|sample|` (0–32767). Writer: `fetch_max(mag, Relaxed)`. Emitter:
  `swap(0, Relaxed)` then `as f32 / 32768.0`.
- `jitter`: `Mutex<VecDeque<i16>>` (playback buffer). **Rate-coupled to the current
  output device** (see Hot-swap).
- `decoder`: `Mutex<opus::Decoder>` (fixed 48 kHz; rate-invariant).
- `call_active: AtomicBool`, `tone_active: AtomicBool`.

The net worker holds a clone of the playback bits (`jitter` + `decoder` +
`out_resampler` handle) for inbound packets and sends `StartCall`/`EndCall`.

### Hot-swap (Phase 3)

`SetInputDevice` / `SetOutputDevice` rebuild **only** the affected stream in place.
`controls` and `meters` survive. **The jitter buffer and the affected resampler are
rate-coupled to the device and must be reset on swap:**

- Output swap: clear the jitter buffer and rebuild `out_resampler` for the new device
  rate (buffered samples are at the old rate — playing them on the new stream would be
  pitch/speed-wrong).
- Input swap: reset the encoder's input resampler for the new device rate.
- The Opus decoder (fixed 48 kHz) is untouched.

On open failure the previous stream is kept and an error event is emitted.

## Metering

Both callbacks must be **allocation-free and lock-free** in the hot path (cpal
callbacks run on the realtime audio thread; allocations or contended locks cause
xruns — the exact class of glitch being chased):

- **Input callback:** compute a running max magnitude **in place** over the frame (no
  per-callback `Vec`, no `downmix` clone for the meter path) → `fetch_max` into
  `input_peak`. If `call_active`, forward PCM to the encoder via the fixed `Sender`.
- **Output callback:** fill the buffer from the test tone (if `tone_active`) or the
  jitter buffer → apply output gain → `fetch_max` into `output_peak`. (The existing
  jitter `Mutex` lock in this callback is pre-existing and out of scope, but no *new*
  locks are added.)
- **Level-emitter thread:** every ~33 ms (≈30 Hz; may drop to 20 Hz if IPC cost
  warrants) `swap(0)` both peaks, normalize to `f32` 0–1, and emit
  `Levels { input, output }`. Runs while the engine is in Test or Call.

## Test tone

`Tone(true)` sets `tone_active`; the output callback synthesizes a 440 Hz sine at the
device sample rate using a **persistent phase accumulator** (`phase += 2π·440/rate`
per sample, wrapped — never `sin` of an absolute, overflow-prone sample counter), with
no allocation in the callback. Metered like any other output, so the output VU lights
without needing the mic.

## Device enumeration & persistence (Phase 2)

- `list_audio_devices()` → `{ inputs: string[], outputs: string[], currentInput,
  currentOutput }`. Served by the engine via the `ListDevices` command (all cpal
  access serialized on the engine thread).
- `Settings { input_device: Option<String>, output_device: Option<String> }`
  serialized as JSON in the Tauri app-config dir. **Only device selection is persisted
  — never per-tick volume.** The **bridge command handler** owns the file write (the
  engine thread does no blocking disk I/O); written on device change only. Loaded at
  startup and applied when streams open. A saved device that is no longer present →
  fall back to system default and emit a notice. Unreadable/corrupt JSON → fall back
  to defaults without panicking.

## Tauri commands & events

**New commands** (registered in `src-tauri/src/bridge.rs`):

- `list_audio_devices() -> DeviceList` *(Phase 2)*
- `set_input_device(name: Option<String>)` / `set_output_device(name: Option<String>)` *(Phase 2)*
- `start_audio_test()` / `stop_audio_test()` *(Phase 1)*
- `play_test_tone(on: bool)` *(Phase 1)*

**Changed:** `toggle_mute`, `set_input_volume`, `set_output_volume` route to the
`AudioEngine` and now also work in Test mode.

**New event:** `levels` (`{ type: "levels", input: f32, output: f32 }`) on the
existing `engine-event` channel. `AudioUnavailable` is reused for device-open and
fallback notices.

## Frontend (React)

Reusable components:

- `<DeviceSelect>` *(Phase 2)* — input and output dropdowns populated from
  `list_audio_devices()`; changing a selection invokes `set_input_device` /
  `set_output_device`.
- `<VuMeter>` *(Phase 1)* — animated horizontal bar with peak-decay smoothing.
  Accessible: `role="meter"` with `aria-valuenow`/`aria-valuemin`/`aria-valuemax`, or
  a textual "input active / output active" status for screen-reader users.

**`levels` does not flow through the `Screen` reducer.** 30 Hz updates through the
reducer would re-render the whole screen subtree 30×/sec. Instead, `levels` is
delivered on a dedicated subscription (`engine.ts` exposes `onLevels(cb)`) consumed
directly by `<VuMeter>` via a ref / `useSyncExternalStore`, keeping meter values out
of reducer state. `engine.ts` also gains typed wrappers for the new commands.

Placement:

- **Exchange screen** — an Audio self-test panel: two VU meters, a Start/Stop test
  toggle, a Play-test-tone button, and (Phase 2) the two device dropdowns.
- **Chat screen** — two live meters (and, Phase 2, the dropdowns) for monitoring and
  hot-swap during a call. Meters keep running after `peerLeft` because `EndCall` falls
  back to Test mode.

## Error handling

- Device open / hot-swap failure: keep the prior stream, emit `AudioUnavailable` with
  the reason; the UI surfaces it without crashing.
- Saved device missing at startup or corrupt settings: fall back to default, emit a
  notice, never panic.
- All engine commands are non-blocking sends; a dead engine thread degrades to "no
  audio" rather than hanging the UI.

## Phasing

The architecture refactor (long-lived engine + StartCall/EndCall handoff) is shared
across all phases and lands in Phase 1, because the engine now owns audio for calls
too. Each phase is independently shippable.

- **Phase 1 — Diagnose (ships first).** Long-lived `AudioEngine` actor; net-worker
  `StartCall`/`EndCall` handoff; Idle/Test/Call modes; input + output VU meters
  (`levels` event + `<VuMeter>`); test tone; self-test panel on the Exchange screen;
  meters on the Chat screen; mute/volume routed through the engine. Operates on the
  **system default devices**. `list_audio_devices` may be included read-only to show
  what devices exist. This alone answers "is my mic capturing?" and "does my default
  output play?".
- **Phase 2 — Selection + persistence.** `<DeviceSelect>` dropdowns,
  `set_input_device`/`set_output_device` (applied by reopening the engine's streams),
  and persisted `Settings`.
- **Phase 3 — Live hot-swap.** Rebuild a single live stream in place (with jitter /
  resampler reset) so a device can change mid-call without dropping the call.

## Testing

Unit tests (Rust):

- Peak → normalized `f32` conversion and `fetch_max` / `swap(0)` read-and-reset.
- Test-tone phase-accumulator generation (frequency/amplitude sanity at a given rate;
  phase wrap correctness).
- `Settings` load/save round-trip, missing-device fallback, and corrupt-JSON fallback.
- `AudioCmd` mode transitions: idle → test → call → end(→test) → idle, including
  stream open/close and encoder-thread spawn/exit expectations.

Manual (hardware-dependent): actual device capture/playback, hot-swap, and that the
meters and test tone behave on real devices.
