import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { WikiProvider } from "@equationalapplications/react-llm-wiki";
import { App } from "./App";
import { wiki, setupWiki } from "./lib/wiki";
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
        <App />
      </WikiProvider>
    </React.StrictMode>
  );
}

setupWiki().then(() => {
  ReactDOM.createRoot(document.getElementById("root")!).render(<Root />);
}).catch((err) => {
  console.error("[wiki] setup failed, rendering without wiki:", err);
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>
  );
});
