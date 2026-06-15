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
    if (s.kind === "chat") expect(s.messages).toEqual([{ from: "peer", text: "yo" }]);
    else throw new Error("expected chat");
  });

  it("local echo appends a you message", () => {
    let s = reduce(initialState, { type: "connected", peer: "p" });
    s = reduce(s, { type: "sent", text: "hello" });
    if (s.kind === "chat") expect(s.messages).toEqual([{ from: "me", text: "hello" }]);
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
      expect(s.messages.some((m) => m.text.includes("disconnected"))).toBe(true);
    } else throw new Error("expected chat");
  });

  it("audioUnavailable clears voice and notes it", () => {
    let s = reduce(initialState, { type: "connected", peer: "p" });
    s = reduce(s, { type: "audioUnavailable", reason: "no mic" });
    if (s.kind === "chat") {
      expect(s.voice).toBe(false);
      expect(s.messages.some((m) => m.text.includes("voice unavailable"))).toBe(true);
    } else throw new Error("expected chat");
  });

  it("fatal transitions to fatal screen from anywhere", () => {
    const s = reduce(initialState, { type: "fatal", message: "boom" });
    expect(s.kind).toBe("fatal");
  });
});
