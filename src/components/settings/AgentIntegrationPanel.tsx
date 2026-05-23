import { useEffect, useMemo, useState } from "react";
import { getBinaryPath } from "../../lib/tauri";

interface Props {
  brainDir: string | null;
  brainDirError?: string | null;
}

function binaryPath(): string {
  // Prefer the non-deprecated userAgentData API (Chromium 90+ / Tauri WebView);
  // fall back to navigator.platform for environments where it isn't available.
  const p =
    typeof navigator !== "undefined"
      ? ((navigator as unknown as { userAgentData?: { platform?: string } })
          .userAgentData?.platform ?? navigator.platform ?? "")
      : "";
  if (/Win/i.test(p)) {
    return "C:\\Program Files\\Curated Thoughts\\curated-thoughts.exe";
  }
  if (/Linux/i.test(p)) {
    return "/usr/bin/curated-thoughts";
  }
  // macOS default
  return "/Applications/Curated Thoughts.app/Contents/MacOS/curated-thoughts";
}

export function AgentIntegrationPanel({ brainDir, brainDirError }: Props) {
  const [commandPath, setCommandPath] = useState<string>(binaryPath());
  const [binaryPathError, setBinaryPathError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;

    getBinaryPath()
      .then((path) => {
        if (active && path) {
          setCommandPath(path);
        }
      })
      .catch((error) => {
        if (!active) {
          return;
        }
        console.error("Failed to resolve binary path for MCP snippet:", error);
        setBinaryPathError(
          "Could not resolve the current app binary path. The snippet is using a safe fallback path.",
        );
      });

    return () => {
      active = false;
    };
  }, []);

  const snippet = useMemo(
    () =>
      brainDir === null
        ? ""
        : JSON.stringify(
            {
              mcpServers: {
                "curated-thoughts": {
                  command: commandPath,
                  args: ["--mcp"],
                  env: {
                    CURATED_BRAIN_DIR: brainDir,
                  },
                },
              },
            },
            null,
            2,
          ),
    [brainDir, commandPath],
  );

  const isUnavailable = brainDir === null;
  const [copyStatus, setCopyStatus] = useState<"idle" | "success" | "error">("idle");
  const [copyError, setCopyError] = useState<string | null>(null);

  async function handleCopy() {
    if (isUnavailable || !snippet) {
      return;
    }

    setCopyError(null);
    setCopyStatus("idle");

    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(snippet);
      } else {
        const textarea = document.createElement("textarea");
        textarea.value = snippet;
        textarea.style.position = "fixed";
        textarea.style.left = "-9999px";
        textarea.style.top = "0";
        document.body.appendChild(textarea);

        try {
          textarea.focus();
          textarea.select();

          if (typeof document.execCommand !== "function") {
            throw new Error("document.execCommand is unavailable");
          }

          const success = document.execCommand("copy");
          if (!success) {
            throw new Error("copy command failed");
          }
        } finally {
          document.body.removeChild(textarea);
        }
      }

      setCopyStatus("success");
    } catch (error) {
      console.error("Copy failed", error);
      setCopyError("Copy failed. Please select the text and copy manually.");
      setCopyStatus("error");
    }
  }

  return (
    <div className="settings-section">
      <h3>Developer / Agent Integration</h3>
      <p className="vault-hint">
        Paste this into your agent's MCP server configuration (Cursor, Claude
        Code, etc.) to connect it to your vault.
      </p>
      <div className="agent-snippet-wrapper">
        <pre>
          <code data-testid="agent-snippet">{snippet}</code>
        </pre>
        {brainDirError ? (
          <p className="agent-snippet-error" role="alert" aria-live="assertive">
            {brainDirError}
          </p>
        ) : null}
        {binaryPathError ? (
          <p className="agent-snippet-error" role="alert" aria-live="assertive">
            {binaryPathError}
          </p>
        ) : null}
        <button type="button" className="agent-snippet-copy" onClick={handleCopy} disabled={isUnavailable}>
          Copy
        </button>
        {copyStatus === "success" ? (
          <p className="agent-snippet-copy-success" role="status" aria-live="polite">
            Copied to clipboard.
          </p>
        ) : null}
        {copyError ? (
          <p className="agent-snippet-error" role="alert" aria-live="assertive">
            {copyError}
          </p>
        ) : null}
      </div>
    </div>
  );
}
