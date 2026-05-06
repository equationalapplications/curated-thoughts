import React from "react";
import ReactDOM from "react-dom/client";
import { WikiProvider } from "@equationalapplications/react-llm-wiki";
import { App } from "./App";
import { wiki, setupWiki } from "./lib/wiki";
import "./index.css";

setupWiki().then(() => {
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <WikiProvider wiki={wiki}>
        <App />
      </WikiProvider>
    </React.StrictMode>
  );
}).catch((err) => {
  console.error("[wiki] setup failed, rendering without wiki:", err);
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>
  );
});
