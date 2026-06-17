import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type DeviceList = {
  inputs: string[];
  outputs: string[];
  currentInput: string | null;
  currentOutput: string | null;
};

export type EngineEvent =
  | { type: "discovered"; code: string; localCode: string | null }
  | { type: "connected"; peer: string }
  | { type: "incoming"; text: string }
  | { type: "audioState"; muted: boolean; inputVol: number; outputVol: number }
  | { type: "audioUnavailable"; reason: string }
  | { type: "levels"; input: number; output: number }
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
  startAudioTest: () => invoke<void>("start_audio_test"),
  stopAudioTest: () => invoke<void>("stop_audio_test"),
  playTestTone: (on: boolean) => invoke<void>("play_test_tone", { on }),
  listAudioDevices: () => invoke<DeviceList>("list_audio_devices"),
  setInputDevice: (name: string | null) => invoke<void>("set_input_device", { name }),
  setOutputDevice: (name: string | null) => invoke<void>("set_output_device", { name }),
  quit: () => invoke<void>("quit"),
};
