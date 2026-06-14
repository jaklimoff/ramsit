import { useEffect, useState } from "react";
import { engine, type DeviceList } from "../engine";

export default function DeviceSelect() {
  const [list, setList] = useState<DeviceList | null>(null);

  async function refresh() {
    setList(await engine.listAudioDevices());
  }

  useEffect(() => {
    refresh();
  }, []);

  if (!list) return null;

  return (
    <div className="device-select">
      <label>
        Input
        <select
          value={list.currentInput ?? ""}
          onChange={async (e) => {
            await engine.setInputDevice(e.target.value || null);
            refresh();
          }}
        >
          <option value="">System default</option>
          {list.inputs.map((d) => (
            <option key={d} value={d}>
              {d}
            </option>
          ))}
        </select>
      </label>
      <label>
        Output
        <select
          value={list.currentOutput ?? ""}
          onChange={async (e) => {
            await engine.setOutputDevice(e.target.value || null);
            refresh();
          }}
        >
          <option value="">System default</option>
          {list.outputs.map((d) => (
            <option key={d} value={d}>
              {d}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}
