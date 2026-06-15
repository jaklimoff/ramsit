# macOS-Native Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restyle the ramsit desktop app to a native macOS look — auto light/dark, translucent vibrant window, no titlebar (traffic lights overlay content), chat bubbles, round mute button, native device pickers.

**Architecture:** Presentation-mostly change. One reducer data-shape change (`messages: string[] → Message[]`), Tauri window-config + Cargo feature flags for transparency, one Rust `window-vibrancy` call, an in-place `styles.css` refactor to a CSS-variable design system, and splitting `DeviceSelect` into a single-channel component. No networking/audio-engine/protocol changes.

**Tech Stack:** Tauri 2 (Rust), React 18 + TypeScript, Vite, Vitest, `window-vibrancy` crate, CSS custom properties + `prefers-color-scheme`.

**Spec:** `docs/superpowers/specs/2026-06-15-macos-native-redesign-design.md`

**Testing note:** This codebase only unit-tests pure logic (`reducer.test.ts`, `levels.test.ts`) — there is no jsdom/React Testing Library setup, and adding one is out of scope (YAGNI). Task 1 (pure reducer logic) is TDD. The component/CSS/Rust tasks are verified by `tsc` typecheck, `cargo` build, and the manual checklist in Task 6.

---

## File Structure

- `src/reducer.ts` — add `Message` type, change `messages` to `Message[]`, update producers. **(Task 1)**
- `src/reducer.test.ts` — update four assertions to the new shape. **(Task 1)**
- `src-tauri/tauri.conf.json` — `macOSPrivateApi`, transparent/overlay window. **(Task 2)**
- `src-tauri/Cargo.toml` — `macos-private-api` feature + `window-vibrancy` dep. **(Task 2)**
- `src-tauri/src/bridge.rs` — apply vibrancy in `.setup`. **(Task 2)**
- `src/styles.css` — full design-system refactor. **(Task 3)**
- `src/components/DeviceSelect.tsx` — single-channel component with bundled VU meter. **(Task 4)**
- `src/components/AudioTest.tsx` — use two single-channel `DeviceSelect`s. **(Task 4)**
- `src/screens/Chat.tsx` — top inset row, bubbles, round mute, new voice bar. **(Task 5)**
- `src/components/VuMeter.tsx` — unchanged logic; styled by Task 3 CSS.

---

## Task 1: Structured messages in the reducer (TDD)

**Files:**
- Modify: `src/reducer.ts`
- Test: `src/reducer.test.ts:19,26,45,54`

- [ ] **Step 1: Update the tests to expect the new `Message` shape**

In `src/reducer.test.ts`, change exactly these four assertions:

Line 19:
```ts
    if (s.kind === "chat") expect(s.messages).toEqual([{ from: "peer", text: "yo" }]);
```

Line 26:
```ts
    if (s.kind === "chat") expect(s.messages).toEqual([{ from: "me", text: "hello" }]);
```

Line 45:
```ts
      expect(s.messages.some((m) => m.text.includes("disconnected"))).toBe(true);
```

Line 54:
```ts
      expect(s.messages.some((m) => m.text.includes("voice unavailable"))).toBe(true);
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `pnpm test`
Expected: FAIL — the `incoming`/`sent` cases mismatch (`"peer> yo"` ≠ object), and the `.some()` cases throw `m.text is undefined` because `m` is still a string.

- [ ] **Step 3: Add the `Message` type and switch the chat state to `Message[]`**

In `src/reducer.ts`, add the type above the `Screen` union (after the import on line 1):
```ts
export type Message = { from: "me" | "peer" | "system"; text: string };
```

Change the chat screen's `messages` field (currently `messages: string[];`) to:
```ts
      messages: Message[];
```

- [ ] **Step 4: Update the four message producers**

In `src/reducer.ts`, replace the string-prefix pushes:

`incoming` case:
```ts
    case "incoming":
      return state.kind === "chat"
        ? { ...state, messages: [...state.messages, { from: "peer", text: action.text }] }
        : state;
```

`sent` case:
```ts
    case "sent":
      return state.kind === "chat"
        ? { ...state, messages: [...state.messages, { from: "me", text: action.text }] }
        : state;
```

`audioUnavailable` case (replace the `messages` line):
```ts
            messages: [
              ...state.messages,
              { from: "system", text: `voice unavailable: ${action.reason}` },
            ],
```

`peerLeft` case (replace the `messages` line):
```ts
            messages: [...state.messages, { from: "system", text: "peer disconnected" }],
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `pnpm test`
Expected: PASS (all reducer + levels tests green).

- [ ] **Step 6: Commit**

```bash
git add src/reducer.ts src/reducer.test.ts
git commit -m "feat(state): tag chat messages with sender (Message type)"
```

---

## Task 2: Transparent vibrant window, no titlebar (Tauri + Rust)

**Files:**
- Modify: `src-tauri/tauri.conf.json`
- Modify: `src-tauri/Cargo.toml:21`
- Modify: `src-tauri/src/bridge.rs` (setup closure, before `Ok(())` on line 201)

- [ ] **Step 1: Enable the macOS private API + transparent overlay window in config**

In `src-tauri/tauri.conf.json`, replace the entire `"app"` block (lines 12–25) with:
```json
  "app": {
    "macOSPrivateApi": true,
    "windows": [
      {
        "title": "ramsit",
        "width": 800,
        "height": 600,
        "resizable": true,
        "fullscreen": false,
        "titleBarStyle": "Overlay",
        "hiddenTitle": true,
        "transparent": true
      }
    ],
    "security": {
      "csp": null
    }
  },
```

- [ ] **Step 2: Enable the matching Cargo feature and add `window-vibrancy`**

In `src-tauri/Cargo.toml`, change line 21 from `tauri = { version = "2", features = [] }` to:
```toml
tauri = { version = "2", features = ["macos-private-api"] }
```

Then add a macOS-only dependency section at the end of the file:
```toml
[target."cfg(target_os = \"macos\")".dependencies]
window-vibrancy = "0.5"
```

- [ ] **Step 3: Apply vibrancy in the setup closure**

In `src-tauri/src/bridge.rs`, inside the `.setup(move |app| { ... })` closure, insert this block immediately before `Ok(())` (currently line 201). `Manager` is already imported (line 12), so `get_webview_window` is in scope:
```rust
            #[cfg(target_os = "macos")]
            {
                use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};
                if let Some(window) = app.get_webview_window("main") {
                    if let Err(e) =
                        apply_vibrancy(&window, NSVisualEffectMaterial::HudWindow, None, None)
                    {
                        log::warn!("window vibrancy unavailable: {e}");
                    }
                }
            }
```

- [ ] **Step 4: Make the web layer transparent so vibrancy shows through**

In `src/styles.css`, change the `body` background. (Task 3 rewrites this file fully; if Task 3 is done first this is already handled — set `background: transparent` on `body` there. If doing Task 2 first, temporarily edit line 9 `body { margin: 0; background: var(--bg); ... }` so `--bg`/body is `transparent`.)

- [ ] **Step 5: Build the Rust side to verify it compiles**

Run: `cd src-tauri && cargo build 2>&1 | tail -20`
Expected: compiles successfully (downloads `window-vibrancy` on first run). No errors about `macos-private-api` or `apply_vibrancy`.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/bridge.rs
git commit -m "feat(window): transparent overlay titlebar + macOS vibrancy"
```

---

## Task 3: Design-system CSS (auto light/dark, native primitives)

**Files:**
- Modify: `src/styles.css` (full in-place refactor — preserves `.center/.error/.ok/.bad`)

- [ ] **Step 1: Replace `src/styles.css` with the design system**

Write the complete file:
```css
:root {
  color-scheme: light dark;
  font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", system-ui, sans-serif;

  /* light appearance (default) */
  --surface: rgba(255, 255, 255, 0.7);
  --surface-2: rgba(118, 118, 128, 0.12);
  --text: #1d1d1f;
  --text-secondary: rgba(60, 60, 67, 0.6);
  --separator: rgba(0, 0, 0, 0.08);
  --accent: #0a84ff;
  --danger: #ff453a;
  --ok: #34c759;
  --bubble-them: rgba(118, 118, 128, 0.16);
}

@media (prefers-color-scheme: dark) {
  :root {
    --surface: rgba(118, 118, 128, 0.24);
    --surface-2: rgba(118, 118, 128, 0.24);
    --text: #f2f2f5;
    --text-secondary: rgba(235, 235, 245, 0.6);
    --separator: rgba(255, 255, 255, 0.1);
    --bubble-them: rgba(118, 118, 128, 0.32);
  }
}

* { box-sizing: border-box; }

/* transparent so native window vibrancy shows through */
body { margin: 0; background: transparent; color: var(--text); }

.error, .bad { color: var(--danger); }
.ok { color: var(--ok); }

/* ---- shared controls ---- */
button, input, select {
  font: inherit;
  color: var(--text);
}

.btn, button {
  cursor: pointer;
  border: none;
  border-radius: 9px;
  padding: 8px 16px;
  font-size: 13px;
  font-weight: 600;
  color: #fff;
  background: var(--accent);
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.25), 0 1px 3px rgba(0, 0, 0, 0.25);
}
.btn:hover, button:hover { filter: brightness(1.06); }
.btn:active, button:active { filter: brightness(0.94); }
.btn:disabled, button:disabled { opacity: 0.45; cursor: default; filter: none; }

.btn.secondary {
  background: transparent;
  color: var(--accent);
  box-shadow: none;
}

input:not([type="range"]), .field, select {
  background: var(--surface-2);
  border: 0.5px solid var(--separator);
  border-radius: 9px;
  padding: 8px 12px;
  color: var(--text);
}
input[type="range"] { accent-color: var(--accent); }

:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

/* ---- simple centered screens ---- */
.center {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 0.75rem;
  height: 100vh;
  padding: 48px 1rem 1rem; /* top padding clears overlay traffic lights */
  text-align: center;
}
.center code {
  background: var(--surface-2);
  padding: 2px 6px;
  border-radius: 6px;
}

/* ---- chat ---- */
.chat { display: flex; flex-direction: column; height: 100vh; }

/* draggable inset row in place of a titlebar; left pad clears traffic lights */
.titlebar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 44px;
  padding: 0 16px 0 80px;
  font-size: 13px;
  color: var(--text-secondary);
}
.titlebar .peer { font-weight: 600; color: var(--text); }
.titlebar .status {
  padding: 2px 9px;
  border-radius: 20px;
  font-size: 11px;
  font-weight: 700;
}
.titlebar .status.live { background: rgba(52, 199, 89, 0.18); color: var(--ok); }
.titlebar .status.off { background: rgba(255, 69, 58, 0.18); color: var(--danger); }

.log {
  flex: 1;
  overflow-y: auto;
  padding: 12px 16px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.bubble {
  max-width: 78%;
  padding: 7px 12px;
  border-radius: 16px;
  font-size: 13.5px;
  line-height: 1.35;
  white-space: pre-wrap;
  word-break: break-word;
}
.bubble.me { align-self: flex-end; background: var(--accent); color: #fff; }
.bubble.them { align-self: flex-start; background: var(--bubble-them); color: var(--text); }
.bubble.system {
  align-self: center;
  background: none;
  color: var(--text-secondary);
  font-size: 12px;
  font-style: italic;
}

/* voice bar */
.voice {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 16px;
  border-top: 0.5px solid var(--separator);
  flex-wrap: wrap;
}
.voice label {
  display: flex;
  align-items: center;
  gap: 0.35rem;
  font-size: 12px;
  color: var(--text-secondary);
}

/* round mute button */
.mute {
  width: 40px;
  height: 40px;
  padding: 0;
  border-radius: 50%;
  display: grid;
  place-items: center;
  background: var(--surface-2);
  color: var(--text);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.25);
}
.mute.muted { background: var(--danger); color: #fff; }
.mute svg { width: 20px; height: 20px; }

/* device picker row: icon beside native select, VU meter beneath */
.device-row { display: flex; flex-direction: column; gap: 6px; min-width: 180px; flex: 1 1 180px; }
.device-line { display: flex; align-items: center; gap: 8px; }
.device-icon { display: grid; place-items: center; opacity: 0.7; flex: 0 0 auto; }
.device-icon svg { width: 16px; height: 16px; }
.device-line select { flex: 1; max-width: 100%; }

/* composer */
.chat form {
  display: flex;
  gap: 8px;
  padding: 12px 16px;
  border-top: 0.5px solid var(--separator);
}
.chat form input { flex: 1; }

/* VU meter */
.vu { display: flex; align-items: center; gap: 8px; }
.vu-label { width: 56px; font-size: 11px; color: var(--text-secondary); }
.vu-track {
  flex: 1;
  height: 6px;
  background: rgba(128, 128, 128, 0.25);
  border-radius: 3px;
  overflow: hidden;
}
.vu-fill {
  height: 100%;
  border-radius: 3px;
  background: linear-gradient(90deg, #34c759, #ffd60a, #ff453a);
  transition: width 80ms linear;
}

/* audio test panel */
.audio-test { display: flex; flex-direction: column; gap: 12px; width: 100%; max-width: 360px; }
.audio-test-controls { display: flex; gap: 8px; justify-content: center; }
```

- [ ] **Step 2: Verify the frontend builds**

Run: `pnpm build`
Expected: `tsc` + Vite build succeed with no errors (CSS-only change; existing class names `.center/.error/.ok/.bad/.chat/.log/.voice/.vu*` still resolve).

- [ ] **Step 3: Commit**

```bash
git add src/styles.css
git commit -m "feat(ui): macOS design-system CSS with auto light/dark"
```

---

## Task 4: Split DeviceSelect into a single-channel component

**Files:**
- Modify: `src/components/DeviceSelect.tsx` (full rewrite)
- Modify: `src/components/AudioTest.tsx`

- [ ] **Step 1: Rewrite `DeviceSelect.tsx` as a single-channel component with a bundled VU meter**

Write the complete file:
```tsx
import { useEffect, useState } from "react";
import { engine, type DeviceList } from "../engine";
import VuMeter from "./VuMeter";

const MIC = (
  <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
    <path d="M12 14a3 3 0 0 0 3-3V6a3 3 0 0 0-6 0v5a3 3 0 0 0 3 3z" />
    <path d="M19 11a7 7 0 0 1-14 0H3a9 9 0 0 0 8 8.94V23h2v-3.06A9 9 0 0 0 21 11h-2z" />
  </svg>
);
const SPEAKER = (
  <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
    <path d="M5 9v6h4l5 5V4L9 9H5z" />
    <path d="M16 8a5 5 0 0 1 0 8v-2a3 3 0 0 0 0-4V8z" />
  </svg>
);

export default function DeviceSelect({
  channel,
}: {
  channel: "input" | "output";
}) {
  const [list, setList] = useState<DeviceList | null>(null);

  async function refresh() {
    setList(await engine.listAudioDevices());
  }

  useEffect(() => {
    refresh();
  }, []);

  if (!list) return null;

  const isInput = channel === "input";
  const devices = isInput ? list.inputs : list.outputs;
  const current = isInput ? list.currentInput : list.currentOutput;
  const setDevice = isInput ? engine.setInputDevice : engine.setOutputDevice;
  const label = isInput ? "Microphone" : "Output";

  return (
    <div className="device-row">
      <div className="device-line">
        <span className="device-icon">{isInput ? MIC : SPEAKER}</span>
        <select
          aria-label={label}
          value={current ?? ""}
          onChange={async (e) => {
            await setDevice(e.target.value || null);
            refresh();
          }}
        >
          <option value="">System default</option>
          {devices.map((d) => (
            <option key={d} value={d}>
              {d}
            </option>
          ))}
        </select>
      </div>
      <VuMeter channel={channel} label={isInput ? "Mic" : "Speaker"} />
    </div>
  );
}
```

- [ ] **Step 2: Update `AudioTest.tsx` to use two single-channel pickers**

In `src/components/AudioTest.tsx`, remove the `VuMeter` import (line 4) and replace the JSX `<section>` body. The new file:
```tsx
import { useEffect, useState } from "react";
import { engine } from "../engine";
import DeviceSelect from "./DeviceSelect";

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
      <DeviceSelect channel="input" />
      <DeviceSelect channel="output" />
      <div className="audio-test-controls">
        <button onClick={toggleTest}>
          {testing ? "Stop audio test" : "Test audio devices"}
        </button>
        <button disabled={!testing} onClick={toggleTone}>
          {tone ? "Stop test tone" : "Play test tone"}
        </button>
      </div>
    </section>
  );
}
```

- [ ] **Step 3: Typecheck/build to verify**

Run: `pnpm build`
Expected: succeeds. (`DeviceSelect` now requires a `channel` prop; the only other caller is `Chat.tsx`, updated in Task 5 — if Task 4 is committed before Task 5, build will error on `Chat.tsx`'s prop-less `<DeviceSelect/>`. Run Task 5 in the same session, or temporarily pass `channel="input"` — Task 5 finalizes it.)

- [ ] **Step 4: Commit**

```bash
git add src/components/DeviceSelect.tsx src/components/AudioTest.tsx
git commit -m "feat(ui): single-channel DeviceSelect with bundled VU meter"
```

---

## Task 5: Restyle the Chat screen (inset titlebar, bubbles, round mute)

**Files:**
- Modify: `src/screens/Chat.tsx` (full rewrite)

- [ ] **Step 1: Rewrite `Chat.tsx`**

Write the complete file:
```tsx
import { useEffect, useRef, useState } from "react";
import { engine } from "../engine";
import type { Screen } from "../reducer";
import DeviceSelect from "../components/DeviceSelect";

type ChatState = Extract<Screen, { kind: "chat" }>;

const MIC = (
  <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
    <path d="M12 14a3 3 0 0 0 3-3V6a3 3 0 0 0-6 0v5a3 3 0 0 0 3 3z" />
    <path d="M19 11a7 7 0 0 1-14 0H3a9 9 0 0 0 8 8.94V23h2v-3.06A9 9 0 0 0 21 11h-2z" />
  </svg>
);
const MIC_OFF = (
  <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
    <path d="M3.3 2 2 3.3l6 6V11a3 3 0 0 0 4.7 2.4l1.5 1.5A5 5 0 0 1 7 11H5a7 7 0 0 0 5 6.7V21h2v-3.3l4.7 4.7L20.7 21 3.3 2z" />
    <path d="M15 11V6a3 3 0 0 0-5.5-1.6L15 11z" />
  </svg>
);

export default function Chat({
  state,
  onSent,
}: {
  state: ChatState;
  onSent: (text: string) => void;
}) {
  const [input, setInput] = useState("");
  const logRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to newest on every message change.
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

  return (
    <main className="chat">
      <header className="titlebar" data-tauri-drag-region>
        <span className="peer">peer {state.peer}</span>
        <span className={`status ${state.connected ? "live" : "off"}`}>
          {state.connected ? "connected" : "disconnected"}
        </span>
      </header>

      <div className="log" ref={logRef}>
        {state.messages.map((m, i) => (
          <div key={i} className={`bubble ${m.from}`}>
            {m.text}
          </div>
        ))}
      </div>

      <div className="voice">
        <button
          className={`mute${state.muted ? " muted" : ""}`}
          disabled={!state.voice}
          aria-pressed={state.muted}
          aria-label={state.muted ? "Unmute microphone" : "Mute microphone"}
          onClick={() => engine.toggleMute()}
        >
          {state.muted ? MIC_OFF : MIC}
        </button>
        <DeviceSelect channel="input" />
        <DeviceSelect channel="output" />
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

- [ ] **Step 2: Build to verify the whole frontend compiles**

Run: `pnpm build`
Expected: succeeds. `m.from`/`m.text` resolve against the `Message` type from Task 1; both `DeviceSelect` usages pass `channel`.

- [ ] **Step 3: Run the unit tests (regression guard)**

Run: `pnpm test`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/screens/Chat.tsx
git commit -m "feat(ui): native chat screen — inset titlebar, bubbles, round mute"
```

---

## Task 6: Full-app verification (dev + release build)

**Files:** none (verification only)

- [ ] **Step 1: Typecheck + unit tests**

Run: `pnpm build && pnpm test`
Expected: both succeed.

- [ ] **Step 2: Run the app in dev and walk the checklist**

Run: `pnpm tauri dev`
Confirm visually:
- Window has **no titlebar**; traffic-light buttons float over the top-left content; the top inset row is draggable.
- Window is **translucent/vibrant** over a wallpaper.
- Toggle macOS appearance (System Settings → Appearance) **while running**: UI switches light/dark correctly, text stays legible over the vibrancy.
- Reach the chat screen: messages render as **bubbles** (you = right/blue, peer = left/tinted); system notices (e.g. "peer disconnected") are **centered/italic**.
- **Round mute button** toggles grey ↔ red; disabled until voice is live.
- Both **input and output device pickers** are present, list devices, switch the active device, and show live **VU meters**.
- Keyboard focus rings are visible when tabbing.

- [ ] **Step 3: Verify the release build keeps transparency**

Run: `pnpm tauri build` then launch the bundled app from `src-tauri/target/release/bundle/`.
Expected: transparency/vibrancy survives the release build (guards against the known Tauri 2 bug where transparent windows render opaque in `build` but not `dev`). If it regresses, the fix is config/feature-flag level (re-verify `macOSPrivateApi` + `macos-private-api` from Task 2) — note it and stop for review.

- [ ] **Step 4: Commit any verification-driven fixes (if needed)**

```bash
git add -A
git commit -m "fix(ui): redesign verification follow-ups"
```

---

## Self-Review (completed by plan author)

- **Spec coverage:** §1 window/vibrancy → Task 2; §2 design system → Task 3; §3 chat screen → Task 5; §4 components (DeviceSelect/VuMeter/AudioTest) → Tasks 4 + 3 (CSS); §5 reducer → Task 1; Testing → Tasks 1 & 6. All sections mapped.
- **Type consistency:** `Message`/`from`/`text` (Task 1) are consumed in `Chat.tsx` (Task 5). `DeviceSelect({ channel })` (Task 4) matches both call sites (Tasks 4 & 5). `VuMeter({ channel, label })` matches its existing signature. CSS class names (`.titlebar/.bubble.{me,them,system}/.mute/.device-row/.device-line/.device-icon/.vu*`) in Task 3 match the markup emitted in Tasks 4 & 5.
- **Cross-task build ordering:** Task 4's prop change breaks `Chat.tsx` until Task 5; called out in Task 4 Step 3. Run Tasks 4→5 back-to-back.
