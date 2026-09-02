import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));

// The @tiptap/* packages arrive transitively via @blocknote/*; pnpm does not
// hoist them to node_modules/@tiptap/, so vite's node-style resolver cannot
// find them by bare specifier. Only the GHSA-cp6q-959q-f8rh regression test
// (src/__tests__/tiptapMergeAttributes.test.ts) imports @tiptap/* directly —
// no application source does — so these aliases live under `test.alias` and
// never reach the production bundle. Application code continues to resolve
// tiptap through BlockNote's own nested node_modules, exactly as before.
//
// Resolution is deliberately non-fatal. vite.config.ts is evaluated by every
// vite command (build, dev, vitest), so throwing here would break the
// production build on a fresh clone, in CI before install, or under a hoisted
// node-linker. A missing alias instead surfaces as an unresolved import in
// the single test that needs it, which is where the diagnostic belongs.
const TIPTAP_PACKAGES = [
  "core",
  "extension-bold",
  "extension-bubble-menu",
  "extension-code",
  "extension-floating-menu",
  "extension-italic",
  "extension-strike",
  "extension-text",
  "extension-underline",
  "extensions",
  "pm",
  "react",
];

/** Locate one @tiptap package in pnpm's store, or null if it is not there. */
function tiptapStorePath(name: string, version: string): string | null {
  try {
    const prefix = `@tiptap+${name}@${version}`;
    const match = readdirSync(resolve(__dirname, "node_modules/.pnpm")).find(
      (entry) => entry.startsWith(prefix),
    );
    return match
      ? resolve(
          __dirname,
          `node_modules/.pnpm/${match}/node_modules/@tiptap/${name}`,
        )
      : null;
  } catch {
    // No .pnpm store — fresh clone, or a non-pnpm/hoisted node-linker.
    return null;
  }
}

/**
 * Build the test-only @tiptap alias map. Versions come from package.json's
 * pnpm.overrides (the GHSA-cp6q-959q-f8rh security pin), so the aliases track
 * any future bump of that block without edits here.
 */
function tiptapTestAliases(): Record<string, string> {
  let overrides: Record<string, string>;
  try {
    const pkg = JSON.parse(
      readFileSync(resolve(__dirname, "package.json"), "utf8"),
    );
    overrides = pkg.pnpm?.overrides ?? {};
  } catch {
    return {};
  }

  const entries: [string, string][] = [];
  for (const name of TIPTAP_PACKAGES) {
    const version = overrides[`@tiptap/${name}`];
    if (!version) continue;
    const path = tiptapStorePath(name, version);
    if (path) entries.push([`@tiptap/${name}`, path]);
  }
  return Object.fromEntries(entries);
}

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    chunkSizeWarningLimit: 1500,
  },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
    alias: tiptapTestAliases(),
    exclude: ["**/node_modules/**", "**/dist/**", ".worktree/**", ".worktrees/**"],
  },
});
