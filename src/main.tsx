import React from "react";
import ReactDOM from "react-dom/client";
import { isTauri } from "@tauri-apps/api/core";
import App from "./App";
import "./styles.css";
import "./reference.css";

async function start() {
  if (import.meta.env.DEV && !isTauri() && new URLSearchParams(location.search).has("preview")) {
    const { installPreview } = await import("./preview");
    installPreview();
  }
  ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
}

void start();
