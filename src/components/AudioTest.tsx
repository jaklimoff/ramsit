import { useEffect, useState } from "react";
import { engine } from "../engine";
import DeviceSelect from "./DeviceSelect";
import VuMeter from "./VuMeter";

export default function AudioTest() {
  const [testing, setTesting] = useState(false);
  const [tone, setTone] = useState(false);

  // Release the mic when this panel unmounts (e.g. the call connects).
  useEffect(() => {
    return () => {
      engine.playTestTone(false);
      engine.stopAudioTest();
    };
  }, []);

  function toggleTest() {
    if (testing) {
      engine.playTestTone(false);
      engine.stopAudioTest();
      setTone(false);
      setTesting(false);
    } else {
      engine.startAudioTest();
      setTesting(true);
    }
  }

  function toggleTone() {
    const next = !tone;
    engine.playTestTone(next);
    setTone(next);
  }

  return (
    <section className="audio-test">
      <DeviceSelect />
      <div className="audio-test-controls">
        <button onClick={toggleTest}>
          {testing ? "Stop audio test" : "Test audio devices"}
        </button>
        <button disabled={!testing} onClick={toggleTone}>
          {tone ? "Stop test tone" : "Play test tone"}
        </button>
      </div>
      <VuMeter channel="input" label="Mic" />
      <VuMeter channel="output" label="Speaker" />
    </section>
  );
}
