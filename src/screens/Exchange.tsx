import { useState } from "react";
import { engine } from "../engine";
import AudioTest from "../components/AudioTest";

export default function Exchange({
  myCode,
  onPunching,
}: {
  myCode: string;
  onPunching: (peer: string) => void;
}) {
  const [input, setInput] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    try {
      await engine.submitPeerCode(input);
      onPunching(input.trim());
    } catch (err) {
      setError(String(err));
    }
  }

  function copy() {
    navigator.clipboard.writeText(myCode);
    setCopied(true);
    setTimeout(() => setCopied(false), 1200);
  }

  return (
    <main className="setup">
      <div className="setup-card">
        <header className="setup-head">
          <h1>Ramsit</h1>
          <p className="subtle">Share your code, or connect to a peer.</p>
        </header>

        <div className="field-group">
          <span className="field-label">Your code</span>
          <div className="code-pill">
            <code>{myCode}</code>
            <button className="btn tinted" onClick={copy}>
              {copied ? "Copied" : "Copy"}
            </button>
          </div>
        </div>

        <form className="field-group" onSubmit={submit}>
          <span className="field-label">Connect to a peer</span>
          <div className="connect-row">
            <input
              autoFocus
              placeholder="1.2.3.4:5678"
              value={input}
              onChange={(e) => {
                setInput(e.target.value);
                setError(null);
              }}
            />
            <button type="submit" disabled={!input.trim()}>
              Connect
            </button>
          </div>
          {error && <p className="error">{error}</p>}
        </form>

        <AudioTest />
      </div>
    </main>
  );
}
