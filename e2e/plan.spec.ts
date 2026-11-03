// Checks COVERAGE.md against itself and the specs: the counts table matches
// the case headings, every case has a test in its file, every test id is a
// case, and the traceability tables name only real cases. Runs in Node; no app.
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

import { expect, test } from "@playwright/test";

const DIR = import.meta.dirname;
const plan = readFileSync(path.join(DIR, "COVERAGE.md"), "utf8").split("\n");

/** Lines from the `## N.` heading up to the next `## ` heading. */
function section(n: number): string[] {
  const start = plan.findIndex((l) => l.startsWith(`## ${n}. `));
  expect(start, `COVERAGE.md has a section ${n}`).toBeGreaterThan(-1);
  const end = plan.findIndex((l, i) => i > start && l.startsWith("## "));
  return plan.slice(start + 1, end === -1 ? undefined : end);
}

type Case = { id: string; priority: string; file: string };

/** Section 5: `### 5.N `file` (PREFIX)` groups `#### ID · Pn · title` cases. */
function cases(): Case[] {
  const out: Case[] = [];
  let file = "";
  let prefix = "";
  for (const line of section(5)) {
    const group = /^### 5\.\d+ `([\w.-]+)` \((\w+)\)/.exec(line);
    if (group) {
      file = group[1] ?? "";
      prefix = group[2] ?? "";
      continue;
    }
    if (!line.startsWith("#### ")) continue;
    const m = /^#### ([A-Z]+)-(\d+) · (P[123]) · \S/.exec(line);
    expect(m, `case heading is "#### ID · Pn · title": ${line}`).not.toBeNull();
    if (!m) continue;
    expect(m[1], `${m[1]}-${m[2]} sits under the ${prefix} group`).toBe(prefix);
    out.push({ id: `${m[1]}-${m[2]}`, priority: m[3] ?? "", file });
  }
  return out;
}

/** Table cells of a markdown row, trimmed, without bold markers. */
function cells(row: string): string[] {
  return row
    .split("|")
    .slice(1, -1)
    .map((c) => c.trim().replaceAll("**", ""));
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

test("the counts table matches the case headings", () => {
  // [cases, P1, P2, P3] per file, and the total.
  const counted: Record<string, number[]> = { Total: [0, 0, 0, 0] };
  for (const c of cases()) {
    counted[c.file] ??= [0, 0, 0, 0];
    for (const row of [counted[c.file], counted["Total"]]) {
      for (const i of [0, Number(c.priority.slice(1))]) {
        if (row) row[i] = (row[i] ?? 0) + 1;
      }
    }
  }

  const table: Record<string, number[]> = {};
  for (const line of section(4)) {
    const c = cells(line);
    const name = c[0]?.replaceAll("`", "");
    if (!name || !(name.endsWith(".spec.ts") || name === "Total")) continue;
    table[name] = c.slice(5, 9).map(Number);
  }
  // [cases, P1, P2, P3] per file. On a mismatch, copy the counted values.
  expect(table).toEqual(counted);
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

test("the traceability tables name only real cases", () => {
  const known = new Set(cases().map((c) => c.id));
  const named = [...section(6), ...section(7)].flatMap((l) =>
    l.startsWith("|") ? [...l.matchAll(/\b([A-Z]{2,4}-\d+)\b/g)].map((m) => m[1] ?? "") : [],
  );
  expect(named.length).toBeGreaterThan(0);
  expect(named.filter((id) => !known.has(id))).toEqual([]);
});
