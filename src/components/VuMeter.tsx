import { useSyncExternalStore } from "react";
import { subscribe, getInput, getOutput } from "../levels";

export default function VuMeter({
  channel,
  label,
}: {
  channel: "input" | "output";
  label: string;
}) {
  const level = useSyncExternalStore(
    subscribe,
    channel === "input" ? getInput : getOutput,
  );
  const pct = Math.round(Math.min(1, Math.max(0, level)) * 100);
  return (
    <div className="vu">
      <span className="vu-label">{label}</span>
      <div
        className="vu-track"
        role="meter"
        aria-label={`${label} level`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
      >
        <div className="vu-fill" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}
