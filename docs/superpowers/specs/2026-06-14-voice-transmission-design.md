# Voice transmission for ramsit — design

**Date:** 2026-06-14
**Status:** Approved, ready for implementation plan

## Goal

Add real-time voice over the existing P2P UDP connection. Once two peers connect,
the microphone goes live automatically (like picking up a phone). Provide
mute/unmute and adjustable input (mic) and output (speaker) volume. Must work
well on Linux and macOS using the system default input/output devices.

## Non-negotiable constraint: keep implementation separate from UI

The next planned change is a migration to Tauri. The audio/network core must have
**zero UI dependencies** so that migration only swaps the front-end.

- **Core (no `ratatui`/`crossterm`):** `proto.rs`, `punch.rs`, `net.rs`, `audio.rs`.
  These speak only in the plain-data `Command` (in) and `Event` (out) enums. Tauri
  reuses all of this untouched.
- **UI layer (the only Tauri-replaceable files):** `app.rs` (state machine + crossterm
  keymap) and `ui.rs` (ratatui rendering). Tauri swaps these two and drives the same
  `Command`/`Event` channels.

The `Command`/`Event` channel pair is the sole contract between the two layers.

## Decisions (locked)

| Decision | Choice |
|---|---|
| Codec | Opus, 48 kHz mono, 20 ms frames |
| Mic on connect | Live (press to mute) |
| Volume control | In-app digital gain, 0–200%, ±10% steps |
| Device selection | System default input/output only |
| State ownership | Core (engine) owns authoritative audio state; UI mirrors it |

## 1. Wire format (`proto.rs`)

Audio frames are binary Opus and can start with any byte, so they need a sentinel
prefix to stay distinct from raw-UTF-8 chat.

- New const: `AUDIO_PREFIX = b"\x00AUD"`. An audio packet is `\x00AUD` + Opus payload.
- New `PacketKind::Audio`.
- `classify()` gains a `buf.starts_with(AUDIO_PREFIX)` check. Ordering is safe:
  chat short-circuits on "first byte is not `0x00`"; the exact control strings
  (`PUNCH`, `PUNCH_ACK`, `KEEPALIVE`, `BYE`) do not collide with the `\x00AUD` prefix.

No sequence numbers in v1: frames are decoded in arrival order. Loss/reorder causes
minor artifacts only. This is a documented limitation.

Opus payload at 48 kHz/20 ms is ~80–120 bytes, well within `RECV_BUF` (1500).

## 2. Audio engine (new `audio.rs`)

Dependencies: `cpal` (ALSA on Linux, CoreAudio on macOS) + Opus bindings.

**Fixed format: 48 kHz mono, 20 ms frames (960 samples).** Opus supports only
8/12/16/24/48 kHz; 48 kHz is what PipeWire and CoreAudio provide on request, so we
request 48 kHz from `cpal` and avoid arbitrary-rate resampling. Channels = device
default, downmixed → mono on capture and upmixed on playback. If 48 kHz cannot be
obtained, `start()` returns `Err` (surfaced as `AudioUnavailable`; chat still works).

### Capture path
1. cpal input callback delivers device-format samples.
2. Downmix to mono → apply input gain → accumulate into 960-sample frames.
3. Opus-encode each full frame.
4. Hand encoded bytes to a sender thread that owns `sock.try_clone()` + `peer` and
   does `send_to(\x00AUD + payload)`.
5. **Muted ⇒ do not encode or send** (silence suppression, zero bandwidth).

### Playback path
1. Received Opus payloads are decoded to 48 kHz mono i16.
2. Pushed into a jitter buffer: `Mutex<VecDeque<i16>>`, primed ~40 ms before output
   starts, capped ~500 ms (drop-oldest on overrun), silence on underrun.
3. cpal output callback drains the buffer, applies output gain, upmixes to device
   channels.

### Controls and state
- `Arc<AudioControls>` holds atomics the realtime callbacks read each tick:
  `muted: AtomicBool`, `input_gain` / `output_gain` (f32 bits in `AtomicU32`).
- The engine **owns the authoritative audio state** and performs all 0–200% clamping
  exactly once. Setters: `set_muted`, `toggle_mute`, `adjust_input_gain(delta)`,
  `adjust_output_gain(delta)`. Each returns the resulting `AudioState` snapshot so the
  worker can emit an event.

### Public API (UI-agnostic)
```
start(sock: UdpSocket, peer: SocketAddr) -> Result<AudioEngine>
AudioEngine::play(&self, opus: &[u8])       // decode + enqueue for playback
AudioEngine::toggle_mute(&self) -> AudioState
AudioEngine::adjust_input_gain(&self, delta_pct: i8) -> AudioState
AudioEngine::adjust_output_gain(&self, delta_pct: i8) -> AudioState
AudioEngine::state(&self) -> AudioState
```

`cpal::Stream` is `!Send`, so the engine is created and held entirely on the
network-worker thread (long-lived) and dropped when the session ends. It is never
moved to another thread.

### Testability
The realtime/device code is integration-only (no audio device in CI). Pure helpers
are factored out and unit-tested:
- `downmix_to_mono(samples, channels)`
- `apply_gain(samples, gain)`
- frame accumulator (stream of samples → fixed 960-sample frames)
- Opus encode → decode roundtrip

cpal stream construction is not exercised in unit tests.

## 3. Network worker (`net.rs`)

New `Command` variants (plain data, drained by the session loop):
- `ToggleMute`
- `AdjustInputVolume(i8)`   // signed percent delta
- `AdjustOutputVolume(i8)`

New `Event` variants:
- `AudioState { muted: bool, input_vol: u8, output_vol: u8 }` — emitted once at call
  start and on every change.
- `AudioUnavailable(String)` — engine failed to start; chat continues.

Worker changes:
- After `Connected`, build the engine on the worker thread (`audio::start(sock.try_clone()?, peer)`).
  On success, emit the initial `AudioState`. On failure, emit `AudioUnavailable(msg)`
  and continue with no engine.
- `session()` gains an owned `Option<AudioEngine>` parameter, held for the session
  lifetime and dropped at the end. The loopback test passes `None` (unchanged).
- On `PacketKind::Audio` → `engine.play(payload)` (no-op if no engine).
- On the new audio `Command`s → call the matching engine setter and emit the returned
  `AudioState`.
- Keepalive timing switches from iteration-count to elapsed `Instant`: during a call the
  loop spins far faster than 200 ms per iteration, so the old tick count would fire
  keepalives too often.

## 4. App state machine (`app.rs`)

`Screen::Chat` gains audio fields that are a **pure mirror** of the last `AudioState`
event (not a source of truth):
- `muted: bool`, `input_vol: u8`, `output_vol: u8`, `voice: bool`

`apply()`:
- `Event::AudioState { .. }` → update the mirror fields, set `voice = true`.
- `Event::AudioUnavailable(msg)` → set `voice = false`, push `* voice unavailable: {msg} *`
  into the message log.

`on_key()` (Chat screen) sends intent commands. Bindings are chosen to never collide
with chat typing (bare `Char(c)` feeds the input box) or terminal conventions:
- `Ctrl-T` → `Command::ToggleMute`
- `Ctrl-↑` / `Ctrl-↓` → `Command::AdjustInputVolume(+10 / -10)`
- `Alt-↑` / `Alt-↓` → `Command::AdjustOutputVolume(+10 / -10)`

The UI does no clamping; it renders whatever `AudioState` it last received.

## 5. Rendering (`ui.rs`)

`draw_chat` status bar (history block title) appends an ASCII audio segment:
- `[LIVE] mic 100% spk 100%` or `[MUTED] mic 100% spk 100%`
- A one-line key hint for the audio bindings.
- If `voice == false`, show `[no voice]`.

ASCII only (no emoji) for terminal portability.

## 6. Dependencies & cross-platform

- `cpal = "0.15"`.
- Opus bindings: prefer a crate that **vendors/builds libopus from source** (no system
  package required). To be confirmed during planning that the chosen crate builds
  cleanly on Linux and macOS; documented fallback is the `opus` crate with
  `apt install libopus-dev` / `brew install opus`.
- Jitter buffer uses `std::sync::Mutex<VecDeque>` — no extra dependency.

## 7. Files touched

| File | Change |
|---|---|
| `Cargo.toml` | add `cpal`, Opus crate |
| `proto.rs` | `AUDIO_PREFIX`, `PacketKind::Audio`, `classify` prefix check + tests |
| `audio.rs` | **new** — cpal engine, Opus enc/dec, controls/state, pure helpers + tests |
| `net.rs` | new Commands/Events, start engine on connect, route Audio packets, `Option<AudioEngine>` in `session`, Instant-based keepalive |
| `app.rs` | Chat audio mirror fields, key bindings → intent commands, apply audio events |
| `ui.rs` | audio status segment + key hint in chat status bar |
| `README.md` | document voice + controls |

## Out of scope (v1)

Push-to-talk, sequence numbers / FEC / PLC, arbitrary-rate resampling, device
selection (system default only), echo cancellation, Windows support.

## Testing strategy

- `proto`: `classify` recognizes `AUDIO_PREFIX` → `Audio`; chat/control unaffected.
- `audio`: pure-helper unit tests (downmix, gain, frame accumulator, Opus roundtrip).
- `net`: existing loopback test unchanged (passes `Option::None` engine). The audio
  routing branch is exercised by the proto test plus a worker-level check that an
  `Audio` packet does not reach the chat path.
- Manual integration: two instances on a LAN, confirm two-way voice, mute, and volume.
