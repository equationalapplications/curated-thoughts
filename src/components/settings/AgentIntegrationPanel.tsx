import { useMemo, useState } from "react";

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
  const snippet = useMemo(
    () =>
      brainDir === null
        ? ""
        : JSON.stringify(
            {
              mcpServers: {
                "curated-thoughts": {
                  command: binaryPath(),
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
    [brainDir],
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
        textarea.focus();
        textarea.select();

        const success = document.execCommand("copy");
        document.body.removeChild(textarea);

        if (!success) {
          throw new Error("copy command failed");
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
