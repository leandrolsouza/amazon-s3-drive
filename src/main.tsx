import React from "react";
import ReactDOM from "react-dom/client";
import { invoke, isTauri } from "@tauri-apps/api/core";
import App from "./App";
import { initializeLocale } from "./i18n";

async function start() {
  const windowsTitleBar = isTauri() && navigator.userAgent.includes("Windows NT");
  if (windowsTitleBar) {
    document.documentElement.setAttribute("data-native-backdrop", "");
  }
  initializeLocale(await invoke<string>("get_language").catch(() => null));
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App windowsTitleBar={windowsTitleBar} />
    </React.StrictMode>
  );
}

void start();
