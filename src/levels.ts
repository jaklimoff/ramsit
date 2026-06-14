import { listen } from "@tauri-apps/api/event";

let input = 0;
let output = 0;
const subscribers = new Set<() => void>();

function emit() {
  for (const cb of subscribers) cb();
}

export function subscribe(cb: () => void): () => void {
  subscribers.add(cb);
  return () => {
    subscribers.delete(cb);
  };
}

export function getInput(): number {
  return input;
}

export function getOutput(): number {
  return output;
}

/** Test-only setter; bypasses the Tauri event listener. */
export function __setForTest(i: number, o: number): void {
  input = i;
  output = o;
  emit();
}

// Subscribe once at module load. The backend multiplexes every event onto the single
// `engine-event` channel (same channel `onEngineEvent` uses) with a `type`
// discriminator, so we listen there and keep only `type === "levels"`. The reducer
// ignores `levels`, so this is the only consumer — keeping 30 Hz updates off the React
// reducer path. Guarded so importing this module under vitest (no `window`) won't throw.
try {
  void listen<{ type: string; input: number; output: number }>(
    "engine-event",
    (ev) => {
      const p = ev.payload;
      if (p && p.type === "levels") {
        input = p.input;
        output = p.output;
        emit();
      }
    },
  ).catch(() => {
    /* no Tauri runtime (tests) */
  });
} catch {
  /* no Tauri runtime (tests) */
}
