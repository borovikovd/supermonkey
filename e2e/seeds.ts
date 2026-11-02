// Seed scenarios (e2e/COVERAGE.md, section 2). Each builds fake-Gmail state
// through the control API, then opens the app and waits for the connect sync
// to settle. Each returns handles: threads by key, label ids, and drafts.
//
//   const s = await seeds.standard(gmail, app);
//   await app.row(s.thread("p1").subject).click();
//   await gmail.modify(A, s.message("plan", 12).id, [], ["UNREAD"], { poll: true });

import type { App } from "./app.ts";
import type { AddedDraft, Control, Delivered, Delivery, DraftSpec } from "./control.ts";
import { attachment, files, pixelDataUrl } from "./files.ts";

// ------------------------------------------------------------------ time

/** The time zone both the core and the browser run in. */
export const TZ = "America/New_York";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** Monday 2026-11-02 10:00 in New York (EST, UTC−5): app time when every test starts. */
export const ANCHOR = Date.parse("2026-11-02T10:00:00-05:00");

/** Times relative to the anchor. DST ended on 1 November, so the test week is all EST. */
export const dates = {
  anchor: ANCHOR,
  minutesAgo: (n: number) => ANCHOR - n * MINUTE,
  hoursAgo: (n: number) => ANCHOR - n * HOUR,
  daysAgo: (n: number) => ANCHOR - n * DAY,
  /** A New York wall time in the test week, e.g. `at("2026-11-02T18:00")`. */
  at: (local: string) => Date.parse(`${local}-05:00`),
};

// ------------------------------------------------------------------ identities

export const A = "ana@acme.test";
export const B = "ben@bolt.test";
export const C = "cat@cove.test";
export const A_ALIAS = "ana@ames.dev";

export type Identity = "A" | "B" | "C";

const IDENTITIES: Record<Identity, { email: string; name: string }> = {
  A: { email: A, name: "Ana Ames" },
  // Differs from the address, so a test can see the display name comes from sendAs.
  B: { email: B, name: "Zed Bolt" },
  C: { email: C, name: "Cat Cove" },
};

export type SeedOptions = {
  /** Which seeded identities are already signed in. Default: all the scenario names. */
  connect?: Identity[];
  /** Start the app and wait for the connect sync. Default true. */
  open?: boolean;
};

/** How long a seed waits for the connect sync, by scenario size. */
const OPEN_TIMEOUT = { normal: 10_000, huge: 60_000 };

// ------------------------------------------------------------------ handles

/** A seeded thread: its Gmail thread id, subject, and messages oldest first. */
export type Thread = {
  key: string;
  account: string;
  threadId: string;
  subject: string;
  messages: Delivered[];
};

/** What a scenario seeded, by key. Lookups throw on an unknown key. */
export class Seeded {
  readonly threads = new Map<string, Thread>();
  readonly labels = new Map<string, string>();
  readonly drafts = new Map<string, AddedDraft>();

  thread(key: string): Thread {
    const t = this.threads.get(key);
    if (!t) throw new Error(`no seeded thread ${key}`);
    return t;
  }

  /** The nth message of a thread, counted from 1. */
  message(key: string, n: number): Delivered {
    const m = this.thread(key).messages[n - 1];
    if (!m) throw new Error(`thread ${key} has no message ${n}`);
    return m;
  }

  /** The latest message of a thread. */
  latest(key: string): Delivered {
    return this.message(key, this.thread(key).messages.length);
  }

  /** A user label's Gmail id. */
  label(account: string, name: string): string {
    const id = this.labels.get(`${account}/${name}`);
    if (!id) throw new Error(`no seeded label ${name} on ${account}`);
    return id;
  }

  draft(key: string): AddedDraft {
    const d = this.drafts.get(key);
    if (!d) throw new Error(`no seeded draft ${key}`);
    return d;
  }
}

/** One message of a seeded thread; the account, thread, and reply headers are filled in. */
type Msg = Omit<Delivery, "account" | "threadId" | "poll" | "subject" | "after"> & {
  subject?: string;
};

/** Runs steps one after another, each waiting for the one before. */
async function inOrder(steps: (() => Promise<unknown>)[]): Promise<void> {
  await steps.reduce<Promise<unknown>>((prev, step) => prev.then(step), Promise.resolve());
}

/**
 * Queues a scenario's mail and delivers it in one batch, so a seed costs one
 * round trip whatever its size.
 */
class Builder {
  readonly seeded = new Seeded();
  readonly gmail: Control;
  private readonly batch: Delivery[] = [];
  private readonly queued: { key: string; account: string; subject: string; items: number[] }[] =
    [];

  constructor(gmail: Control) {
    this.gmail = gmail;
  }

  async identity(id: Identity, isSignedIn: boolean): Promise<void> {
    const { email, name } = IDENTITIES[id];
    if (isSignedIn) await this.gmail.account(email, name);
    else await this.gmail.user(email, name);
  }

  async labels(account: string, names: string[]): Promise<void> {
    const ids = await Promise.all(names.map((name) => this.gmail.label(account, name)));
    for (const [i, name] of names.entries()) {
      const id = ids[i]?.id;
      if (id) this.seeded.labels.set(`${account}/${name}`, id);
    }
  }

  label(account: string, name: string): string {
    return this.seeded.label(account, name);
  }

  /** Queues a thread: each message replies to the one before it. */
  thread(key: string, account: string, subject: string, msgs: Msg[]): void {
    const items = msgs.map((m, i) => {
      const at = this.batch.length;
      this.batch.push({
        ...m,
        account,
        subject: m.subject ?? (i === 0 ? subject : `Re: ${subject}`),
        poll: false,
        ...(i > 0 ? { after: at - 1 } : {}),
      });
      return at;
    });
    this.queued.push({ key, account, subject, items });
  }

  /** Delivers everything queued, then records the threads. */
  async deliver(): Promise<void> {
    const delivered = await this.gmail.deliverMany(this.batch.splice(0));
    for (const q of this.queued.splice(0)) {
      const messages = q.items.map((i) => {
        const d = delivered[i];
        if (!d) throw new Error(`seed ${q.key}: message ${i} was not delivered`);
        return d;
      });
      const threadId = messages[0]?.threadId ?? "";
      this.seeded.threads.set(q.key, {
        key: q.key,
        account: q.account,
        threadId,
        subject: q.subject,
        messages,
      });
    }
  }

  /** Gmail drafts made by another client, created in order. */
  async drafts(account: string, list: [string, DraftSpec][]): Promise<void> {
    await inOrder(
      list.map(([key, d]) => async () => {
        this.seeded.drafts.set(key, await this.gmail.createDraft(account, d));
      }),
    );
  }
}

/** Inbox labels for a received personal message. */
const inbox = (unread = false) => ["INBOX", "CATEGORY_PERSONAL", ...(unread ? ["UNREAD"] : [])];

/** Two digits, for names that sort as they are numbered. */
const two = (i: number) => String(i).padStart(2, "0");

const ANA = "Ana Ames <ana@acme.test>";
const P = {
  carol: "Carol Chen <carol@corp.test>",
  dave: "Dave Diaz <dave@corp.test>",
  eve: "Eve Evans <eve@corp.test>",
  ivy: "Ivy Ito <ivy@help.test>",
  gus: "Gus Grant <gus@guild.test>",
  kim: "Kim Kato <kim@kite.test>",
  alice: "Alice Adams <alice@law.test>",
  lou: "Lou Lane <lou@site.test>",
  mia: "Mia Moss <mia@m.test>",
  ned: "Ned Nolan <ned@n.test>",
};

/** `p1`…`p6`: one message each, from these people. */
export const PERSONS = [
  "Pia Park <pia@people.test>",
  "Quin Quill <quin@people.test>",
  "Rae Reed <rae@people.test>",
  "Sol Snow <sol@people.test>",
  "Tom Tate <tom@people.test>",
  "Uma Ueda <uma@people.test>",
];

async function finish(
  b: Builder,
  app: App,
  opts: SeedOptions,
  timeout = OPEN_TIMEOUT.normal,
): Promise<Seeded> {
  if (opts.open ?? true) await app.open({ timeout });
  return b.seeded;
}

function signedIn(opts: SeedOptions, id: Identity, byDefault: Identity[]): boolean {
  return (opts.connect ?? byDefault).includes(id);
}

// ------------------------------------------------------------------ standard

/** Queues A's Inbox (COVERAGE `standard`). Rows from newest: p1…p6, plan, … trashedLatest. */
function queueInbox(b: Builder): void {
  for (const [i, from] of PERSONS.entries()) {
    const n = i + 1;
    const first = from.split(" ")[0] ?? from;
    b.thread(`p${n}`, A, `Person ${n}`, [
      {
        from,
        to: [A],
        text: n === 3 ? "I saw a quokka at the zoo today." : `Hello from ${first}.`,
        labels: inbox(n % 2 === 1),
        date: dates.minutesAgo(10 * n),
      },
    ]);
  }
  const planFrom = [
    P.carol,
    P.dave,
    P.carol,
    ANA,
    P.dave,
    P.carol,
    P.dave,
    ANA,
    P.carol,
    P.dave,
    P.carol,
    P.dave,
  ];
  b.thread(
    "plan",
    A,
    "Q4 plan",
    planFrom.map((from, i) => {
      const k = i + 1;
      const own = from === ANA;
      return {
        from,
        to: own ? ["carol@corp.test", "dave@corp.test"] : [A],
        text: `Plan message ${k}.`,
        labels: own ? ["SENT"] : inbox(k >= 10),
        date: dates.hoursAgo(2 + (12 - k) * 4),
      };
    }),
  );
  b.thread(
    "fold",
    A,
    "Fold check",
    Array.from({ length: 11 }, (_, i) => ({
      from: P.eve,
      to: [A],
      text: `Fold message ${i + 1}.`,
      labels: inbox(i + 1 === 5),
      date: dates.hoursAgo(3 + (10 - i) * 2),
    })),
  );
  b.thread("addr", A, "Offsite venue", [
    {
      from: P.carol,
      to: [A, "dave@corp.test"],
      cc: ["erin@corp.test"],
      text: "Which venue should we book for the offsite?",
      labels: inbox(true),
      date: dates.hoursAgo(4),
    },
  ]);
  b.thread("replyto", A, "Ticket #42", [
    {
      from: P.ivy,
      to: [A],
      cc: ["jo@help.test"],
      headers: [["Reply-To", "tickets@help.test"]],
      text: "Your ticket is open.",
      labels: inbox(),
      date: dates.hoursAgo(5),
    },
  ]);
  b.thread("alias", A, "Guild meetup", [
    {
      from: P.gus,
      to: [A_ALIAS, "hal@guild.test"],
      text: "The guild meets on Thursday.",
      labels: inbox(),
      date: dates.hoursAgo(6),
    },
  ]);
  b.thread("own", A, "Budget draft", [
    {
      from: P.kim,
      to: [A],
      text: "Here is the budget draft.",
      labels: inbox(),
      date: dates.hoursAgo(8),
    },
    {
      from: ANA,
      to: [P.kim],
      cc: ["lee@kite.test"],
      text: "Thanks, looks fine.",
      labels: ["SENT"],
      date: dates.hoursAgo(7),
    },
  ]);
  b.thread("ownAlias", A, "Alias note", [
    {
      from: P.kim,
      to: [A_ALIAS],
      text: "A note for your alias.",
      labels: inbox(),
      date: dates.hoursAgo(9),
    },
    {
      from: `Ana Ames <${A_ALIAS}>`,
      to: [P.kim],
      text: "Noted.",
      labels: ["SENT"],
      date: dates.hoursAgo(8.5),
    },
  ]);
  b.thread("fwd", A, "Contract", [
    {
      from: P.alice,
      to: [A],
      html:
        '<p>Please review the contract.</p><p><img src="cid:logo" alt="logo"></p>' +
        '<div class="gmail_quote">On Fri, Oct 30, 2026 at 9:00 AM Bob Brand &lt;bob@law.test&gt; wrote:' +
        "<blockquote>The first draft of the contract.</blockquote></div>",
      attachments: [attachment(files.logoPng, "logo"), attachment(files.contractPdf)],
      labels: inbox(),
      date: dates.hoursAgo(10),
    },
  ]);
  const url = b.gmail.url;
  b.thread("links", A, "Links", [
    {
      from: P.lou,
      to: [A],
      html:
        '<p><a href="https://example.org/a">Link A</a></p>' +
        '<p><a href="mailto:x@example.org">Mail X</a></p>' +
        '<p><a href="https://example.org/b">Link B</a></p>' +
        '<p><a href="https://example.org/top" target="_top">Top link</a></p>' +
        `<p><img src="${url}/img/pixel.png" alt="remote pixel"></p>` +
        `<div style="background-image:url(${url}/img/bg.png);width:8px;height:8px"></div>` +
        `<p><img src="${pixelDataUrl}" alt="data pixel"></p>`,
      labels: inbox(),
      date: dates.hoursAgo(11),
    },
  ]);
  b.thread("trashedLatest", A, "Latest rule", [
    { from: P.mia, to: [A], text: "The first word.", labels: inbox(), date: dates.daysAgo(1) },
    { from: P.ned, to: [A], text: "A trashed reply.", labels: ["TRASH"], date: dates.hoursAgo(20) },
  ]);

  // Other: list mail and categories.
  const unsubscribe: [string, string] = ["List-Unsubscribe", "<mailto:leave@letters.test>"];
  const news = (
    key: string,
    subject: string,
    from: string,
    labels: string[],
    hours: number,
    list: Partial<Msg>,
  ) =>
    b.thread(key, A, subject, [
      {
        from,
        to: ["subscribers@letters.test"],
        text: `${subject}.`,
        labels,
        date: dates.hoursAgo(hours),
        ...list,
      },
    ]);
  news(
    "news1",
    "Digest: week 44",
    "Letters <news@letters.test>",
    ["INBOX", "UNREAD", "CATEGORY_UPDATES"],
    26,
    {
      headers: [unsubscribe],
    },
  );
  news(
    "news2",
    "Digest: daily",
    "Letters Daily <digest@letters.test>",
    ["INBOX", "UNREAD", "CATEGORY_PERSONAL"],
    30,
    {
      listId: "<daily.letters.test>",
    },
  );
  news(
    "news3",
    "Digest: week 43",
    "Letters <news@letters.test>",
    ["INBOX", "UNREAD", "CATEGORY_UPDATES"],
    48,
    {
      headers: [unsubscribe],
    },
  );
  const other = (key: string, subject: string, from: string, labels: string[], hours: number) =>
    b.thread(key, A, subject, [
      { from, to: [A], text: `${subject}.`, labels, date: dates.hoursAgo(hours) },
    ]);
  other("promo", "Big sale", "Shop Deals <deals@shop.test>", ["INBOX", "CATEGORY_PROMOTIONS"], 54);
  other("social", "New follower", "Social <notify@social.test>", ["INBOX", "CATEGORY_SOCIAL"], 72);
  other("forum", "Forum thread", "Forum <group@forum.test>", ["INBOX", "CATEGORY_FORUMS"], 80);
  other(
    "receipt",
    "Your receipt",
    "Shop Billing <billing@shop.test>",
    ["INBOX", "CATEGORY_PROMOTIONS", b.label(A, "Receipts")],
    96,
  );
}

/** Queues A's mail outside Inbox. */
function queueArchive(b: Builder): void {
  const old = (
    key: string,
    subject: string,
    from: string,
    text: string,
    labels: string[],
    days: number,
  ) => b.thread(key, A, subject, [{ from, to: [A], text, labels, date: dates.daysAgo(days) }]);
  old(
    "starredOld",
    "Trip ideas",
    "Tara Trip <tara@trip.test>",
    "Some places to visit.",
    ["STARRED", "CATEGORY_PERSONAL", b.label(A, "Travel")],
    30,
  );
  for (const days of [10, 89, 91]) {
    b.thread(`sent${days}`, A, `Sent ${days} days ago`, [
      {
        from: ANA,
        to: [P.kim],
        text: `Written ${days} days ago.`,
        labels: ["SENT"],
        date: dates.daysAgo(days),
      },
    ]);
  }
  old(
    "archivedOld",
    "Archived before",
    "Arlo Arch <arlo@arch.test>",
    "Filed away.",
    ["CATEGORY_PERSONAL"],
    20,
  );
  for (const [i, name] of ["alpha", "beta", "gamma"].entries()) {
    old(
      `projOld${i + 1}`,
      `Project ${name}`,
      "Pat Proj <pat@proj.test>",
      `Notes on project ${name}.`,
      ["CATEGORY_PERSONAL", b.label(A, "Projects")],
      200 + i,
    );
  }
  old("trashOld", "Old trash", "Tim Trash <tim@trash.test>", "A wombat was here.", ["TRASH"], 40);
  old("spamOld", "Old spam", "Spammer <spam@junk.test>", "You won a prize.", ["SPAM"], 40);
  old(
    "invoiceOld",
    "Invoice 2019",
    "Old Billing <billing@old.test>",
    "Your invoice from 2019.",
    ["CATEGORY_PERSONAL"],
    400,
  );
}

/** Queues B's and C's mail. */
function queueOthers(b: Builder): void {
  const bMail: [string, string, boolean, number, string[]][] = [
    ["b1", "Bea Blue <bea@blue.test>", true, 15, []],
    ["b2", "Bo Brown <bo@brown.test>", true, 25, []],
    ["b3", "Bill Black <bill@black.test>", false, 180, []],
    ["b4", "Bree Bell <bree@bell.test>", false, 300, [b.label(B, "Family")]],
  ];
  for (const [key, from, unread, minutes, extra] of bMail) {
    b.thread(key, B, `B ${key.slice(1)}`, [
      {
        from,
        to: [B],
        text: `Mail ${key} for Ben.`,
        labels: [...inbox(unread), ...extra],
        date: dates.minutesAgo(minutes),
      },
    ]);
  }
  for (const n of [1, 2, 3]) {
    b.thread(`c${n}`, C, `C ${n}`, [
      {
        from: `Cy ${n} <cy${n}@cove.test>`,
        to: [C],
        text: `Mail ${n} for Cat.`,
        labels: inbox(true),
        date: dates.minutesAgo(5 * n),
      },
    ]);
  }
}

/** A's, B's, and C's mail (COVERAGE `standard`), without opening the app. */
async function standardMail(b: Builder, opts: SeedOptions, byDefault: Identity[]): Promise<void> {
  await b.identity("A", signedIn(opts, "A", byDefault));
  await b.identity("B", signedIn(opts, "B", byDefault));
  await b.identity("C", signedIn(opts, "C", byDefault));
  await b.gmail.sendAs(A, A_ALIAS, "Ana Ames");
  await b.labels(A, ["Receipts", "Travel", "Projects", "Work/Clients"]);
  await b.labels(B, ["Family"]);
  queueInbox(b);
  queueArchive(b);
  queueOthers(b);
  await b.deliver();
  // B's `collide` shares p1's Gmail thread id: rows are keyed by account.
  const collide = await b.gmail.deliver({
    account: B,
    from: "Cole Lide <cole@collide.test>",
    to: [B],
    subject: "B collide",
    text: "Same thread id as Ana's p1.",
    labels: inbox(),
    threadId: b.seeded.thread("p1").threadId,
    date: dates.minutesAgo(360),
    poll: false,
  });
  b.seeded.threads.set("collide", {
    key: "collide",
    account: B,
    threadId: collide.threadId,
    subject: "B collide",
    messages: [collide],
  });
}

/** The first-run screen: A, B, and C exist with `standard` mail; no account is signed in. */
export async function none(gmail: Control, app: App, opts: SeedOptions = {}): Promise<Seeded> {
  const b = new Builder(gmail);
  await standardMail(b, opts, []);
  return finish(b, app, opts);
}

/** A signed in with no mail and only system labels. */
export async function empty(gmail: Control, app: App, opts: SeedOptions = {}): Promise<Seeded> {
  const b = new Builder(gmail);
  await b.identity("A", signedIn(opts, "A", ["A"]));
  return finish(b, app, opts);
}

/** A and B signed in; C only in Gmail (COVERAGE `standard`). */
export async function standard(gmail: Control, app: App, opts: SeedOptions = {}): Promise<Seeded> {
  const b = new Builder(gmail);
  await standardMail(b, opts, ["A", "B"]);
  return finish(b, app, opts);
}

/** `standard`, plus Gmail-made drafts on A. */
export async function drafts(gmail: Control, app: App, opts: SeedOptions = {}): Promise<Seeded> {
  const b = new Builder(gmail);
  await standardMail(b, opts, ["A", "B"]);
  const s = b.seeded;
  const p2 = s.thread("p2").threadId;
  await b.drafts(A, [
    [
      "dNew",
      {
        subject: "Idea",
        to: [P.kim],
        html: "<p>An idea for later.</p>",
        date: dates.minutesAgo(90),
      },
    ],
    [
      "dFwd",
      {
        subject: "Fwd: Contract",
        threadId: s.thread("fwd").threadId,
        inReplyTo: s.message("fwd", 1).messageId,
        html: "<p>FYI</p>",
        date: dates.minutesAgo(80),
      },
    ],
    [
      "dLatest",
      {
        subject: "Re: Person 4",
        to: [PERSONS[3] ?? ""],
        threadId: s.thread("p4").threadId,
        inReplyTo: s.latest("p4").messageId,
        html:
          "<p>My text</p>" +
          '<div class="gmail_quote">On Mon, Nov 2, 2026 at 9:20 AM Sol Snow &lt;sol@people.test&gt; wrote:' +
          "<blockquote>Hello from Sol.</blockquote></div>",
        attachments: [attachment(files.specPdf)],
        date: dates.minutesAgo(70),
      },
    ],
    [
      "dEarly",
      {
        subject: "Re: Q4 plan",
        to: [P.carol],
        threadId: s.thread("plan").threadId,
        inReplyTo: s.message("plan", 3).messageId,
        html: "<p>About message three.</p>",
        date: dates.minutesAgo(60),
      },
    ],
    [
      "dUnknown",
      {
        subject: "Re: Person 6",
        to: [PERSONS[5] ?? ""],
        threadId: s.thread("p6").threadId,
        inReplyTo: "<nobody@x.test>",
        html: "<p>Aimed at nobody.</p>",
        date: dates.minutesAgo(50),
      },
    ],
    ...(["dTwoA", "dTwoB"] as const).map((key, i): [string, DraftSpec] => [
      key,
      {
        subject: "Re: Person 2",
        to: [PERSONS[1] ?? ""],
        threadId: p2,
        inReplyTo: s.latest("p2").messageId,
        html: `<p>${key}</p>`,
        // dTwoB is the newer one.
        date: dates.minutesAgo(40 - 10 * i),
      },
    ]),
  ]);
  return finish(b, app, opts);
}

/** A signed in, with mail built to exercise autocomplete (COVERAGE `contacts`). */
export async function contacts(gmail: Control, app: App, opts: SeedOptions = {}): Promise<Seeded> {
  const b = new Builder(gmail);
  await b.identity("A", signedIn(opts, "A", ["A"]));
  await gmail.sendAs(A, A_ALIAS, "Ana Ames");
  const sent = (key: string, to: string, days: number) =>
    b.thread(key, A, `To ${to}`, [
      { from: ANA, to: [to], text: "Hi.", labels: ["SENT"], date: dates.daysAgo(days) },
    ]);
  const received = (key: string, m: Msg & { subject: string }) =>
    b.thread(key, A, m.subject, [{ to: [A], text: "Hello.", labels: inbox(), ...m }]);

  for (const [i, days] of [2, 5, 10, 15, 20].entries())
    sent(`sam${i + 1}`, "Sam Stone <sam@x.test>", days);
  // Outside the 90-day sync: found only by the contacts backfill.
  sent("sara", "Sara Swift <sara@x.test>", 200);
  received("sue1", {
    from: "Sue Sender <sue@recv.test>",
    subject: "From Sue",
    date: dates.daysAgo(3),
  });
  received("sue2", {
    from: "Sue Sender <sue@recv.test>",
    subject: "Sue again",
    date: dates.daysAgo(4),
  });
  received("sid", {
    from: "Rob Root <rob@root.test>",
    cc: ["Sid Copy <sid@cc.test>"],
    subject: "Copying Sid",
    date: dates.daysAgo(5),
  });
  received("sly", {
    from: "List Bot <bot@list.test>",
    to: ["Sly List <sly@list.test>"],
    cc: ["Sly List <sly@list.test>"],
    listId: "<people.list.test>",
    subject: "List mail",
    date: dates.daysAgo(6),
  });
  received("noreply", {
    from: "Svc <noreply@svc.test>",
    subject: "Service notice",
    date: dates.daysAgo(7),
  });
  received("noReply", {
    from: "Svc <no-reply@svc.test>",
    subject: "Service notice 2",
    date: dates.daysAgo(8),
  });
  received("spammy", {
    from: "Spam Sam <spammy@spam.test>",
    subject: "Spam",
    labels: ["SPAM"],
    date: dates.daysAgo(9),
  });
  received("alias", {
    from: "Ola Owl <ola@owl.test>",
    to: [A_ALIAS],
    subject: "To the alias",
    date: dates.daysAgo(10),
  });
  // Eight "Mark M…" contacts A sent to: more than the six suggestions shown.
  const marks = ["Maddox", "Malone", "Mann", "Marsh", "Mayer", "Meyer", "Miles", "Moore"];
  for (const [i, last] of marks.entries()) {
    sent(`mark${i + 1}`, `Mark ${last} <${last.toLowerCase()}@marks.test>`, 3 + i);
  }
  await b.deliver();
  await b.drafts(A, [
    ["drafty", { subject: "Draft to Sy", to: ["Sy Draft <drafty@d.test>"], html: "<p>Draft.</p>" }],
  ]);
  return finish(b, app, opts);
}

/** `standard`, plus X (labels Proj and Ops on m1 only) and Y on A. */
export async function splits(gmail: Control, app: App, opts: SeedOptions = {}): Promise<Seeded> {
  const b = new Builder(gmail);
  await standardMail(b, opts, ["A", "B"]);
  await b.labels(A, ["Proj", "Ops"]);
  b.thread("X", A, "X marks", [
    {
      from: "Xia Xu <x@corp.test>",
      to: [A],
      text: "First from X.",
      labels: [...inbox(), b.label(A, "Proj"), b.label(A, "Ops")],
      date: dates.minutesAgo(75),
    },
    {
      from: "Xia Xu <x@corp.test>",
      to: [A],
      text: "Second from X.",
      labels: inbox(),
      date: dates.minutesAgo(65),
    },
  ]);
  b.thread("Y", A, "Y knot", [
    {
      from: "Yan Yu <y@corp.test>",
      to: [A],
      text: "From Y.",
      labels: inbox(),
      date: dates.minutesAgo(85),
    },
  ]);
  await b.deliver();
  return finish(b, app, opts);
}

/**
 * A signed in, with hostile mail. Every payload sets `top.__pwned` or
 * `parent.__pwned`; `evil.test` is routed to the server's sink (`gmail.hits()`).
 */
export async function security(gmail: Control, app: App, opts: SeedOptions = {}): Promise<Seeded> {
  const b = new Builder(gmail);
  await b.identity("A", signedIn(opts, "A", ["A"]));
  b.thread("hostile", A, "Hostile", [
    {
      from: "Mallory Mal <mallory@evil.test>",
      to: [A],
      html:
        "<p>Hostile content.</p>" +
        "<script>top.__pwned = 1</script>" +
        '<img src="x" onerror="top.__pwned = 2">' +
        '<svg onload="parent.__pwned = 3"><circle r="1"></circle></svg>' +
        '<body onload="top.__pwned = 4">' +
        '<a href="javascript:top.__pwned = 5">Run me</a>' +
        '<form action="http://evil.test/form" method="post"><input name="q" value="1"><button type="submit">Submit</button></form>' +
        '<iframe src="http://evil.test/frame"></iframe>' +
        '<meta http-equiv="refresh" content="0;url=http://evil.test/refresh">' +
        '<base href="http://evil.test/">' +
        '<div style="background:url(javascript:parent.__pwned = 6)">Styled</div>',
      labels: inbox(true),
      date: dates.minutesAgo(5),
    },
  ]);
  b.thread("hostileMeta", A, "<img src=x onerror=top.__pwned=1>", [
    {
      from: '"<b>Boss</b>" <boss@meta.test>',
      to: [A],
      text: "<script>top.__pwned = 7</script> please read",
      labels: inbox(true),
      date: dates.minutesAgo(10),
    },
  ]);
  await b.deliver();
  return finish(b, app, opts);
}

/**
 * A signed in: 60 Inbox threads (40 Important, 20 Other), 40 in Archive2019
 * out of Inbox, 40 archived invoices older than 90 days. Fast quota.
 */
export async function bulk(gmail: Control, app: App, opts: SeedOptions = {}): Promise<Seeded> {
  const b = new Builder(gmail);
  await b.identity("A", signedIn(opts, "A", ["A"]));
  await b.labels(A, ["Archive2019"]);
  const archive = b.label(A, "Archive2019");
  const one = (key: string, m: Msg & { subject: string }) =>
    b.thread(key, A, m.subject, [{ to: [A], ...m }]);
  for (let i = 1; i <= 40; i++) {
    one(`important${i}`, {
      from: `Person ${two(i)} <person${i}@bulk.test>`,
      subject: `Bulk ${two(i)}`,
      text: `Important mail ${i}.`,
      labels: inbox(i % 3 === 1),
      date: dates.minutesAgo(i),
    });
  }
  for (let i = 1; i <= 20; i++) {
    one(`other${i}`, {
      from: `Shop ${two(i)} <shop${i}@deals.test>`,
      subject: `Offer ${two(i)}`,
      text: `Promotion ${i}.`,
      labels: ["INBOX", "CATEGORY_PROMOTIONS"],
      date: dates.minutesAgo(60 + i),
    });
  }
  for (let i = 1; i <= 40; i++) {
    one(`archive${i}`, {
      from: `Archivist <arch${i}@old.test>`,
      subject: `Archived ${two(i)}`,
      text: `Archived mail ${i}.`,
      labels: ["CATEGORY_PERSONAL", archive],
      date: dates.daysAgo(30 + i),
    });
    one(`invoice${i}`, {
      from: `Billing <billing${i}@bills.test>`,
      subject: `Invoice ${two(i)}`,
      text: `Invoice number ${i}.`,
      labels: ["CATEGORY_PERSONAL"],
      date: dates.daysAgo(100 + i),
    });
  }
  await b.deliver();
  return finish(b, app, opts);
}

/** A signed in with 999 Important Inbox threads. Fast quota; its connect sync takes a while. */
export async function bulk1000(gmail: Control, app: App, opts: SeedOptions = {}): Promise<Seeded> {
  const b = new Builder(gmail);
  await b.identity("A", signedIn(opts, "A", ["A"]));
  for (let i = 1; i <= 999; i++) {
    b.thread(`t${i}`, A, `Thread ${i}`, [
      {
        from: `Person ${i} <p${i}@many.test>`,
        to: [A],
        text: `Mail ${i}.`,
        labels: inbox(),
        date: dates.minutesAgo(i),
      },
    ]);
  }
  await b.deliver();
  return finish(b, app, opts, OPEN_TIMEOUT.huge);
}
