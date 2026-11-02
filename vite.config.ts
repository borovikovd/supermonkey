import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig, lazyPlugins } from "vite-plus";

// Tauri expects a fixed port and does not need the browser opened.
export default defineConfig({
  clearScreen: false,
  server: { port: 5173, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  build: { target: "safari17", outDir: "dist" },
  fmt: {
    ignorePatterns: ["src/bindings/**", "src-tauri/**", "SPEC.md"],
  },
  lint: {
    plugins: ["react", "typescript", "oxc"],
    ignorePatterns: ["src/bindings/**", "src-tauri/**", "dist/**"],
    rules: {
      "react/rules-of-hooks": "error",
      "react/only-export-components": "off",
      "vite-plus/prefer-vite-plus-imports": "error",
    },
    options: {
      typeAware: true,
      typeCheck: true,
    },
    jsPlugins: [{ name: "vite-plus", specifier: "vite-plus/oxlint-plugin" }],
  },
  test: {
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    environment: "node",
  },
  plugins: lazyPlugins(() => [react(), tailwindcss()]),
});
