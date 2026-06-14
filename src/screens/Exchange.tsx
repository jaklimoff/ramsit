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
