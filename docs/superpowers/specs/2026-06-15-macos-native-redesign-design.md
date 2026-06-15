# ramsit — macOS-native redesign

**Date:** 2026-06-15
**Status:** Approved design, revised after architect review, ready for implementation plan
**Scope:** Presentation-mostly restyle of the whole app to a native macOS look. No networking, audio-engine, or protocol changes. Structural changes are limited to: one reducer data-shape change (message sender), Tauri window-config + Cargo feature flags for transparency/vibrancy, one Rust vibrancy call, and splitting `DeviceSelect` into a single-channel component.

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
5. **Device pickers:** Native `<select>` for both input and output (kept native — not a custom widget), styled as a popup-button row with a mic/speaker icon **beside** the select and a chevron, present in the chat window, each with its own VU meter directly beneath.
6. **Accent:** System blue `#0a84ff`.

## Architecture / components

### 1. Window & vibrancy — `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `src-tauri/src/bridge.rs`

- **Transparency prerequisites (mandatory — build errors without both):**
  - `tauri.conf.json` → add `"app": { "macOSPrivateApi": true }` (alongside the existing `app` keys).
  - `Cargo.toml` → enable the matching feature: `tauri = { version = "2", features = ["macos-private-api"] }`. The config flag and crate feature must both be present and agree.
- **Titlebar:** In the window config set `"titleBarStyle": "Overlay"`, `"hiddenTitle": true`, and `"transparent": true`. Traffic lights float over content; the top inset region remains OS-draggable. Use the default traffic-light position (don't set `trafficLightPosition` unless manual verification shows misalignment).
- **Vibrancy:** Add the `window-vibrancy` crate (macOS-only) to `Cargo.toml`. In the `.setup` closure in `bridge.rs`, fetch the main window with `app.get_webview_window("main")` (the single unlabeled window defaults to the label `"main"`) and call `apply_vibrancy(&window, NSVisualEffectMaterial::HudWindow, None, None)`. The semantic `HudWindow` material auto-follows system appearance, so vibrancy works in both light and dark with no manual swap. Wrap the whole block in `#[cfg(target_os = "macos")]` so non-mac builds still compile; on `None`/`Err`, log and continue (non-fatal) so the window still opens.
- Frontend `body`/`:root` background becomes transparent so the native vibrancy shows through.

### 2. Design system — `src/styles.css` (incremental refactor, not a from-scratch rewrite)

Refactor in place to avoid regressing the four simple screens. **Preserve the existing `.center`, `.error`, `.ok`, `.bad` classes** (still referenced by screens) and extend them; add new primitives alongside.

- Remove the hard-coded `color-scheme: dark`. Set `color-scheme: light dark`.
- Define CSS variables for both appearances using `@media (prefers-color-scheme: dark)` over a default-light base:
  `--bg` (transparent over vibrancy), `--surface`, `--surface-2`, `--text`, `--text-secondary`, `--separator`, `--accent` (`#0a84ff`), `--danger` (`#ff453a`), `--ok` (`#34c759`). `.error`/`.bad` map to `--danger`, `.ok` to `--ok`.
- Shared primitives (used by every screen):
  - `.btn` — primary (filled blue, subtle inner highlight + drop shadow), `.btn.secondary` (transparent, blue text), rounded 9px, hover/active states.
  - `input`, `.field` — translucent rounded fields.
  - `.device-row` — popup-button row: mic/speaker icon **beside** a native `<select>` (the select keeps native chevron/behavior; the leading icon is a separate element), translucent surface, ellipsized text.
  - `:focus-visible` ring on all interactive elements (buttons, inputs, selects) — keyboard focus must stay visible after the restyle.
  - Title-inset helpers and a content top-padding so nothing hides under the traffic lights.
- Verify text contrast of `--text`/`--text-secondary` over the translucent surfaces in both appearances (WCAG AA).
- `.center` screens (Discovering, Exchange, Punching, Fatal) get top padding to clear the overlay controls and inherit the new typography/buttons.

### 3. Chat screen — `src/screens/Chat.tsx` + CSS

- **Top inset row** replaces `<header>`: transparent, draggable via `data-tauri-drag-region` on the container (preferred over raw `-webkit-app-region`); interactive children opt out. Left padding past the traffic lights. Shows `peer <code>` on the left and a connection/`LIVE` status pill on the right.
- **Message log → bubbles:** render `state.messages` (now structured — see §5) as bubbles. `from: "me"` → right blue; `from: "peer"` → left tinted; `from: "system"` → centered muted text, no bubble. Keep auto-scroll behavior (`useEffect` dep `state.messages.length` is unaffected by the shape change).
- **Voice bar:**
  - Round icon Mute button (grey live / red muted, mic / mic-slash icon). The visible text label is dropped, so it **must** carry `aria-pressed={muted}` + an `aria-label` ("Mute microphone" / "Unmute microphone"); keep the existing `disabled={!state.voice}` and `onClick`.
  - Two `DeviceSelect` instances — `channel="input"` and `channel="output"` (see §4) — each with its own VU meter beneath. Replaces today's single `<DeviceSelect/>` plus the two standalone `<VuMeter/>` siblings.
  - Existing mic/speaker volume sliders, unchanged.
- **Composer:** translucent rounded field + blue `.btn` Send.

### 4. Components

- **`DeviceSelect.tsx` (split to single-channel):** change from one propless component rendering both selects to `DeviceSelect({ channel: "input" | "output" })` rendering exactly one channel — a leading mic (input) / speaker (output) SVG icon **beside** the native `<select>`, plus a `<VuMeter channel={channel} .../>` directly beneath. Each instance fetches the device list and refreshes on change (independent and self-contained; the two channels' lists are unrelated, so no shared state needed). Engine calls (`listAudioDevices`, `setInputDevice`/`setOutputDevice`) unchanged — input instances use the input setter, output instances the output setter.
- **`VuMeter.tsx`:** logic unchanged; restyle track/fill to a thinner rounded gradient bar (green→yellow→red) via new CSS. Now rendered inside `DeviceSelect` rather than as a standalone sibling.
- **`AudioTest.tsx`:** replace `<DeviceSelect/>` + the two standalone `<VuMeter/>` with `<DeviceSelect channel="input"/>` and `<DeviceSelect channel="output"/>` (meters now bundled); inherits the new `.btn`/`.device-row` styles.

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

- `reducer.test.ts` — four assertions break on the `string[] → Message[]` change and must be updated:
  - line 19: `toEqual(["peer> yo"])` → `toEqual([{ from: "peer", text: "yo" }])`
  - line 26: `toEqual(["you> hello"])` → `toEqual([{ from: "me", text: "hello" }])`
  - line 45: `m.includes("disconnected")` → `m.text.includes("disconnected")` (would throw on an object otherwise)
  - line 54: `m.includes("voice unavailable")` → `m.text.includes("voice unavailable")`
- `levels.test.ts` untouched. `vitest run` must stay green after the edits.
- Manual verification:
  - `pnpm tauri dev`: no titlebar with floating traffic lights, translucent window over a wallpaper, correct rendering in both system light and dark (toggle macOS appearance mid-run), bubbles aligned by sender, system messages centered/muted, round mute toggling grey/red, both device pickers functional with live VU meters, keyboard focus rings visible.
  - **`pnpm tauri build`** then launch the bundled app: confirm transparency/vibrancy survives the release build (a known Tauri 2 issue can render transparent windows opaque/white in `build` but not `dev` — dev alone won't catch it).

## Out of scope

- Audio engine, networking, STUN/punch logic.
- New features or settings (no manual theme switch, no new audio controls).
- Non-macOS visual parity (vibrancy is mac-only; other platforms compile and run with a solid background).
