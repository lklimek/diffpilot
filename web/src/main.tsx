import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app/App";
import "./index.css";

async function start() {
  if (import.meta.env.DEV && import.meta.env.VITE_MSW === "on") {
    const { startMockApi } = await import("./mocks/browser");
    await startMockApi();
  }
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void start();
