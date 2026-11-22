import { execFileSync } from "node:child_process";
import path from "node:path";

export default function globalSetup() {
  const root = path.resolve(import.meta.dirname, "..");
  if (process.env.E2E_SKIP_BUILD === "1") return;
  execFileSync("vp", ["build"], { cwd: root, stdio: "inherit" });
  execFileSync("cargo", ["build", "--features", "fake", "--bin", "e2e-server"], {
    cwd: path.join(root, "src-tauri"),
    stdio: "inherit",
  });
}
