import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

import { expect, test } from "@playwright/test";

const SRC = path.join(import.meta.dirname, "..", "src");

function sources(): { file: string; text: string }[] {
  return readdirSync(SRC, { recursive: true, encoding: "utf8" })
    .filter((f) => /\.(ts|tsx)$/.test(f) && !f.startsWith(`bindings${path.sep}`))
    .map((f) => ({ file: f, text: readFileSync(path.join(SRC, f), "utf8") }));
}

function findings(pattern: RegExp): string[] {
  const out: string[] = [];
  for (const { file, text } of sources()) {
    text.split("\n").forEach((line, i) => {
      for (const m of line.matchAll(pattern)) out.push(`${file}:${i + 1}: ${m[0].trim()}`);
    });
  }
  return out;
}

const theme = readFileSync(path.join(SRC, "theme.css"), "utf8");

function tokenNames(kind: string): Set<string | undefined> {
  return new Set(
    [...theme.matchAll(new RegExp(`--${kind}-([\\w-]+?)(?:--[\\w-]+)?:`, "g"))].map((m) => m[1]),
  );
}

test("components use no color literals", () => {
  expect(findings(/#[0-9a-fA-F]{3,8}\b|\brgba?\(|\bhsla?\(/g)).toEqual([]);
});

test("components use no one-off Tailwind values", () => {
  expect(findings(/(?:^|[\s"'`{])!?(?:[\w-]+:)*-?[a-z][\w-]*-\[[^\]\s]+\]/g)).toEqual([]);
});

test("theme.css writes colors only in its tokens", () => {
  const outside = theme.replace(/@theme\s*\{[^}]*\}/, "");
  expect(outside.match(/#[0-9a-fA-F]{3,8}\b|\brgba?\([^)]*\d/g) ?? []).toEqual([]);
});

test("no color token shares a name with a text size", () => {
  const colors = tokenNames("color");
  expect([...tokenNames("text")].filter((n) => colors.has(n))).toEqual([]);
});
