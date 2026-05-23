import { useMemo } from "react";

interface Props {
  brainDir: string;
}

function binaryPath(): string {
  const p = typeof navigator !== "undefined" ? (navigator.platform ?? "") : "";
  if (/Win/i.test(p)) {
    return "C:\\Program Files\\CuratedThoughts\\curated-thoughts.exe";
  }
  if (/Linux/i.test(p)) {
    return "/usr/bin/curated-thoughts";
  }
  // macOS default
  return "/Applications/CuratedThoughts.app/Contents/MacOS/curated-thoughts";
}

export function AgentIntegrationPanel({ brainDir }: Props) {
  const snippet = useMemo(
    () =>
      JSON.stringify(
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

  function handleCopy() {
    navigator.clipboard.writeText(snippet).catch(() => {});
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
          <code role="code">{snippet}</code>
        </pre>
        <button type="button" className="agent-snippet-copy" onClick={handleCopy}>
          Copy
        </button>
      </div>
    </div>
  );
}
