# Tauri Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace ramsit's ratatui TUI with a Tauri (React + Vite) desktop app, reusing the networking/voice engine unchanged.

**Architecture:** Single Tauri crate. The engine (`net`, `punch`, `proto`, `audio`) moves into `src-tauri/src/engine/`. A thin `bridge.rs` holds the `Command` sender in Tauri managed state, exposes `#[command]`s, and forwards engine `Event`s to the webview as a single tagged `"engine-event"`. React owns the `Screen` state machine (ported from `app.rs`). `main.rs`/`ui.rs`/`app.rs` and the root `Cargo.toml` are deleted.

**Tech Stack:** Rust, Tauri v2, React 18 + TypeScript, Vite, Vitest, pnpm. Engine deps unchanged: `anyhow`, `cpal`, `opus`, `stunclient`, `log`, `env_logger`.

---

## File Structure

```
ramsit/
├── package.json, vite.config.ts, index.html, tsconfig.json, tsconfig.node.json
├── src/                          # React frontend
│   ├── main.tsx                  # React entry; mounts <App/>
│   ├── App.tsx                   # listener wiring + screen dispatch
│   ├── engine.ts                 # typed invoke() wrappers + EngineEvent types
│   ├── reducer.ts                # Screen state machine (port of app.rs)
│   ├── reducer.test.ts           # Vitest unit tests
│   ├── screens/
│   │   ├── Discovering.tsx
│   │   ├── Exchange.tsx
│   │   ├── Punching.tsx
│   │   ├── Chat.tsx
│   │   └── Fatal.tsx
│   └── styles.css
└── src-tauri/
    ├── Cargo.toml, tauri.conf.json, build.rs, capabilities/default.json, icons/
    └── src/
        ├── main.rs               # logger + STUN resolve + Builder + managed state
        ├── bridge.rs             # AppState, start(), commands, event forwarding
        └── engine/
            ├── mod.rs            # re-exports net/punch/proto/audio
            ├── net.rs            # moved, Command enum edited (Task 4)
            ├── punch.rs          # moved unchanged
            ├── proto.rs          # moved unchanged
            └── audio.rs          # moved, set_*_volume added (Task 3)
```

---

## Task 1: Scaffold the frontend toolchain

**Files:**
- Create: `package.json`, `vite.config.ts`, `index.html`, `tsconfig.json`, `tsconfig.node.json`, `src/main.tsx`, `src/App.tsx`, `src/styles.css`
- Create: `.gitignore` additions

- [ ] **Step 1: Create `package.json`**

```json
{
  "name": "ramsit",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc && vite build",
    "preview": "vite preview",
    "test": "vitest run",
    "tauri": "tauri"
  },
  "dependencies": {
    "@tauri-apps/api": "^2",
    "react": "^18.3.1",
    "react-dom": "^18.3.1"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2",
    "@types/react": "^18.3.12",
    "@types/react-dom": "^18.3.1",
    "@vitejs/plugin-react": "^4.3.4",
    "typescript": "^5.6.3",
    "vite": "^6.0.3",
    "vitest": "^2.1.8"
  }
}
```

- [ ] **Step 2: Create `vite.config.ts`**

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed port and ignores src-tauri during HMR.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2021", outDir: "dist" },
});
```

- [ ] **Step 3: Create `index.html`**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>ramsit</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

- [ ] **Step 4: Create `tsconfig.json`**

```json
{
  "compilerOptions": {
    "target": "ES2021",
    "useDefineForClassFields": true,
    "lib": ["ES2021", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "skipLibCheck": true,
    "moduleResolution": "bundler",
    "resolveJsonModule": true,
    "isolatedModules": true,
    "noEmit": true,
    "jsx": "react-jsx",
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true
  },
  "include": ["src"],
  "references": [{ "path": "./tsconfig.node.json" }]
}
```

- [ ] **Step 5: Create `tsconfig.node.json`**

```json
{
  "compilerOptions": {
    "composite": true,
    "skipLibCheck": true,
    "module": "ESNext",
    "moduleResolution": "bundler",
    "allowSyntheticDefaultImports": true
  },
  "include": ["vite.config.ts"]
}
```

- [ ] **Step 6: Create a placeholder `src/styles.css`**

```css
:root { color-scheme: dark; font-family: system-ui, sans-serif; }
body { margin: 0; }
```

- [ ] **Step 7: Create `src/main.tsx`**

```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
```

- [ ] **Step 8: Create a temporary `src/App.tsx` placeholder**

```tsx
export default function App() {
  return <main>ramsit</main>;
}
```

- [ ] **Step 9: Append build artifacts to `.gitignore`**

Add these lines to `.gitignore` (create the file if absent):

```
node_modules
dist
```

- [ ] **Step 10: Install and verify the frontend builds**

Run: `pnpm install && pnpm build`
Expected: `tsc` passes and Vite writes `dist/index.html`. No errors.

- [ ] **Step 11: Commit**

```bash
git add package.json vite.config.ts index.html tsconfig.json tsconfig.node.json src/main.tsx src/App.tsx src/styles.css .gitignore
git commit -m "chore: scaffold React + Vite frontend toolchain"
```

---

## Task 2: Generate the Tauri shell and relocate the engine

**Files:**
- Create (via CLI): `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/build.rs`, `src-tauri/capabilities/default.json`, `src-tauri/icons/*`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`
- Move: `src/{net,punch,proto,audio}.rs` → `src-tauri/src/engine/`
- Create: `src-tauri/src/engine/mod.rs`
- Delete: `Cargo.toml` (root), `src/app.rs`, `src/ui.rs`, `src/main.rs` (root Rust), `Cargo.lock` (root, regenerated under src-tauri)

- [ ] **Step 1: Generate the Tauri scaffold non-interactively**

Run:

```bash
pnpm tauri init --ci \
  --app-name ramsit \
  --window-title ramsit \
  --frontend-dist ../dist \
  --dev-url http://localhost:1420 \
  --before-dev-command "pnpm dev" \
  --before-build-command "pnpm build"
```

Expected: creates `src-tauri/` with `Cargo.toml`, `tauri.conf.json`, `build.rs`, `icons/`, `capabilities/default.json`, and a default `src/main.rs` + `src/lib.rs`. This gives us working icons and capabilities without hand-authoring them.

- [ ] **Step 2: Move the engine files into the Tauri crate**

Run:

```bash
mkdir -p src-tauri/src/engine
git mv src/net.rs src-tauri/src/engine/net.rs
git mv src/punch.rs src-tauri/src/engine/punch.rs
git mv src/proto.rs src-tauri/src/engine/proto.rs
git mv src/audio.rs src-tauri/src/engine/audio.rs
```

- [ ] **Step 3: Delete the obsolete TUI Rust and root manifest**

Run:

```bash
git rm src/app.rs src/ui.rs src/main.rs Cargo.toml
git rm --cached Cargo.lock 2>/dev/null || true
rm -f Cargo.lock
```

(The `src/` directory now holds only the React frontend created in Task 1.)

- [ ] **Step 4: Create `src-tauri/src/engine/mod.rs`**

The engine modules currently reference each other as `crate::net`, `crate::audio`, etc. Re-export them at the crate root (Step 7) so those paths keep resolving.

```rust
pub mod audio;
pub mod net;
pub mod proto;
pub mod punch;
```

- [ ] **Step 5: Fix intra-engine import paths**

The moved files use `crate::audio`, `crate::net`, `crate::proto`, `crate::punch`. After Step 7 these are re-exported at the crate root, so **no path edits are needed** — `crate::net` still resolves. Verify by grepping:

Run: `grep -rn "crate::" src-tauri/src/engine/`
Expected: references to `crate::audio`, `crate::net`, `crate::proto`, `crate::punch` only. Leave them as-is.

- [ ] **Step 6: Set `src-tauri/Cargo.toml` dependencies**

Replace the `[dependencies]` section so it includes both Tauri and the engine crates (keep the `[package]`, `[lib]`, and `[build-dependencies]` blocks the scaffold generated):

```toml
[dependencies]
tauri = { version = "2", features = [] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
cpal = "0.18"
env_logger = "0.11"
log = "0.4"
opus = "0.3"
stunclient = "0.4"
```

- [ ] **Step 7: Replace `src-tauri/src/lib.rs` with the module wiring**

This declares the engine and bridge modules and re-exports engine submodules at the crate root so `crate::net` etc. resolve. The Tauri `run()` entry is filled in by Task 5; for now keep a minimal builder so the crate compiles.

```rust
mod bridge;
mod engine;

pub use engine::{audio, net, proto, punch};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    bridge::run();
}
```

- [ ] **Step 8: Create a minimal `src-tauri/src/bridge.rs` so the crate compiles**

(Fully implemented in Task 5; this stub keeps Tasks 2–4 building and testable.)

```rust
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 9: Ensure `src-tauri/src/main.rs` calls into the lib**

The scaffold generates this; confirm it reads:

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ramsit_lib::run();
}
```

(The lib crate name comes from `[lib] name` in `Cargo.toml`, typically `ramsit_lib`. Match whatever the scaffold generated.)

- [ ] **Step 10: Verify the engine still compiles and its tests pass**

Run: `cd src-tauri && cargo test`
Expected: PASS — all existing `net`, `proto`, `audio` tests run green. No `app`/`ui` tests (those files are deleted).

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "feat: scaffold Tauri shell and relocate engine into src-tauri"
```

---

## Task 3: Add absolute volume setters to the audio engine

**Files:**
- Modify: `src-tauri/src/engine/audio.rs` (add methods near `adjust_*_volume`, lines ~95-109)
- Test: `src-tauri/src/engine/audio.rs` (add to existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `audio.rs`. (If the module has no existing `AudioHandle` constructor for tests, the methods are pure over `Controls`, so test via a fresh handle helper is unavailable — instead test `clamp_vol` composition through a small unit. Use this direct test of the clamp behavior the setters rely on, plus an integration assertion on `set_*` once implemented.)

```rust
#[test]
fn set_volume_clamps_into_range() {
    // set_*_volume stores clamp_vol(pct as i32); verify the clamp contract.
    assert_eq!(clamp_vol(250), VOL_MAX as u32);
    assert_eq!(clamp_vol(-5), VOL_MIN as u32);
    assert_eq!(clamp_vol(80), 80);
}
```

- [ ] **Step 2: Run it to confirm it compiles and passes against existing `clamp_vol`**

Run: `cd src-tauri && cargo test set_volume_clamps_into_range`
Expected: PASS (this guards the clamp the setters reuse).

- [ ] **Step 3: Add the setter methods to `impl AudioHandle`**

Insert after `adjust_output_volume` (after line ~109):

```rust
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
```

- [ ] **Step 4: Verify it compiles**

Run: `cd src-tauri && cargo test`
Expected: PASS. (`adjust_*_volume` may now be unused — that is resolved in Task 4.)

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/engine/audio.rs
git commit -m "feat(audio): add absolute set_input/output_volume setters"
```

---

## Task 4: Switch the Command enum to absolute volume setters

**Files:**
- Modify: `src-tauri/src/engine/net.rs` — `Command` enum (lines ~16-23) and the `session` match arms (lines ~180-189)
- Modify: `src-tauri/src/engine/audio.rs` — remove now-unused `adjust_*_volume`

- [ ] **Step 1: Replace the volume variants in `Command`**

Change lines 20-21 of `net.rs` from:

```rust
    AdjustInputVolume(i8),
    AdjustOutputVolume(i8),
```

to:

```rust
    SetInputVolume(u8),
    SetOutputVolume(u8),
```

- [ ] **Step 2: Update the `session` loop match arms**

Replace the two `AdjustInputVolume`/`AdjustOutputVolume` arms (lines ~180-189) with:

```rust
                Ok(Command::SetInputVolume(pct)) => {
                    if let Some(a) = &audio {
                        let _ = events.send(Event::AudioState(a.set_input_volume(pct)));
                    }
                }
                Ok(Command::SetOutputVolume(pct)) => {
                    if let Some(a) = &audio {
                        let _ = events.send(Event::AudioState(a.set_output_volume(pct)));
                    }
                }
```

- [ ] **Step 3: Remove the now-unused `adjust_*_volume` methods**

Delete `adjust_input_volume` and `adjust_output_volume` from `impl AudioHandle` in `audio.rs` (lines ~95-109). The TUI was their only caller.

- [ ] **Step 4: Verify the crate compiles with no warnings about the removed methods**

Run: `cd src-tauri && cargo test`
Expected: PASS, no "method never used" warnings for `adjust_*`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/engine/net.rs src-tauri/src/engine/audio.rs
git commit -m "feat(net): replace adjust-volume deltas with absolute set commands"
```

---

## Task 5: Implement the Tauri bridge

**Files:**
- Rewrite: `src-tauri/src/bridge.rs`
- Modify: `src-tauri/src/main.rs` is unchanged; `lib.rs` already calls `bridge::run()`

The bridge owns: STUN resolution + logger init, managed `AppState` holding the
`Command` sender, a `start()` command that spawns the engine **after** the
frontend has attached its listener (Tauri events are not buffered, so starting
earlier would drop `Discovered`), the per-action commands, and a thread that
forwards every `Event` to the `"engine-event"` channel.

- [ ] **Step 1: Write the failing test for the Event→JSON mapping**

Add at the bottom of `bridge.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::event_to_json;
    use crate::audio::AudioState;
    use crate::net::Event;

    #[test]
    fn maps_audio_state_with_camel_case_keys() {
        let v = event_to_json(&Event::AudioState(AudioState {
            muted: true,
            input_vol: 80,
            output_vol: 120,
        }));
        assert_eq!(v["type"], "audioState");
        assert_eq!(v["muted"], true);
        assert_eq!(v["inputVol"], 80);
        assert_eq!(v["outputVol"], 120);
    }

    #[test]
    fn maps_incoming_text() {
        let v = event_to_json(&Event::Incoming("hi".into()));
        assert_eq!(v["type"], "incoming");
        assert_eq!(v["text"], "hi");
    }

    #[test]
    fn maps_discovered_addr_as_string() {
        let addr = "203.0.113.5:54213".parse().unwrap();
        let v = event_to_json(&Event::Discovered(addr));
        assert_eq!(v["type"], "discovered");
        assert_eq!(v["code"], "203.0.113.5:54213");
    }
}
```

- [ ] **Step 2: Run it to confirm it fails to compile (no `event_to_json` yet)**

Run: `cd src-tauri && cargo test event_to_json 2>&1 | head`
Expected: FAIL — `cannot find function event_to_json`.

- [ ] **Step 3: Implement `bridge.rs`**

```rust
use crate::net::{self, Command, Event};
use crate::proto::parse_code;
use serde_json::{json, Value};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

const DEFAULT_STUN: &str = "stun.l.google.com:19302";
const EVENT_CHANNEL: &str = "engine-event";

/// Shared state: the resolved STUN server and the live command sender (None
/// until `start` spawns the engine).
struct AppState {
    stun: SocketAddr,
    cmd_tx: Mutex<Option<Sender<Command>>>,
}

/// Map an engine `Event` to the tagged JSON the frontend listens for.
fn event_to_json(ev: &Event) -> Value {
    match ev {
        Event::Discovered(addr) => json!({ "type": "discovered", "code": addr.to_string() }),
        Event::Connected(addr) => json!({ "type": "connected", "peer": addr.to_string() }),
        Event::Incoming(s) => json!({ "type": "incoming", "text": s }),
        Event::AudioState(st) => json!({
            "type": "audioState",
            "muted": st.muted,
            "inputVol": st.input_vol,
            "outputVol": st.output_vol,
        }),
        Event::AudioUnavailable(s) => json!({ "type": "audioUnavailable", "reason": s }),
        Event::PeerLeft => json!({ "type": "peerLeft" }),
        Event::Fatal(s) => json!({ "type": "fatal", "message": s }),
    }
}

/// Spawn the engine once. Called by the frontend after it attaches its listener.
#[tauri::command]
fn start(app: AppHandle, state: State<AppState>) {
    let mut guard = state.cmd_tx.lock().unwrap();
    if guard.is_some() {
        return; // already started
    }
    let (_handle, cmd_tx, evt_rx) = net::spawn(state.stun);
    *guard = Some(cmd_tx);
    std::thread::spawn(move || {
        while let Ok(ev) = evt_rx.recv() {
            let _ = app.emit(EVENT_CHANNEL, event_to_json(&ev));
        }
    });
}

fn send(state: &State<AppState>, cmd: Command) {
    if let Some(tx) = state.cmd_tx.lock().unwrap().as_ref() {
        let _ = tx.send(cmd);
    }
}

#[tauri::command]
fn submit_peer_code(code: String, state: State<AppState>) -> Result<(), String> {
    let addr = parse_code(&code).map_err(|e| e.to_string())?;
    send(&state, Command::PeerCode(addr));
    Ok(())
}

#[tauri::command]
fn send_message(text: String, state: State<AppState>) {
    send(&state, Command::Send(text));
}

#[tauri::command]
fn toggle_mute(state: State<AppState>) {
    send(&state, Command::ToggleMute);
}

#[tauri::command]
fn set_input_volume(pct: u8, state: State<AppState>) {
    send(&state, Command::SetInputVolume(pct));
}

#[tauri::command]
fn set_output_volume(pct: u8, state: State<AppState>) {
    send(&state, Command::SetOutputVolume(pct));
}

#[tauri::command]
fn quit(state: State<AppState>) {
    send(&state, Command::Quit);
}

/// Resolve a STUN host:port to its first IPv4 socket address.
fn resolve_stun(s: &str) -> SocketAddr {
    s.to_socket_addrs()
        .ok()
        .and_then(|mut it| it.find(|a| a.is_ipv4()))
        .unwrap_or_else(|| panic!("could not resolve STUN server '{s}'"))
}

pub fn run() {
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .format_timestamp_millis()
    .format_target(false)
    .try_init();

    let stun = resolve_stun(DEFAULT_STUN);
    log::info!("stun: using server {DEFAULT_STUN} ({stun})");

    tauri::Builder::default()
        .manage(AppState {
            stun,
            cmd_tx: Mutex::new(None),
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                if let Some(state) = window.try_state::<AppState>() {
                    if let Some(tx) = state.cmd_tx.lock().unwrap().as_ref() {
                        let _ = tx.send(Command::Quit);
                    }
                }
                // Give the worker a beat to flush a best-effort BYE.
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
            quit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    // (test bodies from Step 1)
}
```

Note: paste the Step 1 test bodies into the `#[cfg(test)] mod tests` block at the bottom.

- [ ] **Step 4: Run the bridge tests**

Run: `cd src-tauri && cargo test event_to_json`
Expected: PASS — all three mapping tests green.

- [ ] **Step 5: Verify the whole crate compiles**

Run: `cd src-tauri && cargo build`
Expected: builds clean (logger goes to stderr now; `ramsit.log` file target is dropped in favor of the default stderr logger, which the webview process captures).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/bridge.rs src-tauri/src/lib.rs
git commit -m "feat(bridge): wire engine to Tauri commands and events"
```

---

## Task 6: Frontend engine client (typed invoke + event types)

**Files:**
- Create: `src/engine.ts`

- [ ] **Step 1: Create `src/engine.ts`**

```ts
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type EngineEvent =
  | { type: "discovered"; code: string }
  | { type: "connected"; peer: string }
  | { type: "incoming"; text: string }
  | { type: "audioState"; muted: boolean; inputVol: number; outputVol: number }
  | { type: "audioUnavailable"; reason: string }
  | { type: "peerLeft" }
  | { type: "fatal"; message: string };

export function onEngineEvent(cb: (e: EngineEvent) => void): Promise<UnlistenFn> {
  return listen<EngineEvent>("engine-event", (ev) => cb(ev.payload));
}

export const engine = {
  start: () => invoke<void>("start"),
  submitPeerCode: (code: string) => invoke<void>("submit_peer_code", { code }),
  sendMessage: (text: string) => invoke<void>("send_message", { text }),
  toggleMute: () => invoke<void>("toggle_mute"),
  setInputVolume: (pct: number) => invoke<void>("set_input_volume", { pct }),
  setOutputVolume: (pct: number) => invoke<void>("set_output_volume", { pct }),
  quit: () => invoke<void>("quit"),
};
```

- [ ] **Step 2: Verify it type-checks**

Run: `pnpm exec tsc --noEmit`
Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src/engine.ts
git commit -m "feat(ui): typed Tauri engine client and event types"
```

---

## Task 7: Frontend screen reducer (port of app.rs) with tests

**Files:**
- Create: `src/reducer.ts`
- Test: `src/reducer.test.ts`

The reducer mirrors `app.rs::apply`. DOM auto-scroll replaces the TUI's scroll-offset state (handled in the Chat component via a ref), so the reducer carries no `scroll` field.

- [ ] **Step 1: Write the failing tests**

```ts
import { describe, it, expect } from "vitest";
import { initialState, reduce } from "./reducer";

describe("reducer", () => {
  it("discovered moves to exchange", () => {
    const s = reduce(initialState, { type: "discovered", code: "1.2.3.4:5" });
    expect(s.kind).toBe("exchange");
    if (s.kind === "exchange") expect(s.myCode).toBe("1.2.3.4:5");
  });

  it("connected moves to chat", () => {
    const s = reduce(initialState, { type: "connected", peer: "1.2.3.4:5" });
    expect(s.kind).toBe("chat");
  });

  it("incoming appends a peer message in chat", () => {
    let s = reduce(initialState, { type: "connected", peer: "p" });
    s = reduce(s, { type: "incoming", text: "yo" });
    if (s.kind === "chat") expect(s.messages).toEqual(["peer> yo"]);
    else throw new Error("expected chat");
  });

  it("local echo appends a you message", () => {
    let s = reduce(initialState, { type: "connected", peer: "p" });
    s = reduce(s, { type: "sent", text: "hello" });
    if (s.kind === "chat") expect(s.messages).toEqual(["you> hello"]);
    else throw new Error("expected chat");
  });

  it("audioState updates the widget mirror and marks voice live", () => {
    let s = reduce(initialState, { type: "connected", peer: "p" });
    s = reduce(s, { type: "audioState", muted: true, inputVol: 80, outputVol: 120 });
    if (s.kind === "chat") {
      expect(s.muted).toBe(true);
      expect(s.voice).toBe(true);
      expect([s.inputVol, s.outputVol]).toEqual([80, 120]);
    } else throw new Error("expected chat");
  });

  it("peerLeft marks disconnected and notes it", () => {
    let s = reduce(initialState, { type: "connected", peer: "p" });
    s = reduce(s, { type: "peerLeft" });
    if (s.kind === "chat") {
      expect(s.connected).toBe(false);
      expect(s.messages.some((m) => m.includes("disconnected"))).toBe(true);
    } else throw new Error("expected chat");
  });

  it("audioUnavailable clears voice and notes it", () => {
    let s = reduce(initialState, { type: "connected", peer: "p" });
    s = reduce(s, { type: "audioUnavailable", reason: "no mic" });
    if (s.kind === "chat") {
      expect(s.voice).toBe(false);
      expect(s.messages.some((m) => m.includes("voice unavailable"))).toBe(true);
    } else throw new Error("expected chat");
  });

  it("fatal transitions to fatal screen from anywhere", () => {
    const s = reduce(initialState, { type: "fatal", message: "boom" });
    expect(s.kind).toBe("fatal");
  });
});
```

- [ ] **Step 2: Run to confirm failure**

Run: `pnpm test`
Expected: FAIL — cannot resolve `./reducer`.

- [ ] **Step 3: Implement `src/reducer.ts`**

```ts
import type { EngineEvent } from "./engine";

export type Screen =
  | { kind: "discovering" }
  | { kind: "exchange"; myCode: string }
  | { kind: "punching"; peer: string }
  | {
      kind: "chat";
      peer: string;
      messages: string[];
      connected: boolean;
      muted: boolean;
      inputVol: number;
      outputVol: number;
      voice: boolean;
    }
  | { kind: "fatal"; message: string };

/** Engine events plus UI-local actions the reducer also folds in. */
export type Action = EngineEvent | { type: "sent"; text: string };

export const initialState: Screen = { kind: "discovering" };

export function reduce(state: Screen, action: Action): Screen {
  switch (action.type) {
    case "discovered":
      return state.kind === "discovering"
        ? { kind: "exchange", myCode: action.code }
        : state;
    case "connected":
      return {
        kind: "chat",
        peer: action.peer,
        messages: [],
        connected: true,
        muted: false,
        inputVol: 100,
        outputVol: 100,
        voice: false,
      };
    case "incoming":
      return state.kind === "chat"
        ? { ...state, messages: [...state.messages, `peer> ${action.text}`] }
        : state;
    case "sent":
      return state.kind === "chat"
        ? { ...state, messages: [...state.messages, `you> ${action.text}`] }
        : state;
    case "audioState":
      return state.kind === "chat"
        ? {
            ...state,
            muted: action.muted,
            inputVol: action.inputVol,
            outputVol: action.outputVol,
            voice: true,
          }
        : state;
    case "audioUnavailable":
      return state.kind === "chat"
        ? {
            ...state,
            voice: false,
            messages: [...state.messages, `* voice unavailable: ${action.reason} *`],
          }
        : state;
    case "peerLeft":
      return state.kind === "chat"
        ? {
            ...state,
            connected: false,
            messages: [...state.messages, "* peer disconnected *"],
          }
        : state;
    case "fatal":
      return { kind: "fatal", message: action.message };
    default:
      return state;
  }
}
```

Note: the `punching` screen is entered by the Exchange component (on successful `submitPeerCode`), not by an engine event — see Task 8.

- [ ] **Step 4: Run the tests**

Run: `pnpm test`
Expected: PASS — all reducer tests green.

- [ ] **Step 5: Commit**

```bash
git add src/reducer.ts src/reducer.test.ts
git commit -m "feat(ui): screen reducer ported from app.rs with tests"
```

---

## Task 8: Screen components and App wiring

**Files:**
- Create: `src/screens/Discovering.tsx`, `Exchange.tsx`, `Punching.tsx`, `Chat.tsx`, `Fatal.tsx`
- Rewrite: `src/App.tsx`

- [ ] **Step 1: Create `src/screens/Discovering.tsx`**

```tsx
export default function Discovering() {
  return (
    <main className="center">
      <p>Discovering your public code…</p>
    </main>
  );
}
```

- [ ] **Step 2: Create `src/screens/Exchange.tsx`**

```tsx
import { useState } from "react";
import { engine } from "../engine";

export default function Exchange({
  myCode,
  onPunching,
}: {
  myCode: string;
  onPunching: (peer: string) => void;
}) {
  const [input, setInput] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    try {
      await engine.submitPeerCode(input);
      onPunching(input.trim());
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <main className="center">
      <p>
        Your code: <code>{myCode}</code>{" "}
        <button onClick={() => navigator.clipboard.writeText(myCode)}>copy</button>
      </p>
      <form onSubmit={submit}>
        <input
          autoFocus
          placeholder="Peer code (1.2.3.4:5678)"
          value={input}
          onChange={(e) => {
            setInput(e.target.value);
            setError(null);
          }}
        />
        <button type="submit">Connect</button>
      </form>
      {error && <p className="error">{error}</p>}
    </main>
  );
}
```

- [ ] **Step 3: Create `src/screens/Punching.tsx`**

```tsx
export default function Punching({ peer }: { peer: string }) {
  return (
    <main className="center">
      <p>Connecting to {peer}…</p>
    </main>
  );
}
```

- [ ] **Step 4: Create `src/screens/Chat.tsx`**

```tsx
import { useEffect, useRef, useState } from "react";
import { engine } from "../engine";
import type { Screen } from "../reducer";

type ChatState = Extract<Screen, { kind: "chat" }>;

export default function Chat({
  state,
  onSent,
}: {
  state: ChatState;
  onSent: (text: string) => void;
}) {
  const [input, setInput] = useState("");
  const logRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to newest on every message change (replaces the TUI scroll logic).
  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [state.messages.length]);

  async function send(e: React.FormEvent) {
    e.preventDefault();
    const text = input.trim();
    if (!text) return;
    onSent(text);
    setInput("");
    await engine.sendMessage(text);
  }

  const status = !state.voice
    ? "[no voice]"
    : state.muted
      ? "[MUTED]"
      : "[LIVE]";

  return (
    <main className="chat">
      <header>
        <span>peer {state.peer}</span>
        <span className={state.connected ? "ok" : "bad"}>
          {state.connected ? "connected" : "disconnected"}
        </span>
      </header>

      <div className="log" ref={logRef}>
        {state.messages.map((m, i) => (
          <div key={i} className="line">
            {m}
          </div>
        ))}
      </div>

      <div className="voice">
        <span className="status">{status}</span>
        <button disabled={!state.voice} onClick={() => engine.toggleMute()}>
          {state.muted ? "Unmute" : "Mute"}
        </button>
        <label>
          Mic {state.inputVol}%
          <input
            type="range"
            min={0}
            max={200}
            value={state.inputVol}
            disabled={!state.voice}
            onChange={(e) => engine.setInputVolume(Number(e.target.value))}
          />
        </label>
        <label>
          Speaker {state.outputVol}%
          <input
            type="range"
            min={0}
            max={200}
            value={state.outputVol}
            disabled={!state.voice}
            onChange={(e) => engine.setOutputVolume(Number(e.target.value))}
          />
        </label>
      </div>

      <form onSubmit={send}>
        <input
          autoFocus
          placeholder="Message"
          value={input}
          onChange={(e) => setInput(e.target.value)}
        />
        <button type="submit">Send</button>
      </form>
    </main>
  );
}
```

- [ ] **Step 5: Create `src/screens/Fatal.tsx`**

```tsx
export default function Fatal({ message }: { message: string }) {
  return (
    <main className="center">
      <p className="error">Fatal: {message}</p>
    </main>
  );
}
```

- [ ] **Step 6: Rewrite `src/App.tsx`**

`App` attaches the engine listener, then calls `engine.start()` (order matters — events are not buffered). It holds the reducer state and a manual `punching` override (entered by Exchange).

```tsx
import { useEffect, useReducer, useState } from "react";
import { engine, onEngineEvent } from "./engine";
import { initialState, reduce, type Action, type Screen } from "./reducer";
import Discovering from "./screens/Discovering";
import Exchange from "./screens/Exchange";
import Punching from "./screens/Punching";
import Chat from "./screens/Chat";
import Fatal from "./screens/Fatal";

export default function App() {
  const [state, dispatch] = useReducer(reduce, initialState);
  // Punching is a UI-only transition between Exchange and the Connected event.
  const [punching, setPunching] = useState<string | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    onEngineEvent((e) => dispatch(e as Action)).then((fn) => {
      unlisten = fn;
      engine.start(); // start AFTER the listener is attached
    });
    return () => unlisten?.();
  }, []);

  // A real Connected event supersedes the manual punching override.
  const screen: Screen =
    punching && state.kind === "exchange"
      ? { kind: "punching", peer: punching }
      : state;

  switch (screen.kind) {
    case "discovering":
      return <Discovering />;
    case "exchange":
      return <Exchange myCode={screen.myCode} onPunching={setPunching} />;
    case "punching":
      return <Punching peer={screen.peer} />;
    case "chat":
      return (
        <Chat state={screen} onSent={(text) => dispatch({ type: "sent", text })} />
      );
    case "fatal":
      return <Fatal message={screen.message} />;
  }
}
```

- [ ] **Step 7: Type-check and build the frontend**

Run: `pnpm exec tsc --noEmit && pnpm build`
Expected: no type errors; Vite build succeeds.

- [ ] **Step 8: Commit**

```bash
git add src/screens src/App.tsx
git commit -m "feat(ui): screen components and App wiring"
```

---

## Task 9: Styling

**Files:**
- Rewrite: `src/styles.css`

- [ ] **Step 1: Replace `src/styles.css`**

```css
:root {
  color-scheme: dark;
  font-family: system-ui, sans-serif;
  --bg: #1b1b1f;
  --fg: #e6e6e6;
  --accent: #6ad;
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg); }

.center {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 0.75rem;
  height: 100vh;
  padding: 1rem;
}
.error { color: #f77; }
.ok { color: #7d7; }
.bad { color: #f77; }

input, button {
  font: inherit;
  padding: 0.4rem 0.6rem;
  border-radius: 6px;
  border: 1px solid #444;
  background: #2a2a30;
  color: var(--fg);
}
button { cursor: pointer; }

.chat {
  display: flex;
  flex-direction: column;
  height: 100vh;
}
.chat header {
  display: flex;
  justify-content: space-between;
  padding: 0.5rem 0.75rem;
  border-bottom: 1px solid #333;
  font-size: 0.85rem;
}
.log {
  flex: 1;
  overflow-y: auto;
  padding: 0.75rem;
  white-space: pre-wrap;
  word-break: break-word;
}
.line { padding: 0.1rem 0; }
.voice {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  padding: 0.5rem 0.75rem;
  border-top: 1px solid #333;
  flex-wrap: wrap;
}
.voice .status { font-variant-numeric: tabular-nums; min-width: 4.5rem; }
.voice label { display: flex; align-items: center; gap: 0.35rem; font-size: 0.8rem; }
.chat form {
  display: flex;
  gap: 0.5rem;
  padding: 0.75rem;
  border-top: 1px solid #333;
}
.chat form input { flex: 1; }
```

- [ ] **Step 2: Build to confirm CSS is picked up**

Run: `pnpm build`
Expected: succeeds.

- [ ] **Step 3: Commit**

```bash
git add src/styles.css
git commit -m "feat(ui): style the chat and exchange screens"
```

---

## Task 10: Documentation and final verification

**Files:**
- Rewrite: `README.md` (Build/Use sections)

- [ ] **Step 1: Update `README.md` Build and Use sections**

Replace the "## Build" and "## Use" sections with:

```markdown
## Build

Voice links the system Opus library, so install it first:

- macOS: `brew install opus pkg-config`
- Debian/Ubuntu: `sudo apt install libopus-dev pkg-config`
- Fedora: `sudo dnf install opus-devel pkgconf-pkg-config`

Then install JS deps and run the desktop app in dev:

    pnpm install
    pnpm tauri dev

Build a release bundle with:

    pnpm tauri build

## Use

On **both** machines run `pnpm tauri dev` (or launch the built app). A window
opens and shows `Your code: 203.0.113.5:54213`. Click **copy**, send your code to
the other person (Signal/SMS/whatever), paste theirs into the **Peer code** field
and click **Connect**. Both sides should connect within ~60s of each other. Once
connected, type messages and press Enter to send.

Voice goes live automatically (Opus, 48 kHz mono, system default devices). Use
the **Mute** button and the **Mic**/**Speaker** sliders (0–200%) in the chat
screen. The status shows `[LIVE]`/`[MUTED]`/`[no voice]`.
```

Keep the existing libopus note consistent (it now appears under Build) and leave the STUN/LAN sections intact. Remove the TUI-specific key-table for voice controls and the `cargo run` / `--stun` flag references that no longer apply.

- [ ] **Step 2: Full Rust test + build**

Run: `cd src-tauri && cargo test && cargo build`
Expected: all engine + bridge tests PASS; build clean.

- [ ] **Step 3: Full frontend test + build**

Run: `pnpm test && pnpm exec tsc --noEmit && pnpm build`
Expected: reducer tests PASS; no type errors; Vite build succeeds.

- [ ] **Step 4: Manual smoke test (two machines)**

Run `pnpm tauri dev` on both machines. Exchange codes, connect, verify:
- Text both directions.
- Voice both directions; Mute toggles; sliders change levels and the labels update from `audioState` events.
- Closing one window shows `* peer disconnected *` on the other (BYE flush).

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: document Tauri desktop app build and usage"
```

---

## Self-Review Notes

- **Spec coverage:** Architecture/structure (Tasks 1-2), engine setters + Command swap (Tasks 3-4), bridge with `start()` race fix + Event→JSON + commands + window-close BYE (Task 5), typed client (Task 6), reducer port + tests (Task 7), screens incl. voice widgets + status line (Task 8), styling (Task 9), README + verification (Task 10). All spec sections mapped.
- **Divergence from spec, intentional:** the TUI scroll-offset state machine is replaced by DOM auto-scroll (Chat component ref); the corresponding "scroll pins" unit test is dropped as it no longer models real behavior. Logging goes to stderr (default `env_logger`) rather than `ramsit.log`, since a GUI process has no TUI to corrupt — simpler and captured by the dev console.
- **Type consistency:** `EngineEvent` (engine.ts) ⇄ `event_to_json` keys (bridge.rs) verified field-by-field; reducer `Action` = `EngineEvent | {sent}`; command names match `generate_handler!` ⇄ `engine.*` wrappers.
