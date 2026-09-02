import js from "@eslint/js";
import tseslint from "typescript-eslint";
import react from "eslint-plugin-react";
import jsxA11y from "eslint-plugin-jsx-a11y";

export default [
  {
    ignores: ["node_modules", "dist", "build", "**/target", ".git", "pnpm-lock.yaml", ".worktree", ".worktrees"],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{js,jsx,ts,tsx}"],
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: {
        ecmaVersion: "latest",
        sourceType: "module",
        ecmaFeatures: {
          jsx: true,
        },
      },
    },
    plugins: {
      react,
    },
    rules: {
      "react/react-in-jsx-scope": "off",
      "react/prop-types": "off",
      "no-empty": ["error", { allowEmptyCatch: true }],
    },
  },
  {
    files: ["**/*.{js,jsx,ts,tsx}"],
    plugins: { "jsx-a11y": jsxA11y },
    rules: {
      ...jsxA11y.configs.recommended.rules,
      "jsx-a11y/aria-role": ["error", { ignoreNonDOM: true }],
      // `prefer-tag-over-role` is intentionally NOT enabled: it errors on the
      // spec-mandated ARIA patterns this codebase relies on — role="dialog"
      // surfaces with the custom focus trap, and CommandPalette's
      // listbox/option combobox per WAI-ARIA APG (a native <select> cannot
      // express it). Native-tag migration is phase 2–6 ledger work; revisit
      // then. (CodeRabbit suggestion, evaluated and deferred 2026-09-01.)
    },
  },
  {
    files: ["scripts/**/*.{js,cjs}"],
    languageOptions: {
      globals: {
        require: "readonly",
        module: "readonly",
        process: "readonly",
        __dirname: "readonly",
        console: "readonly",
      },
    },
    rules: {
      "@typescript-eslint/no-require-imports": "off",
      "no-undef": "off",
    },
  },
  {
    files: ["**/tests/fixtures/**/*.{js,ts}"],
    rules: {
      "@typescript-eslint/no-unused-vars": "off",
    },
  },
];
