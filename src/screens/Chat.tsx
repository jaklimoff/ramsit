import { useEffect, useRef, useState } from "react";
import { engine } from "../engine";
import type { Screen } from "../reducer";
import DeviceSelect from "../components/DeviceSelect";
import { linkify } from "../linkify";

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
            {linkify(m.text)}
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
