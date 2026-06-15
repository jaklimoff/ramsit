import { useEffect, useReducer, useState } from "react";
import { engine, onEngineEvent } from "./engine";
import { initialState, reduce, type Action, type Screen } from "./reducer";
import Discovering from "./screens/Discovering";
import Exchange from "./screens/Exchange";
import Punching from "./screens/Punching";
import Chat from "./screens/Chat";
import Fatal from "./screens/Fatal";

export default function App() {
  const [state, dispatch] = useReducer(reduce, initialState);
  // Punching is a UI-only transition between Exchange and the Connected event.
  const [punching, setPunching] = useState<string | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    onEngineEvent((e) => {
      if (e.type !== "levels") dispatch(e as Action);
    }).then((fn) => {
      unlisten = fn;
      engine.start(); // start AFTER the listener is attached
    });
    return () => unlisten?.();
  }, []);

  // A real Connected event supersedes the manual punching override.
  const screen: Screen =
    punching && state.kind === "exchange"
      ? { kind: "punching", peer: punching }
      : state;

  function renderScreen() {
    switch (screen.kind) {
      case "discovering":
        return <Discovering />;
      case "exchange":
        return <Exchange myCode={screen.myCode} onPunching={setPunching} />;
      case "punching":
        return <Punching peer={screen.peer} />;
      case "chat":
        return (
          <Chat state={screen} onSent={(text) => dispatch({ type: "sent", text })} />
        );
      case "fatal":
        return <Fatal message={screen.message} />;
    }
  }

  return (
    <>
      {/* No native titlebar, so a transparent top strip makes every screen draggable. */}
      <div className="drag-strip" data-tauri-drag-region />
      {renderScreen()}
    </>
  );
}
