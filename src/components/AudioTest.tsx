import { useEffect, useState } from "react";
import { engine } from "../engine";
import DeviceSelect from "./DeviceSelect";

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
      <span className="field-label">Audio devices</span>
      <div className="device-group">
        <DeviceSelect channel="input" />
        <DeviceSelect channel="output" />
      </div>
      <div className="audio-test-controls">
        <button className="btn tinted" onClick={toggleTest}>
          {testing ? "Stop test" : "Test devices"}
        </button>
        <button className="btn tinted" disabled={!testing} onClick={toggleTone}>
          {tone ? "Stop tone" : "Play tone"}
        </button>
      </div>
    </section>
  );
}
