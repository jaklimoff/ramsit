import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

// Windows has no native window vibrancy, so a transparent window shows through.
// Flag the platform and let CSS paint an opaque dark backdrop instead.
if (navigator.userAgent.includes("Windows")) {
  document.documentElement.classList.add("win");
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
