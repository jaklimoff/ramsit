# ramsit → Tauri Migration Design

**Date:** 2026-06-14
**Status:** Approved

## Goal

Replace ramsit's ratatui terminal UI with a Tauri desktop app, reusing the
existing networking and voice engine unchanged. The TUI is fully removed; the
Tauri GUI becomes the only frontend.

## Decisions

| Question | Decision |
| --- | --- |
| TUI fate | Replace fully — delete `ui.rs`/ratatui, drop TUI deps |
| Frontend stack | React + Vite (TypeScript) |
| Voice controls | GUI widgets — mute toggle button + two volume sliders |
| Project structure | Single Tauri crate (Approach A) |

## Architecture

Single Tauri crate. The proven engine — `net`, `punch`, `proto`, `audio` —
moves unchanged into `src-tauri/src/engine/`. React/Vite owns all UI: the
`Screen` state machine and keystroke handling from `app.rs`/`ui.rs`/`main.rs`
are reimplemented in TypeScript. Those three Rust files are deleted.

The migration leans on the engine's existing clean boundary: `net::spawn()`
returns `(JoinHandle, Sender<Command>, Receiver<Event>)`. The Tauri backend is
a thin bridge over those two channels.

```
ramsit/
├── package.json, vite.config.ts, index.html, tsconfig.json
├── src/                      # React frontend
│   ├── main.tsx, App.tsx
│   ├── screens/{Discovering,Exchange,Punching,Chat,Fatal}.tsx
│   ├── engine.ts             # invoke() wrappers + event subscription, typed
│   └── styles.css
└── src-tauri/
    ├── Cargo.toml, tauri.conf.json, build.rs
    └── src/
        ├── main.rs           # tauri::Builder, manages bridge state
        ├── bridge.rs         # cmd_tx holder + Event→JSON emitter + #[command]s
        └── engine/{mod,net,punch,proto,audio}.rs   # moved, ~unchanged
```

## Data Flow

### Startup
`main.rs` resolves STUN (default `stun.l.google.com:19302`, unchanged), calls
`net::spawn(stun_addr)`, stores `cmd_tx` in Tauri managed state, and spawns a
thread that drains `evt_rx` and forwards each event via
`app_handle.emit("engine-event", payload)`.

### Frontend → backend (Tauri commands)
Typed `#[command]`s push `Command`s onto `cmd_tx`:

- `submit_peer_code(code: String) -> Result<(), String>` — validates via
  `proto::parse_code`; returns the error string for the same inline display the
  TUI had. On `Ok`, sends `Command::PeerCode`.
- `send_message(text: String)` — sends `Command::Send`.
- `toggle_mute()` — sends `Command::ToggleMute`.
- `set_input_volume(pct: u8)` — sends `Command::SetInputVolume`.
- `set_output_volume(pct: u8)` — sends `Command::SetOutputVolume`.
- `quit()` — sends `Command::Quit`.

### Backend → frontend (single event channel)
Each `Event` maps to a tagged JSON object on the `"engine-event"` channel.
Serde mapping lives in `bridge.rs`, keeping `net.rs`'s enums serde-free.

| `Event` | JSON payload |
| --- | --- |
| `Discovered(addr)` | `{type:"discovered", code:"1.2.3.4:5678"}` |
| `Connected(addr)` | `{type:"connected", peer:"..."}` |
| `Incoming(s)` | `{type:"incoming", text:"..."}` |
| `AudioState{muted,input_vol,output_vol}` | `{type:"audioState", muted, inputVol, outputVol}` |
| `AudioUnavailable(s)` | `{type:"audioUnavailable", reason:"..."}` |
| `PeerLeft` | `{type:"peerLeft"}` |
| `Fatal(s)` | `{type:"fatal", message:"..."}` |

## Frontend Behavior

Mirrors the current TUI screens.

- **Discovering** → spinner. On `discovered` → Exchange.
- **Exchange** → show `my code` with a copy button; input + submit for peer
  code; inline error from `submit_peer_code`. On submit → Punching.
- **Punching** → "connecting…". On `connected` → Chat.
- **Chat** → scrollable message list (auto-pin to newest, matching the TUI's
  scroll logic); text input + send; voice widgets (mute toggle button + two
  volume sliders, 0–200%) driven by `audioState` events; a
  `[LIVE]`/`[MUTED]`/`[no voice]` status line. `incoming`/`peerLeft`/
  `audioUnavailable` append or update accordingly.
- **Fatal** → error screen.

## Engine Changes (minimal)

- Add `AudioHandle::set_input_volume(u8)` / `set_output_volume(u8)` (mirror the
  existing `adjust_*`, reuse `clamp_vol`).
- Replace `Command::AdjustInputVolume(i8)` / `AdjustOutputVolume(i8)` with
  `SetInputVolume(u8)` / `SetOutputVolume(u8)`; update the `session` loop match
  arms accordingly.
- Everything else in `net`/`punch`/`proto`/`audio` is untouched.

## Logging & Shutdown

- Keep file logging (`ramsit.log`) via `env_logger`, initialized in `main.rs`.
- On window close, send `Command::Quit` and keep the existing ~300 ms BYE-flush
  grace before exit, so the peer learns we left.

## Testing

- **Rust:** `app.rs`'s tests are deleted with the file. The engine's existing
  tests (`net`, `proto`, `audio`) are preserved and must still pass. Add a
  small `bridge.rs` unit test for the `Event`→JSON shape.
- **Frontend:** Vitest unit tests for the Screen reducer (the state-machine
  logic ported from `app.rs::apply`/`on_key`) — e.g. `incoming` appends, scroll
  pins to newest, a bad peer code shows an inline error, `audioState` updates
  the widgets.
- **Manual:** two-machine smoke test (text + voice both ways) per the README.

## Out of Scope (YAGNI)

- Installer/bundle config beyond Tauri defaults.
- A settings UI for the STUN server (stays a constant).
- In-app audio device pickers.
- Re-adding a TUI.

## Documentation

- Rewrite `README.md`: replace the "Build/Use" TUI instructions with Tauri dev
  (`pnpm tauri dev`) and build steps; keep the libopus prerequisite and the
  STUN/LAN sections. (Tracked as a step in the implementation plan.)
