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
