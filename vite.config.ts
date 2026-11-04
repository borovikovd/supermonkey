import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig, lazyPlugins } from "vite-plus";

export default defineConfig({
  // Tauri expects a fixed port and does not need the browser opened.
  clearScreen: false,
  server: { port: 5173, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  // A desktop app loads its bundle from disk, so one large chunk is fine.
  build: { target: "safari17", outDir: "dist", chunkSizeWarningLimit: 2048 },

  fmt: {
    // release-please writes CHANGELOG.md and its manifest in its own format.
    ignorePatterns: [
      "src/bindings/**",
      "src-tauri/**",
      "SPEC.md",
      ".claude/**",
      "CHANGELOG.md",
      ".release-please-manifest.json",
    ],
  },

  lint: {
    plugins: ["react", "typescript", "oxc", "unicorn", "import", "jsx-a11y", "promise", "vitest"],
    ignorePatterns: ["src/bindings/**", "src-tauri/**", "dist/**", ".claude/**"],
    categories: {
      correctness: "error",
      suspicious: "error",
      perf: "error",
    },
    rules: {
      // React and the React Compiler's rules of React.
      "react/rules-of-hooks": "error",
      "react/exhaustive-deps": "error",
      "react/jsx-key": "error",
      "react/no-array-index-key": "error",
      // Off: this rule is for the legacy JSX transform; tsconfig uses the automatic
      // runtime ("jsx": "react-jsx"), so React is never in scope by design.
      "react/react-in-jsx-scope": "off",
      "react/no-danger": "error",
      "react/jsx-no-target-blank": "error",
      // Surface components the React Compiler skips instead of silently leaving them unmemoized.
      "react/todo": "error",
      "react/unsupported-syntax": "error",
      "react/rule-suppression": "error",

      // Type-aware TypeScript.
      "typescript/no-floating-promises": "error",
      "typescript/no-misused-promises": "error",
      "typescript/await-thenable": "error",
      "typescript/no-explicit-any": "error",
      "typescript/no-unsafe-argument": "error",
      "typescript/no-unsafe-assignment": "error",
      "typescript/no-unsafe-call": "error",
      "typescript/no-unsafe-member-access": "error",
      "typescript/no-unsafe-return": "error",
      "typescript/no-unsafe-function-type": "error",
      "typescript/no-empty-object-type": "error",
      "typescript/only-throw-error": "error",
      "typescript/prefer-promise-reject-errors": "error",
      "typescript/restrict-plus-operands": "error",
      "typescript/require-await": "error",
      "typescript/return-await": ["error", "in-try-catch"],
      "typescript/no-deprecated": "error",
      "typescript/use-unknown-in-catch-callback-variable": "error",
      "typescript/ban-ts-comment": "error",
      "typescript/no-non-null-assertion": "error",
      "typescript/no-unnecessary-type-assertion": "error",
      "typescript/consistent-type-imports": "error",
      "typescript/switch-exhaustiveness-check": "error",
      "typescript/no-unused-vars": ["error", { argsIgnorePattern: "^_", varsIgnorePattern: "^_" }],
      // `||` stays allowed on strings: an empty display name falls back to the address.
      "typescript/prefer-nullish-coalescing": ["error", { ignorePrimitives: { string: true } }],
      // Off: typescript-eslint recommends tsconfig's noImplicitReturns instead (on in
      // tsconfig.app.json); this rule misfires on React effects that return a cleanup
      // on some paths only.
      "typescript/consistent-return": "off",

      // General.
      // `x != null` is the idiomatic null-or-undefined check.
      eqeqeq: ["error", "always", { null: "ignore" }],
      "no-console": ["error", { allow: ["warn", "error"] }],
      "no-debugger": "error",
      "no-empty": "error",
      "no-var": "error",
      "prefer-const": "error",
      "no-throw-literal": "error",
      "array-callback-return": "error",
      "no-self-compare": "error",
      "no-param-reassign": "error",
      // A disable must name the rules it turns off.
      "unicorn/no-abusive-eslint-disable": "error",
      "import/no-cycle": "error",
      "import/no-duplicates": "error",
      // CSS is imported for its side effect.
      "import/no-unassigned-import": ["error", { allow: ["**/*.css"] }],
      "promise/no-return-in-finally": "error",
      "promise/always-return": ["error", { ignoreLastCallback: true }],
      "vite-plus/prefer-vite-plus-imports": "error",

      // Selected pedantic rules; the rest of that category is stylistic.
      "unicorn/prefer-at": "error",
      "unicorn/prefer-array-some": "error",
      "unicorn/prefer-query-selector": "error",
      "unicorn/prefer-code-point": "error",
      "unicorn/no-lonely-if": "error",

      // Accessibility.
      "jsx-a11y/click-events-have-key-events": "error",
      "jsx-a11y/no-static-element-interactions": "error",
      "jsx-a11y/no-autofocus": "error",
    },
    overrides: [
      {
        // Playwright fixtures: `({}, use)` is the required signature, and their
        // `use` callback is not React's `use` hook.
        files: ["e2e/**"],
        rules: { "no-empty-pattern": "off", "react/rules-of-hooks": "off" },
      },
    ],
    options: {
      typeAware: true,
      typeCheck: true,
      // Warnings fail the gate.
      denyWarnings: true,
      reportUnusedDisableDirectives: "error",
    },
    jsPlugins: [{ name: "vite-plus", specifier: "vite-plus/oxlint-plugin" }],
  },

  // `vp run <task>`. `gate` runs every check; run it before each commit.
  run: {
    // Off: the task cache's file tracking misses files the native tsc (TS 7)
    // reads, so a new file with a type error replayed a cached pass. Cargo
    // and Playwright keep their own caches anyway.
    cache: false,
    tasks: {
      // Also regenerates src/bindings (ts-rs), which the TypeScript checks read.
      "test:rust": { command: "cargo test --workspace -q", cwd: "src-tauri" },
      "lint:rust": {
        command: [
          "cargo fmt --all --check",
          "cargo clippy --workspace --all-targets -q -- -D warnings",
          "cargo clippy --workspace --all-targets --features fake -q -- -D warnings",
          "cargo doc --workspace --no-deps -q",
        ],
        cwd: "src-tauri",
      },
      "lint:ts": { command: "vp check", dependsOn: ["test:rust"] },
      typecheck: {
        command: [
          "tsc -p tsconfig.app.json --noEmit",
          "tsc -p tsconfig.node.json --noEmit",
          "tsc -p e2e --noEmit",
        ],
        dependsOn: ["test:rust"],
      },
      // Regenerated bindings must be staged or committed.
      bindings: {
        command: "git diff --exit-code --stat -- src/bindings",
        dependsOn: ["test:rust"],
      },
      // Builds the UI and the e2e-server first (playwright.config.ts).
      e2e: "playwright test",
      // Last and alone: the end-to-end suite is timing-sensitive under load.
      gate: {
        command: "vp run e2e",
        dependsOn: ["lint:rust", "lint:ts", "typecheck", "bindings"],
      },
    },
  },

  // Pre-commit: format and lint the staged files.
  staged: {
    "*.{ts,tsx,js,json,css,md,html}": "vp check --fix --no-error-on-unmatched-pattern",
    "*.rs": "rustfmt --edition 2024",
  },

  // The React Compiler, the Rust port (oxc-transform-react).
  plugins: lazyPlugins(() => [react({ compiler: true }), tailwindcss()]),
});
