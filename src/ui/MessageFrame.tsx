import { useEffect, useRef, useState } from "react";

import { getBodies } from "../ipc/bridge";
import type { MessageBody } from "../ipc/types";
import { dispatchKey } from "../keyboard/dispatch";
import { frameCsp } from "./frameCsp";
import { catchLinks } from "./frameLinks";
import { frameTokens } from "./frameTokens";

/** Bodies never change, so a loaded body is kept for the session. */
const bodies = new Map<number, MessageBody>();
const waiting = new Map<number, Promise<void>>();
/** Ids asked for in this tick: frames that mount together share one call. */
let batch: { ids: number[]; done: Promise<void> } | null = null;

async function fetchBodies(ids: number[]): Promise<void> {
  // Let the other frames of this tick add their ids first.
  await Promise.resolve();
  batch = null;
  try {
    for (const b of await getBodies(ids)) bodies.set(b.id, b);
  } catch (err) {
    console.warn("load message bodies", ids, err);
  } finally {
    for (const i of ids) waiting.delete(i);
  }
}

function loadBody(id: number): Promise<void> {
  if (bodies.has(id)) return Promise.resolve();
  const pending = waiting.get(id);
  if (pending) return pending;
  if (!batch) {
    const ids: number[] = [];
    batch = { ids, done: fetchBodies(ids) };
  }
  batch.ids.push(id);
  waiting.set(id, batch.done);
  return batch.done;
}

const BASE_STYLE = `
html,body{margin:0;padding:0;background:transparent}
body{font:15px/26px var(--font-sans);color:var(--color-text);overflow-wrap:anywhere;-webkit-font-smoothing:antialiased}
body>:first-child{margin-top:0}
body>:last-child{margin-bottom:0}
img{max-width:100%;height:auto}
table{max-width:100%}
a{color:var(--color-link)}
blockquote{margin:0 0 0 8px;padding-left:12px;border-left:2px solid var(--color-border-2);color:var(--color-text-2)}
.ub-toggle{display:block;width:fit-content;margin:4px 0;color:var(--color-text-3);font-size:15px;line-height:15px;letter-spacing:2px;cursor:pointer;user-select:none}.ub-toggle:hover{color:var(--color-text-2)}
.ub-quote{display:none}
.ub-quote.ub-open{display:block}
a.ub-link-focus{outline:2px solid var(--color-accent);outline-offset:2px;border-radius:2px}
`;

function srcdoc(body: MessageBody, remoteImages: boolean, quoteOpen: boolean): string {
  const csp = frameCsp(remoteImages);
  const quote = body.quoteHtml
    ? `<div class="ub-toggle" data-ub-toggle>•••</div><div class="ub-quote${quoteOpen ? " ub-open" : ""}">${body.quoteHtml}</div>`
    : "";
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${csp}"><style>${frameTokens()}${BASE_STYLE}</style></head><body>${body.html}${quote}</body></html>`;
}

/**
 * A message whose body is still on its way: its snippet, readable at once as
 * in Gmail and Superhuman and set as the body's text is, where the body will
 * start, under a thin bar that takes no room and shows only if the wait is
 * long enough to notice.
 */
export function LoadingBody({ snippet }: { snippet: string }) {
  return (
    <div aria-busy="true" aria-label="Loading message" data-loading className="relative">
      <div aria-hidden="true" className="loading-bar inset-x-0 -top-2" />
      <p className="text-body text-text">{snippet && `${snippet}…`}</p>
    </div>
  );
}

/** An expanded message body in a sandboxed `srcdoc` iframe, sized to its content. */
export function MessageFrame({
  id,
  hasBody,
  snippet,
  remoteImages,
  quoteOpen,
  onSelect,
}: {
  id: number;
  hasBody: boolean;
  /** Shown, with a loading bar, until the body is here. */
  snippet: string;
  remoteImages: boolean;
  quoteOpen: boolean;
  /** Clicks inside the message body select the message. */
  onSelect: (id: number) => void;
}) {
  const [body, setBody] = useState(() => bodies.get(id));
  // The snippet stays until the frame has laid out its body and taken its
  // height, so the message grows once instead of shrinking to an empty frame.
  const [sized, setSized] = useState(false);
  const frame = useRef<HTMLIFrameElement>(null);

  useEffect(() => {
    if (body || !hasBody) return;
    let live = true;
    void loadBody(id).then(() => live && setBody(bodies.get(id)));
    return () => {
      live = false;
    };
  }, [id, hasBody, body]);

  useEffect(() => {
    const el = frame.current;
    if (!el || !body) return;
    let ro: ResizeObserver | null = null;
    const setup = () => {
      const doc = el.contentDocument;
      // Before the srcdoc loads, the frame holds an empty about:blank.
      if (doc?.URL !== "about:srcdoc") return;
      ro?.disconnect();
      const fit = () => {
        el.style.height = `${doc.documentElement.scrollHeight}px`;
        setSized(true);
      };
      fit();
      ro = new ResizeObserver(fit);
      ro.observe(doc.documentElement);
      for (const img of Array.from(doc.images)) img.addEventListener("load", fit);
      // Keys pressed inside a message go to the app's dispatcher, as keys in
      // the window do: the shortcut table and an open composer's keys.
      doc.addEventListener("keydown", dispatchKey);
      doc.addEventListener("mousedown", () => onSelect(id));
      doc.addEventListener("click", (e) => {
        // Nodes in the frame belong to the frame's realm, not the app window's.
        const t = e.target;
        const view = doc.defaultView;
        if (!view || !(t instanceof view.Element)) return;
        if (t.closest("[data-ub-toggle]")) {
          doc.querySelector(".ub-quote")?.classList.toggle("ub-open");
          requestAnimationFrame(fit);
        }
      });
      catchLinks(doc);
    };
    el.addEventListener("load", setup);
    if (el.contentDocument?.readyState === "complete") setup();
    return () => {
      el.removeEventListener("load", setup);
      ro?.disconnect();
    };
  }, [body, onSelect, id]);

  return (
    <div className="relative">
      {!sized && <LoadingBody snippet={snippet} />}
      {body && (
        <iframe
          ref={frame}
          data-frame={id}
          title={`message ${id}`}
          // allow-scripts is only there so WebKit fires the parent's listeners (bug 218086);
          // the document's CSP is script-src 'none', so nothing in a message can run.
          // oxlint-disable-next-line react/iframe-missing-sandbox
          sandbox="allow-same-origin allow-scripts"
          srcDoc={srcdoc(body, remoteImages, quoteOpen)}
          // Until sized, it lays out unseen behind the snippet, from no height, so
          // its measured height is its content's.
          className={`block w-full border-0 ${sized ? "" : "invisible absolute inset-x-0 top-0"}`}
          // The card is always white, so a message never gets its dark-mode styles:
          // WebKit answers prefers-color-scheme inside the frame from this property.
          style={{ height: 0, colorScheme: "light" }}
        />
      )}
    </div>
  );
}
