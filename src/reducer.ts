import type { EngineEvent } from "./engine";

export type Message = { from: "me" | "peer" | "system"; text: string };

export type Screen =
  | { kind: "discovering" }
  | { kind: "exchange"; myCode: string }
  | { kind: "punching"; peer: string }
  | {
      kind: "chat";
      peer: string;
      messages: Message[];
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
        ? { ...state, messages: [...state.messages, { from: "peer", text: action.text }] }
        : state;
    case "sent":
      return state.kind === "chat"
        ? { ...state, messages: [...state.messages, { from: "me", text: action.text }] }
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
            messages: [
              ...state.messages,
              { from: "system", text: `voice unavailable: ${action.reason}` },
            ],
          }
        : state;
    case "peerLeft":
      return state.kind === "chat"
        ? {
            ...state,
            connected: false,
            messages: [...state.messages, { from: "system", text: "peer disconnected" }],
          }
        : state;
    case "fatal":
      return { kind: "fatal", message: action.message };
    default:
      return state;
  }
}
