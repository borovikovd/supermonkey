// Checks COVERAGE.md against SPEC.md and the specs: every case line parses,
// ids are unique and sit in their file's group, every § is a SPEC heading,
// every case has a test in its file, every test id is a case, and the lists
// of what is covered elsewhere name only real cases and files. Runs in Node;
// no app.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";

import { expect, test } from "@playwright/test";

const DIR = import.meta.dirname;
const plan = readFileSync(path.join(DIR, "COVERAGE.md"), "utf8").split("\n");
const spec = readFileSync(path.join(DIR, "..", "SPEC.md"), "utf8").split("\n");

/** SPEC.md's headings, without their `#`s. */
const headings = new Set(spec.flatMap((l) => /^#{2,4} (.+)$/.exec(l)?.[1]?.trim() ?? []));

/** Lines under the `## title` heading, up to the next `## ` heading. */
function section(title: string): string[] {
  const start = plan.indexOf(`## ${title}`);
  expect(start, `COVERAGE.md has a "## ${title}" section`).toBeGreaterThan(-1);
  const end = plan.findIndex((l, i) => i > start && l.startsWith("## "));
  return plan.slice(start + 1, end === -1 ? undefined : end);
}

type Case = { id: string; file: string; sections: string[] };

/** `### file.spec.ts (PREFIX)` groups `- ID · Pn · title · §Section, …` lines. */
function cases(): Case[] {
  const out: Case[] = [];
  let file = "";
  let prefix = "";
  for (const line of section("Cases")) {
    if (line.trim() === "") continue;
    const group = /^### ([\w.-]+\.spec\.ts) \(([A-Z]+)\)$/.exec(line);
    if (group) {
      file = group[1] ?? "";
      prefix = group[2] ?? "";
      continue;
    }
    const m = /^- ([A-Z]+)-(\d+) · (P[123]) · (\S.*?) · (§.+)$/.exec(line);
    expect(m, `a case line is "- ID · Pn · title · §Section, …": ${line}`).not.toBeNull();
    if (!m) continue;
    expect(file, `${m[1]}-${m[2]} sits under a "### file (PREFIX)" group`).not.toBe("");
    expect(m[1], `${m[1]}-${m[2]} sits under the ${prefix} group`).toBe(prefix);
    const sections = (m[5] ?? "").split(/, (?=§)/).map((s) => s.slice(1));
    out.push({ id: `${m[1]}-${m[2]}`, file, sections });
  }
  return out;
}

/** Test ids per spec file, from `test("ID …"` titles (describe blocks excluded). */
function testIds(): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>();
  for (const file of readdirSync(DIR).filter((f) => f.endsWith(".spec.ts"))) {
    const src = readFileSync(path.join(DIR, file), "utf8");
    const ids = new Set<string>();
    for (const m of src.matchAll(/\btest(?:\.(?:fail|skip|fixme|only))?\(\s*["'`]([A-Z]+-\d+) /g)) {
      if (m[1]) ids.add(m[1]);
    }
    out.set(file, ids);
  }
  return out;
}

test("every case line parses, has a unique id, and names SPEC headings", () => {
  const all = cases();
  expect(all.length).toBeGreaterThan(0);
  const counts = new Map<string, number>();
  for (const c of all) counts.set(c.id, (counts.get(c.id) ?? 0) + 1);
  const repeated = [...counts].filter(([, n]) => n > 1).map(([id]) => id);
  expect(repeated, "ids used twice").toEqual([]);
  const unknown = all.flatMap((c) =>
    c.sections.filter((s) => !headings.has(s)).map((s) => `${c.id} §${s}`),
  );
  expect(unknown, "sections that are not SPEC.md headings").toEqual([]);
});

test("every case has a test in its file, and every test id is a case", () => {
  const ids = testIds();
  const all = cases();
  const missing = all.filter((c) => !ids.get(c.file)?.has(c.id)).map((c) => `${c.id} in ${c.file}`);
  expect(missing, "cases with no test").toEqual([]);

  const known = new Set(all.map((c) => `${c.file} ${c.id}`));
  const stray = [...ids].flatMap(([file, set]) =>
    [...set].filter((id) => !known.has(`${file} ${id}`)).map((id) => `${id} in ${file}`),
  );
  expect(stray, "tests whose id is not a case in their file").toEqual([]);
});

test("the lists of what is covered elsewhere name only real cases, files, and headings", () => {
  const all = cases();
  const known = new Set(all.map((c) => c.id));
  const prefixes = [...new Set(all.map((c) => c.id.split("-")[0]))].join("|");
  const lines = [
    ...section("Not covered end-to-end, and why"),
    ...section("Kept unit and property tests"),
  ];
  const ids = lines.flatMap((l) =>
    [...l.matchAll(new RegExp(`\\b((?:${prefixes})-\\d+)\\b`, "g"))].map((m) => m[1] ?? ""),
  );
  expect(ids.length).toBeGreaterThan(0);
  expect(
    ids.filter((id) => !known.has(id)),
    "case ids that are not cases",
  ).toEqual([]);

  const files = lines.flatMap((l) => [...l.matchAll(/`([\w./-]+\.rs)`/g)].map((m) => m[1] ?? ""));
  expect(files.length).toBeGreaterThan(0);
  const tauri = path.join(DIR, "..", "src-tauri");
  expect(
    files.filter((f) => !existsSync(path.join(tauri, f))),
    "files that are not under src-tauri/",
  ).toEqual([]);

  const named = lines.flatMap((l) =>
    [...l.matchAll(/§([A-Za-z][A-Za-z -]*[a-z])/g)].map((m) => m[1] ?? ""),
  );
  expect(
    named.filter((s) => !headings.has(s)),
    "sections that are not SPEC.md headings",
  ).toEqual([]);
});
