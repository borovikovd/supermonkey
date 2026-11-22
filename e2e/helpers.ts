import { expect } from "@playwright/test";

import type { Control, GmailMessage, GmailState } from "./control.ts";
import { A, type Seeded } from "./seeds.ts";

export async function inOrder<T>(
  items: readonly T[],
  step: (item: T, i: number) => Promise<unknown>,
): Promise<void> {
  await items.reduce<Promise<unknown>>(
    (prev, item, i) => prev.then(() => step(item, i)),
    Promise.resolve(),
  );
}

export function emails(header: string | undefined): string[] {
  return [...(header ?? "").matchAll(/[\w.+-]+@[\w-]+(?:\.[\w-]+)+/g)].map((m) =>
    m[0].toLowerCase(),
  );
}

export async function sentByApp(gmail: Control, account: string): Promise<GmailMessage[]> {
  const { messages } = await gmail.gmail(account);
  return messages.filter(
    (m) => m.labels.includes("SENT") && m.headers["x-supermonkey-draft"] !== undefined,
  );
}

export async function sentWithSubject(
  gmail: Control,
  account: string,
  subject: string,
): Promise<GmailMessage> {
  const found = (await sentByApp(gmail, account)).filter((m) => m.headers.subject === subject);
  expect(found, `one sent message "${subject}"`).toHaveLength(1);
  const [m] = found;
  if (!m) throw new Error(`no sent message ${subject}`);
  return m;
}

export function threadHas(state: GmailState, threadId: string, label: string): boolean {
  const msgs = state.messages.filter((m) => m.threadId === threadId && !m.labels.includes("DRAFT"));
  if (label === "TRASH" || label === "SPAM") return msgs.some((m) => m.labels.includes(label));
  return msgs.some(
    (m) => !m.labels.includes("TRASH") && !m.labels.includes("SPAM") && m.labels.includes(label),
  );
}

export async function has(
  gmail: Control,
  s: Seeded,
  key: string,
  label: string,
  account = A,
): Promise<boolean> {
  return threadHas(await gmail.gmail(account), s.thread(key).threadId, label);
}

export async function hasEach(
  gmail: Control,
  s: Seeded,
  keys: string[],
  label: string,
): Promise<boolean[]> {
  const state = await gmail.gmail(A);
  return keys.map((k) => threadHas(state, s.thread(k).threadId, label));
}

export const messagesOf = (state: GmailState, threadId: string): GmailMessage[] =>
  state.messages.filter((m) => m.threadId === threadId);

export const liveMessageHas = (state: GmailState, threadId: string, label: string): boolean =>
  messagesOf(state, threadId).some(
    (m) => m.labels.includes(label) && !m.labels.includes("TRASH") && !m.labels.includes("SPAM"),
  );

export const CONVERSATION_SUBS = `Object.fromEntries(Object.entries(window.__sm.useMirror.getState().subs)
  .filter(([, s]) => s.type === "conversation").map(([id, s]) => [String(s.key.thread), id]))`;
