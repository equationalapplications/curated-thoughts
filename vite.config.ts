import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { readdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// The @tiptap/* packages arrive transitively via @blocknote/*; pnpm does not
// hoist them to node_modules/@tiptap/, so vite's node-style resolver cannot
// find them. The security regression test imports @tiptap/core directly, so
// we alias each @tiptap/* specifier to the real package directory inside
// pnpm's content-addressed store. The lookup reads the directory listing at
// config-load time, so it survives the 3.30.2 → 3.30.6 override change.
const __dirname = dirname(fileURLToPath(import.meta.url));
const tiptapPackages = [
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

function tiptapAlias(name: string): string {
  const prefix = `@tiptap+${name}@`;
  const match = readdirSync(resolve(__dirname, "node_modules/.pnpm")).find(
    (entry) => entry.startsWith(prefix),
  );
  if (!match) {
    throw new Error(
      `@tiptap/${name} not found in node_modules/.pnpm — run \`pnpm install\``,
    );
  }
  return resolve(
    __dirname,
    `node_modules/.pnpm/${match}/node_modules/@tiptap/${name}`,
  );
}

const tiptapAliases = Object.fromEntries(
  tiptapPackages.map((name) => [`@tiptap/${name}`, tiptapAlias(name)]),
);

export default defineConfig({
  plugins: [react()],
  resolve: { alias: tiptapAliases },
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    chunkSizeWarningLimit: 1500,
  },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
    exclude: ["**/node_modules/**", "**/dist/**", ".worktree/**", ".worktrees/**"],
  },
});
