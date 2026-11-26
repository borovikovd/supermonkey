import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

import { expect, test } from "vite-plus/test";

const root = path.resolve(import.meta.dirname, "..");

function names(file: string, section: "dependencies" | "peerDependencies"): string[] {
  const parsed: unknown = JSON.parse(readFileSync(file, "utf8"));
  if (typeof parsed !== "object" || parsed === null) return [];
  const listed: unknown = Reflect.get(parsed, section);
  return typeof listed === "object" && listed !== null ? Object.keys(listed) : [];
}

function sources(dir: string, exts: string[]): string[] {
  return readdirSync(dir, { withFileTypes: true, recursive: true })
    .filter((e) => e.isFile() && exts.includes(path.extname(e.name)))
    .map((e) => path.join(e.parentPath, e.name));
}

test("every UI dependency is imported by the UI, or is a peer of one that is", () => {
  const deps = names(path.join(root, "package.json"), "dependencies");
  const code = sources(path.join(root, "src"), [".ts", ".tsx", ".css"])
    .map((file) => readFileSync(file, "utf8"))
    .join("\n");
  const peers = new Set(
    deps.flatMap((dep) =>
      names(path.join(root, "node_modules", dep, "package.json"), "peerDependencies"),
    ),
  );
  const unused = deps.filter((name) => {
    const escaped = name.replaceAll(/[.*+?^${}()|[\]\\/]/g, "\\$&");
    return !peers.has(name) && !new RegExp(`["'\`]${escaped}(/[^"'\`]*)?["'\`]`).test(code);
  });
  expect(unused).toEqual([]);
});
