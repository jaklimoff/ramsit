# Audio Engine Phase 2 (Device Selection + Persistence) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Let the user enumerate and choose the input/output audio devices (dropdowns on the self-test panel and Chat screen), persist the choice across restarts, and apply it by reopening the engine's streams.

**Architecture:** The `AudioEngine` actor gains device-name state and three commands (`ListDevices`, `SetInputDevice`, `SetOutputDevice`). `open_streams` picks the named device (falling back to system default). Changing a device while streams are open tears them down and reopens with the new selection (clearing the jitter buffer; rebuilding the call sink if a call is active). All `cpal` enumeration is serialized on the engine thread. The bridge owns a `Settings` file (device names only) loaded at startup and saved on every change.

**Tech Stack:** Rust (cpal 0.18.1, serde, anyhow), Tauri 2, React 18 + TypeScript, vitest. Rust tests: `cargo test --manifest-path src-tauri/Cargo.toml`. Frontend: `pnpm test`. This is **Phase 2 of 3** (Phase 1 shipped; Phase 3 = seamless live hot-swap, NOT in this plan).

**Spec:** `docs/superpowers/specs/2026-06-14-audio-device-selection-and-vu-meters-design.md`.

**Behavior decisions (from the spec's Phasing):**
- Device selection is **applied by reopening streams**, not seamless hot-swap (that's Phase 3). Changing a device mid-test or mid-call reopens both streams (a brief audio blip is acceptable).
- A saved device that is no longer present → fall back to system default (logged); selection still works on the next reopen.
- Persistence stores **device names only** (never volume). The **bridge** owns the file write; the engine thread does no disk I/O.

---

## File Structure
- **New:** `src-tauri/src/settings.rs` — `Settings { input_device, output_device }` + JSON load/save. Unit-tested.
- **Modified:** `src-tauri/src/engine/audio_engine.rs` — `DeviceList`, three new `AudioCmd`s, device-name state, `pick_input`/`pick_output`, `open_streams`/`ensure_open` take device names, `build_call_sink` + `reopen` helpers, `enumerate_devices`, new handle methods, `spawn` takes initial devices.
- **Modified:** `src-tauri/src/lib.rs` — declare `mod settings;`.
- **Modified:** `src-tauri/src/bridge.rs` — load `Settings` at setup + pass to `spawn`; `AppState` holds `Mutex<Settings>` + `config_dir`; new commands `list_audio_devices`/`set_input_device`/`set_output_device`.
- **New:** `src/components/DeviceSelect.tsx` — input/output dropdowns.
- **Modified:** `src/engine.ts` — `DeviceList` type + three wrappers.
- **Modified:** `src/components/AudioTest.tsx`, `src/screens/Chat.tsx` — mount `<DeviceSelect />`.
- **Modified:** `src/styles.css` — dropdown styles.

---

## Task P2-1: Engine device enumeration + selection

**Files:** Modify `src-tauri/src/engine/audio_engine.rs`

- [ ] **Step 1: Add the serde import and `DeviceList`**

At the top of `audio_engine.rs`, add to the imports:
```rust
use serde::Serialize;
```
After the `AudioEvent` enum, add:
```rust
/// Snapshot of available devices + the current selection, sent to the UI.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceList {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub current_input: Option<String>,
    pub current_output: Option<String>,
}
```

- [ ] **Step 2: Extend `AudioCmd`**

Add three variants to `AudioCmd`:
```rust
pub enum AudioCmd {
    StartTest,
    StopTest,
    Tone(bool),
    StartCall { sock: UdpSocket, peer: SocketAddr },
    EndCall,
    SetInputDevice(Option<String>),
    SetOutputDevice(Option<String>),
    ListDevices(Sender<DeviceList>),
    Shutdown,
}
```

- [ ] **Step 3: Add the device-selection handle methods**

In `impl AudioEngineHandle`, add (next to `start_test` etc.):
```rust
    pub fn set_input_device(&self, name: Option<String>) {
        let _ = self.cmd_tx.send(AudioCmd::SetInputDevice(name));
    }
    pub fn set_output_device(&self, name: Option<String>) {
        let _ = self.cmd_tx.send(AudioCmd::SetOutputDevice(name));
    }
    /// Enumerate devices on the engine thread (serializes cpal access). Blocks up to 2s.
    pub fn list_devices(&self) -> DeviceList {
        let (tx, rx) = channel();
        if self.cmd_tx.send(AudioCmd::ListDevices(tx)).is_err() {
            return DeviceList::default();
        }
        rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default()
    }
```

- [ ] **Step 4: Update `spawn` to take initial device names**

Change `spawn`'s signature and the `engine_loop` call:
```rust
pub fn spawn(
    on_event: impl Fn(AudioEvent) + Send + Sync + 'static,
    input_device: Option<String>,
    output_device: Option<String>,
) -> Result<AudioEngineHandle> {
```
and inside, change the engine-thread spawn to:
```rust
    {
        let shared = shared.clone();
        thread::spawn(move || engine_loop(cmd_rx, shared, on_event, input_device, output_device));
    }
```
(Leave the level-emitter thread unchanged.)

- [ ] **Step 5: Replace `open_streams`, `ensure_open` (was a closure), and `engine_loop` with the device-aware versions + helpers**

Replace the ENTIRE span from `fn open_streams(` through the end of `fn engine_loop(...)` (i.e. everything between the `spawn` function and the `#[cfg(test)]` module) with:

```rust
/// Find the named input device, or the system default if `name` is None / not found.
fn pick_input(host: &cpal::Host, name: &Option<String>) -> Result<cpal::Device> {
    if let Some(n) = name {
        if let Ok(mut devs) = host.input_devices() {
            if let Some(d) = devs.find(|d| d.name().ok().as_deref() == Some(n.as_str())) {
                return Ok(d);
            }
        }
        log::warn!("audio: input device '{n}' not found; using default");
    }
    host.default_input_device()
        .ok_or_else(|| anyhow!("no default input device"))
}

/// Find the named output device, or the system default if `name` is None / not found.
fn pick_output(host: &cpal::Host, name: &Option<String>) -> Result<cpal::Device> {
    if let Some(n) = name {
        if let Ok(mut devs) = host.output_devices() {
            if let Some(d) = devs.find(|d| d.name().ok().as_deref() == Some(n.as_str())) {
                return Ok(d);
            }
        }
        log::warn!("audio: output device '{n}' not found; using default");
    }
    host.default_output_device()
        .ok_or_else(|| anyhow!("no default output device"))
}

/// Enumerate all device names plus the current selection. Runs on the engine thread.
fn enumerate_devices(in_dev: &Option<String>, out_dev: &Option<String>) -> DeviceList {
    let host = cpal::default_host();
    let inputs = host
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    let outputs = host
        .output_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    DeviceList {
        inputs,
        outputs,
        current_input: in_dev.clone(),
        current_output: out_dev.clone(),
    }
}

/// Open the selected input+output streams and a capture pump. Returns the stream guard
/// and the negotiated input rate.
fn open_streams(
    shared: &Arc<Shared>,
    sink_slot: &Arc<Mutex<Option<CallSink>>>,
    in_dev: &Option<String>,
    out_dev: &Option<String>,
) -> Result<(AudioStreams, u32)> {
    let host = cpal::default_host();
    let in_dev = pick_input(&host, in_dev)?;
    let out_dev = pick_output(&host, out_dev)?;

    let in_cfg = in_dev.default_input_config()?;
    // NOTE: in cpal 0.18.1 `sample_rate` is a plain `u32`.
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

    *shared.out_resampler.lock().unwrap() = if out_rate != SAMPLE_RATE {
        Some(MonoResampler::new(SAMPLE_RATE, out_rate)?)
    } else {
        None
    };

    input.play()?;
    output.play()?;

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
    Ok((
        AudioStreams {
            _input: input,
            _output: output,
        },
        in_rate,
    ))
}

/// Ensure streams are open; report failure once. Returns true if open afterwards.
fn ensure_open(
    shared: &Arc<Shared>,
    sink_slot: &Arc<Mutex<Option<CallSink>>>,
    streams: &mut Option<AudioStreams>,
    in_rate: &mut u32,
    in_dev: &Option<String>,
    out_dev: &Option<String>,
    on_event: &Arc<dyn Fn(AudioEvent) + Send + Sync>,
) -> bool {
    if streams.is_some() {
        return true;
    }
    match open_streams(shared, sink_slot, in_dev, out_dev) {
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
}

/// Build the per-call encoder sink whose `send` closure prepends the audio prefix and
/// transmits to `peer`. Factored out so device-change reopen can rebuild it.
fn build_call_sink(
    shared: &Arc<Shared>,
    in_rate: u32,
    sock: UdpSocket,
    peer: SocketAddr,
) -> Result<CallSink> {
    let prefix = AUDIO_PREFIX.to_vec();
    let mut pkt = Vec::with_capacity(prefix.len() + 400);
    let send = Box::new(move |bytes: &[u8]| {
        pkt.clear();
        pkt.extend_from_slice(&prefix);
        pkt.extend_from_slice(bytes);
        let _ = sock.send_to(&pkt, peer);
    });
    CallSink::new(in_rate, shared.controls.clone(), send)
}

/// Apply a device change by reopening the streams (Phase 2 is not seamless). No-op when
/// streams aren't open (selection then applies on the next StartTest/StartCall).
#[allow(clippy::too_many_arguments)]
fn reopen(
    shared: &Arc<Shared>,
    sink_slot: &Arc<Mutex<Option<CallSink>>>,
    streams: &mut Option<AudioStreams>,
    in_rate: &mut u32,
    in_dev: &Option<String>,
    out_dev: &Option<String>,
    call: &Option<(UdpSocket, SocketAddr)>,
    on_event: &Arc<dyn Fn(AudioEvent) + Send + Sync>,
) {
    if streams.is_none() {
        return;
    }
    // Tear down: clearing the sink stops sends; dropping streams drops pcm_tx so the
    // pump thread exits; clear jitter since the output rate may change.
    *sink_slot.lock().unwrap() = None;
    shared.active.store(false, Ordering::Relaxed);
    *streams = None;
    shared.jitter.lock().unwrap().clear();

    if !ensure_open(shared, sink_slot, streams, in_rate, in_dev, out_dev, on_event) {
        return; // open failed; Unavailable already emitted
    }

    // Rebuild the call sink for the new input rate if a call is active.
    if let Some((sock, peer)) = call.as_ref() {
        match sock.try_clone() {
            Ok(clone) => match build_call_sink(shared, *in_rate, clone, *peer) {
                Ok(sink) => *sink_slot.lock().unwrap() = Some(sink),
                Err(e) => on_event(AudioEvent::Unavailable(e.to_string())),
            },
            Err(e) => on_event(AudioEvent::Unavailable(format!("socket clone failed: {e}"))),
        }
    }
}

fn engine_loop(
    cmd_rx: Receiver<AudioCmd>,
    shared: Arc<Shared>,
    on_event: Arc<dyn Fn(AudioEvent) + Send + Sync>,
    mut in_dev: Option<String>,
    mut out_dev: Option<String>,
) {
    let mut streams: Option<AudioStreams> = None;
    let mut in_rate: u32 = SAMPLE_RATE;
    let mut call: Option<(UdpSocket, SocketAddr)> = None;
    let sink_slot: Arc<Mutex<Option<CallSink>>> = Arc::new(Mutex::new(None));

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            AudioCmd::StartTest => {
                ensure_open(
                    &shared, &sink_slot, &mut streams, &mut in_rate, &in_dev, &out_dev, &on_event,
                );
            }
            AudioCmd::StopTest => {
                *sink_slot.lock().unwrap() = None;
                shared.tone_active.store(false, Ordering::Relaxed);
                shared.active.store(false, Ordering::Relaxed);
                streams = None;
            }
            AudioCmd::Tone(on) => {
                shared.tone_active.store(on, Ordering::Relaxed);
            }
            AudioCmd::StartCall { sock, peer } => {
                shared.tone_active.store(false, Ordering::Relaxed);
                if !ensure_open(
                    &shared, &sink_slot, &mut streams, &mut in_rate, &in_dev, &out_dev, &on_event,
                ) {
                    continue;
                }
                call = sock.try_clone().ok().map(|s| (s, peer));
                match build_call_sink(&shared, in_rate, sock, peer) {
                    Ok(sink) => {
                        *sink_slot.lock().unwrap() = Some(sink);
                        on_event(AudioEvent::State(shared.controls.snapshot()));
                    }
                    Err(e) => on_event(AudioEvent::Unavailable(e.to_string())),
                }
            }
            AudioCmd::EndCall => {
                *sink_slot.lock().unwrap() = None;
                call = None;
            }
            AudioCmd::SetInputDevice(name) => {
                in_dev = name;
                reopen(
                    &shared, &sink_slot, &mut streams, &mut in_rate, &in_dev, &out_dev, &call,
                    &on_event,
                );
            }
            AudioCmd::SetOutputDevice(name) => {
                out_dev = name;
                reopen(
                    &shared, &sink_slot, &mut streams, &mut in_rate, &in_dev, &out_dev, &call,
                    &on_event,
                );
            }
            AudioCmd::ListDevices(reply) => {
                let _ = reply.send(enumerate_devices(&in_dev, &out_dev));
            }
            AudioCmd::Shutdown => {
                *sink_slot.lock().unwrap() = None;
                shared.active.store(false, Ordering::Relaxed);
                drop(streams.take());
                return;
            }
        }
    }
}
```

- [ ] **Step 6: Add a unit test for `DeviceList` default**

In the existing `#[cfg(test)] mod tests` block, add:
```rust
    #[test]
    fn device_list_default_is_empty() {
        let d = DeviceList::default();
        assert!(d.inputs.is_empty() && d.outputs.is_empty());
        assert!(d.current_input.is_none() && d.current_output.is_none());
    }
```

- [ ] **Step 7: Build + test**

The existing 3 `audio_engine` tests construct `AudioEngineHandle` directly and call `spawn` nowhere in tests, so the `spawn` signature change does not break them. Confirm:
Run: `cargo test --manifest-path src-tauri/Cargo.toml audio_engine::`
Expected: 4 tests pass (3 existing + the new one).
Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: a compile ERROR in `bridge.rs` at the `audio_engine::spawn(...)` call (now needs 3 args) — that's fixed in Task P2-3. The `audio_engine.rs` module itself must compile (run `cargo build` and confirm the only errors are in bridge.rs about `spawn` arity; if there are errors inside audio_engine.rs, fix them). Do NOT commit yet if the crate doesn't build — but this task's audio_engine.rs changes are complete when the only remaining error is the bridge `spawn` arity. Commit after Task P2-3 restores the build. (If you prefer, temporarily pass `, None, None` at the bridge spawn call to keep it green and let P2-3 finalize — but do not add other bridge changes here.)

To keep this task independently committable: apply the minimal bridge edit to pass `None, None` to `spawn` now, build clean, commit, and P2-3 will replace it with the real settings wiring.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/engine/audio_engine.rs src-tauri/src/bridge.rs
git commit -m "feat(audio): device enumeration and selection in the engine"
```

---

## Task P2-2: Settings persistence (`settings.rs`)

**Files:** Create `src-tauri/src/settings.rs`; modify `src-tauri/src/lib.rs`

- [ ] **Step 1: Declare the module**

In `src-tauri/src/lib.rs`, add `mod settings;` near the top (after `mod engine;`). Add `pub use settings::Settings;` so the bridge can `use crate::Settings;` (or it can use `crate::settings::Settings` — either is fine; prefer `pub use`).

- [ ] **Step 2: Write the failing tests**

Create `src-tauri/src/settings.rs` with ONLY the tests first:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        // Unique per test name; avoids needing randomness/time.
        let dir = std::env::temp_dir().join(format!("ramsit-settings-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tmp("roundtrip");
        let s = Settings {
            input_device: Some("Mic A".into()),
            output_device: None,
        };
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_file_is_default() {
        let dir = tmp("missing");
        assert_eq!(Settings::load(&dir), Settings::default());
    }

    #[test]
    fn load_corrupt_json_is_default() {
        let dir = tmp("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(Settings::path(&dir), b"not json{{").unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml settings::`
Expected: FAIL — `Settings` not found.

- [ ] **Step 4: Implement**

Prepend to `src-tauri/src/settings.rs`:
```rust
//! Persisted user preferences (currently just the chosen audio devices). Stored as
//! JSON in the Tauri app-config dir. The bridge owns reads/writes; the audio engine
//! never touches disk.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub input_device: Option<String>,
    #[serde(default)]
    pub output_device: Option<String>,
}

impl Settings {
    /// Path to the settings file within `config_dir`.
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("settings.json")
    }

    /// Load settings, falling back to defaults on a missing or unreadable/corrupt file.
    pub fn load(config_dir: &Path) -> Settings {
        match std::fs::read_to_string(Self::path(config_dir)) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Settings::default(),
        }
    }

    /// Write settings as pretty JSON, creating `config_dir` if needed.
    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir)?;
        let json = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        std::fs::write(Self::path(config_dir), json)
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml settings::`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/settings.rs src-tauri/src/lib.rs
git commit -m "feat(settings): persist chosen audio devices to the app config dir"
```

---

## Task P2-3: Bridge wiring (devices + settings)

**Files:** Modify `src-tauri/src/bridge.rs`

- [ ] **Step 1: Imports + `AppState`**

Add to the imports in `bridge.rs`:
```rust
use crate::audio_engine::{self, AudioEngineHandle, AudioEvent, DeviceList};
use crate::settings::Settings;
use std::path::PathBuf;
use std::sync::Mutex;
```
(Keep existing imports; `DeviceList` and `Settings` are the new names. Ensure `std::path::PathBuf` and `Mutex` are imported.)

Change `AppState`:
```rust
struct AppState {
    stun: SocketAddr,
    cmd_tx: Mutex<Option<Sender<Command>>>,
    audio: AudioEngineHandle,
    settings: Mutex<Settings>,
    config_dir: PathBuf,
}
```

- [ ] **Step 2: Device commands**

Add three commands:
```rust
#[tauri::command]
fn list_audio_devices(state: State<AppState>) -> DeviceList {
    state.audio.list_devices()
}

#[tauri::command]
fn set_input_device(name: Option<String>, state: State<AppState>) {
    {
        let mut s = state.settings.lock().unwrap();
        s.input_device = name.clone();
        if let Err(e) = s.save(&state.config_dir) {
            log::warn!("settings: save failed: {e}");
        }
    }
    state.audio.set_input_device(name);
}

#[tauri::command]
fn set_output_device(name: Option<String>, state: State<AppState>) {
    {
        let mut s = state.settings.lock().unwrap();
        s.output_device = name.clone();
        if let Err(e) = s.save(&state.config_dir) {
            log::warn!("settings: save failed: {e}");
        }
    }
    state.audio.set_output_device(name);
}
```

- [ ] **Step 3: Load settings at setup, pass to `spawn`, store in state**

In `run()`'s `.setup(...)` closure, replace the engine creation + `app.manage(...)` with:
```rust
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let config_dir = app
                .path()
                .app_config_dir()
                .expect("no app config dir");
            let settings = Settings::load(&config_dir);
            let audio = audio_engine::spawn(
                move |ev: AudioEvent| {
                    let _ = app_handle.emit(EVENT_CHANNEL, audio_event_json(&ev));
                },
                settings.input_device.clone(),
                settings.output_device.clone(),
            )
            .expect("failed to start audio engine");
            app.manage(AppState {
                stun,
                cmd_tx: Mutex::new(None),
                audio,
                settings: Mutex::new(settings),
                config_dir,
            });
            Ok(())
        })
```
`app.path()` requires `tauri::Manager` (already imported). If P2-1 left a temporary `spawn(on_event, None, None)` call elsewhere, this replaces it.

- [ ] **Step 4: Register the new commands**

Add `list_audio_devices`, `set_input_device`, `set_output_device` to the `tauri::generate_handler![...]` list (keep all existing).

- [ ] **Step 5: Build + full test suite**

Run: `cargo build --manifest-path src-tauri/Cargo.toml` → clean.
Run: `cargo test --manifest-path src-tauri/Cargo.toml` → all pass (existing + settings + audio_engine).
Resolve any unused-import warnings.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/bridge.rs
git commit -m "feat(audio): device-list and device-selection commands with persistence"
```

---

## Task P2-4: Frontend bindings + `DeviceSelect`

**Files:** Modify `src/engine.ts`; create `src/components/DeviceSelect.tsx`

- [ ] **Step 1: Add the `DeviceList` type and wrappers**

In `src/engine.ts`, add the type (near the top, after `EngineEvent`):
```typescript
export type DeviceList = {
  inputs: string[];
  outputs: string[];
  currentInput: string | null;
  currentOutput: string | null;
};
```
Add to the `engine` object (before `quit`):
```typescript
  listAudioDevices: () => invoke<DeviceList>("list_audio_devices"),
  setInputDevice: (name: string | null) => invoke<void>("set_input_device", { name }),
  setOutputDevice: (name: string | null) => invoke<void>("set_output_device", { name }),
```

- [ ] **Step 2: Implement `DeviceSelect`**

Create `src/components/DeviceSelect.tsx`:
```tsx
import { useEffect, useState } from "react";
import { engine, type DeviceList } from "../engine";

export default function DeviceSelect() {
  const [list, setList] = useState<DeviceList | null>(null);

  async function refresh() {
    setList(await engine.listAudioDevices());
  }

  useEffect(() => {
    refresh();
  }, []);

  if (!list) return null;

  return (
    <div className="device-select">
      <label>
        Input
        <select
          value={list.currentInput ?? ""}
          onChange={async (e) => {
            await engine.setInputDevice(e.target.value || null);
            refresh();
          }}
        >
          <option value="">System default</option>
          {list.inputs.map((d) => (
            <option key={d} value={d}>
              {d}
            </option>
          ))}
        </select>
      </label>
      <label>
        Output
        <select
          value={list.currentOutput ?? ""}
          onChange={async (e) => {
            await engine.setOutputDevice(e.target.value || null);
            refresh();
          }}
        >
          <option value="">System default</option>
          {list.outputs.map((d) => (
            <option key={d} value={d}>
              {d}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}
```

- [ ] **Step 3: Type-check**

Run: `pnpm exec tsc --noEmit` → clean.

- [ ] **Step 4: Commit**

```bash
git add src/engine.ts src/components/DeviceSelect.tsx
git commit -m "feat(ui): device list bindings and DeviceSelect dropdowns"
```

---

## Task P2-5: Mount `DeviceSelect` + styles + verify

**Files:** Modify `src/components/AudioTest.tsx`, `src/screens/Chat.tsx`, `src/styles.css`

- [ ] **Step 1: Mount in the self-test panel**

In `src/components/AudioTest.tsx`, add the import:
```tsx
import DeviceSelect from "./DeviceSelect";
```
Render `<DeviceSelect />` as the first child of the returned `<section className="audio-test">`, before the `.audio-test-controls` div:
```tsx
    <section className="audio-test">
      <DeviceSelect />
      <div className="audio-test-controls">
```

- [ ] **Step 2: Mount in the Chat screen**

In `src/screens/Chat.tsx`, add the import:
```tsx
import DeviceSelect from "../components/DeviceSelect";
```
Render `<DeviceSelect />` at the start of the `<div className="voice">` block (before the status `<span>`):
```tsx
      <div className="voice">
        <DeviceSelect />
        <span className="status">{status}</span>
```

- [ ] **Step 3: Styles**

Append to `src/styles.css`:
```css
.device-select { display: flex; flex-wrap: wrap; gap: 12px; margin: 6px 0; }
.device-select label { display: flex; flex-direction: column; font-size: 12px; gap: 2px; }
.device-select select { max-width: 220px; }
```

- [ ] **Step 4: Type-check + tests + build**

Run: `pnpm exec tsc --noEmit` → clean.
Run: `pnpm test` → reducer + levels tests pass.
Run: `cargo test --manifest-path src-tauri/Cargo.toml` → all pass.

- [ ] **Step 5: Commit**

```bash
git add src/components/AudioTest.tsx src/screens/Chat.tsx src/styles.css
git commit -m "feat(ui): mount device selection on the self-test panel and chat screen"
```

- [ ] **Step 6: Manual verification (user)**

Run `pnpm tauri dev` (or a bundled build). On the Exchange screen the self-test panel now shows **Input** and **Output** dropdowns. Verify: switching the Input device while testing makes the Mic meter respond to the newly selected device; the choice persists after restarting the app; an unplugged saved device falls back to default without crashing.

---

## Self-Review Notes
- **Spec coverage (Phase 2):** enumeration (`list_audio_devices`/`enumerate_devices`) ✓; selection applied by reopening streams (`reopen`) ✓; persistence of device names only, bridge-owned write (`settings.rs` + bridge commands) ✓; missing-device fallback to default (`pick_input`/`pick_output`) ✓; `<DeviceSelect>` on Exchange + Chat ✓. Phase 3 (seamless single-stream hot-swap) intentionally excluded.
- **Type consistency:** `DeviceList` fields are camelCase over the wire (`#[serde(rename_all="camelCase")]`) and the TS `DeviceList` matches (`currentInput`/`currentOutput`). `set_input_device(name: Option<String>)` ↔ `invoke("set_input_device", { name })`. `spawn` now takes `(on_event, input_device, output_device)` — the only caller is the bridge setup.
- **Concurrency:** all `cpal` enumeration + device open + reopen happen on the engine thread (serialized). `list_devices()` blocks the calling command thread up to 2s on the reply channel — acceptable for a UI populate. The retained `call` socket clone is rebuilt on reopen so a mid-call device change keeps transmitting.
- **cpal 0.18.1:** `host.input_devices()`/`output_devices()` return `Result<Iterator<Item=Device>>`; `Device::name()` returns `Result<String>`; `sample_rate` is a plain `u32` (unchanged from Phase 1).
