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
