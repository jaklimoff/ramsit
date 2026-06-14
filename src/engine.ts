import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type EngineEvent =
  | { type: "discovered"; code: string }
  | { type: "connected"; peer: string }
  | { type: "incoming"; text: string }
  | { type: "audioState"; muted: boolean; inputVol: number; outputVol: number }
  | { type: "audioUnavailable"; reason: string }
  | { type: "peerLeft" }
  | { type: "fatal"; message: string };

export function onEngineEvent(cb: (e: EngineEvent) => void): Promise<UnlistenFn> {
  return listen<EngineEvent>("engine-event", (ev) => cb(ev.payload));
}

export const engine = {
  start: () => invoke<void>("start"),
  submitPeerCode: (code: string) => invoke<void>("submit_peer_code", { code }),
  sendMessage: (text: string) => invoke<void>("send_message", { text }),
  toggleMute: () => invoke<void>("toggle_mute"),
  setInputVolume: (pct: number) => invoke<void>("set_input_volume", { pct }),
  setOutputVolume: (pct: number) => invoke<void>("set_output_volume", { pct }),
  quit: () => invoke<void>("quit"),
};
