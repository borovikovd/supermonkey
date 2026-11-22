import { useEffect, useState } from "react";

import { getBodies } from "../ipc/bridge";
import type { MessageBody } from "../ipc/types";
import { dispatchKey } from "../keyboard/dispatch";
import { UntrustedFrame } from "./UntrustedFrame";

const bodies = new Map<number, MessageBody>();
const waiting = new Map<number, Promise<void>>();
let batch: { ids: number[]; done: Promise<void> } | null = null;

async function fetchBodies(ids: number[]): Promise<void> {
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
.sm-toggle{display:block;width:fit-content;margin:4px 0;color:var(--color-text-3);font-size:15px;line-height:15px;letter-spacing:2px;cursor:pointer;user-select:none}.sm-toggle:hover{color:var(--color-text-2)}
.sm-quote{display:none}
.sm-quote.sm-open{display:block}
a.sm-link-focus{outline:2px solid var(--color-accent);outline-offset:2px;border-radius:2px}
`;

function withQuote(body: MessageBody, quoteOpen: boolean): string {
  return body.quoteHtml
    ? `${body.html}<div class="sm-toggle" data-sm-toggle>•••</div><div class="sm-quote${quoteOpen ? " sm-open" : ""}">${body.quoteHtml}</div>`
    : body.html;
}

export function LoadingBody({ snippet }: { snippet: string }) {
  return (
    <div aria-busy="true" aria-label="Loading message" data-loading className="relative">
      <div aria-hidden="true" className="loading-bar inset-x-0 -top-2" />
      <p className="text-body text-text">{snippet && `${snippet}…`}</p>
    </div>
  );
}

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
  snippet: string;
  remoteImages: boolean;
  quoteOpen: boolean;
  onSelect: (id: number) => void;
}) {
  const [body, setBody] = useState(() => bodies.get(id));
  const [sized, setSized] = useState(false);

  useEffect(() => {
    if (body || !hasBody) return;
    let live = true;
    void loadBody(id).then(() => live && setBody(bodies.get(id)));
    return () => {
      live = false;
    };
  }, [id, hasBody, body]);

  const ready = (doc: Document, fit: () => void, signal: AbortSignal) => {
    doc.addEventListener("keydown", dispatchKey, { signal });
    doc.addEventListener("mousedown", () => onSelect(id), { signal });
    doc.addEventListener(
      "click",
      (e) => {
        const t = e.target;
        const view = doc.defaultView;
        if (!view || !(t instanceof view.Element) || !t.closest("[data-sm-toggle]")) return;
        doc.querySelector(".sm-quote")?.classList.toggle("sm-open");
        requestAnimationFrame(fit);
      },
      { signal },
    );
  };

  return (
    <div className="relative">
      {!sized && <LoadingBody snippet={snippet} />}
      {body && (
        <UntrustedFrame
          frameId={id}
          title={`message ${id}`}
          html={withQuote(body, quoteOpen)}
          css={BASE_STYLE}
          remoteImages={remoteImages}
          className={`block w-full border-0 ${sized ? "" : "invisible absolute inset-x-0 top-0"}`}
          onFit={() => setSized(true)}
          onReady={ready}
        />
      )}
    </div>
  );
}
