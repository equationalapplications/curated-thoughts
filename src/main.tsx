import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { WikiProvider } from "@equationalapplications/react-llm-wiki";
import { App } from "./App";
import { wiki, setupWiki } from "./lib/wiki";
import { ThemeProvider } from "./lib/ThemeContext";
import { AnnouncerProvider } from "./a11y";
import "./index.css";

function Root() {
  const [wikiInstance, setWikiInstance] = useState(wiki);

  useEffect(() => {
    const handleWikiUpdated = () => setWikiInstance(wiki);
    window.addEventListener("wiki-updated", handleWikiUpdated);
    return () => window.removeEventListener("wiki-updated", handleWikiUpdated);
  }, []);

  return (
    <React.StrictMode>
      <WikiProvider wiki={wikiInstance}>
        <ThemeProvider>
          <AnnouncerProvider>
            <App />
          </AnnouncerProvider>
        </ThemeProvider>
      </WikiProvider>
    </React.StrictMode>
  );
}

setupWiki().then(() => {
  ReactDOM.createRoot(document.getElementById("root")!).render(<Root />);
}).catch((err) => {
  // A bad ontology manifest must reach the user — running untyped is
  // indistinguishable from a deliberate "off" selection, so we render an
  // error surface instead of falling back to a wiki-less App.
  console.error("[wiki] setup failed:", err);
  const detail = err instanceof Error ? err.message : String(err);
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <ThemeProvider>
        <div className="loading-screen">
          <p>The knowledge schema could not be loaded.</p>
          <p style={{ fontFamily: "monospace", whiteSpace: "pre-wrap" }}>{detail}</p>
          <button type="button" onClick={() => window.location.reload()}>
            Reload
          </button>
        </div>
      </ThemeProvider>
    </React.StrictMode>
  );
});
