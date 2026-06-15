# ramsit — macOS-native redesign

**Date:** 2026-06-15
**Status:** Approved design, ready for implementation plan
**Scope:** Presentation-only restyle of the whole app to a native macOS look. No networking, audio-engine, or protocol changes. One reducer data-shape change (message sender), one Tauri window-config change, one Rust vibrancy call.

## Goal

Make ramsit look and feel like a native macOS app:

- System-appearance-aware: automatically follows the OS light/dark setting.
- Translucent, vibrant window (blurred background shows through).
- No separate titlebar — window controls (traffic lights) float over the content for a sleek, seamless window.
- Refined, native-feeling buttons, inputs, device pickers, and a chat-bubble message log.

All five screens (`Discovering`, `Exchange`, `Punching`, `Chat`, `Fatal`) and shared components inherit the new look from a single design system.

## Design decisions (validated visually)

1. **Appearance:** Auto-follow system light/dark. No manual theme toggle.
2. **Window:** Vibrancy/translucency in both appearances; no titlebar; traffic lights overlay the content.
3. **Messages:** Chat bubbles — peer left (tinted), me right (blue), system messages centered/muted.
4. **Mute button:** Round icon button (FaceTime/Zoom style) — grey when live, red when muted, mic / mic-slash icon.
5. **Device pickers:** Native popup-button style (icon + device name + chevron) for both input and output, present in the chat window, each with its VU meter.
6. **Accent:** System blue `#0a84ff`.

## Architecture / components

### 1. Window & vibrancy — `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `src-tauri/src/bridge.rs`

- **Titlebar:** In the window config, set `"titleBarStyle": "Overlay"`, `"hiddenTitle": true`, and `"transparent": true`. Traffic lights float over content; the top inset region remains OS-draggable.
- **Vibrancy:** Add the `window-vibrancy` crate (macOS) to `Cargo.toml`. In the `.setup` closure in `bridge.rs`, get the main window and call `apply_vibrancy(&window, NSVisualEffectMaterial::HudWindow, None, None)`. The chosen material auto-follows system appearance, so vibrancy works in both light and dark without extra code. Wrap in `#[cfg(target_os = "macos")]` so non-mac builds still compile.
- Frontend `body`/`:root` background becomes transparent so the native vibrancy shows through.

### 2. Design system — `src/styles.css` (full rewrite)

- Remove the hard-coded `color-scheme: dark`. Set `color-scheme: light dark`.
- Define CSS variables for both appearances using `@media (prefers-color-scheme: dark)` / default-light:
  `--bg` (transparent over vibrancy), `--surface`, `--surface-2`, `--text`, `--text-secondary`, `--separator`, `--accent` (`#0a84ff`), `--danger` (`#ff453a`), `--ok` (`#34c759`).
- Shared primitives (used by every screen):
  - `.btn` — primary (filled blue, subtle inner highlight + drop shadow), `.btn.secondary` (transparent, blue text), rounded 9px, hover/active states.
  - `input`, `.field` — translucent rounded fields with focus ring.
  - `.select` — native popup-button: flex row of icon + ellipsized name + chevron, translucent surface.
  - Title-inset helpers and a content top-padding so nothing hides under the traffic lights.
- `.center` screens (Discovering, Exchange, Punching, Fatal) get top padding to clear the overlay controls and inherit the new typography/buttons.

### 3. Chat screen — `src/screens/Chat.tsx` + CSS

- **Top inset row** replaces `<header>`: transparent, draggable (`-webkit-app-region: drag` with interactive children set to `no-drag`), left padding past the traffic lights. Shows `peer <code>` on the left and a connection/`LIVE` status pill on the right.
- **Message log → bubbles:** render `state.messages` (now structured — see §5) as bubbles. `from: "me"` → right blue; `from: "peer"` → left tinted; `from: "system"` → centered muted text, no bubble. Keep auto-scroll behavior.
- **Voice bar:** round icon Mute button (grey/red, mic-slash icon when muted), restyled input + output `DeviceSelect`, existing mic/speaker volume sliders, VU meters. Same controls and handlers as today — only markup/classes change.
- **Composer:** translucent rounded field + blue `.btn` Send.

### 4. Components

- **`DeviceSelect.tsx`:** restyle into the `.select` popup-button look — a mic SVG for input, speaker SVG for output, ellipsized current device name, chevron. Behavior, options, and engine calls unchanged. Both input and output remain in the chat window.
- **`VuMeter.tsx`:** logic unchanged; restyle track/fill to a thinner rounded gradient bar (green→yellow→red) via new CSS.
- **`AudioTest.tsx`:** inherits new `.btn` and `.select`; only class names adjusted.

### 5. Message sender in state — `src/reducer.ts` + `src/reducer.test.ts`

- Introduce `type Message = { from: "me" | "peer" | "system"; text: string }`.
- Change the chat screen's `messages: string[]` to `messages: Message[]`.
- Update reducers:
  - `incoming` → `{ from: "peer", text: action.text }`
  - `sent` → `{ from: "me", text: action.text }`
  - `audioUnavailable` → `{ from: "system", text: \`voice unavailable: ${action.reason}\` }`
  - `peerLeft` → `{ from: "system", text: "peer disconnected" }`
- Drop the `peer> ` / `you> ` / `* … *` string prefixes; alignment/style now comes from `from`.
- Update `reducer.test.ts` assertions to the new shape.

## Data flow

Unchanged. Engine events → `onEngineEvent` → `reduce` → `Screen` state → screens render. The only data-shape change is `messages` becoming structured objects, consumed solely by `Chat.tsx`.

## Error handling

- Vibrancy call wrapped in `#[cfg(target_os = "macos")]`; failure is non-fatal (log and continue) so the window still opens without vibrancy.
- `prefers-color-scheme` has a light default, so unknown/forced appearances degrade gracefully.
- System messages (`voice unavailable`, `peer disconnected`) keep their visibility as centered muted bubbles.

## Testing

- `reducer.test.ts` updated for the structured `messages` shape; `vitest run` stays green.
- `levels.test.ts` untouched.
- Manual verification: launch the Tauri app (`pnpm tauri dev`), confirm: no titlebar with floating traffic lights, translucent window over a wallpaper, correct rendering in both system light and dark (toggle macOS appearance), bubbles aligned by sender, round mute toggling grey/red, input+output device pickers functional.

## Out of scope

- Audio engine, networking, STUN/punch logic.
- New features or settings (no manual theme switch, no new audio controls).
- Non-macOS visual parity (vibrancy is mac-only; other platforms compile and run with a solid background).
