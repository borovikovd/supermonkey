import { openUrl } from "@tauri-apps/plugin-opener";
import { memo, useEffect, useRef, useState } from "react";

import { getBodies, inTauri } from "../ipc/bridge";
import type { MessageBody } from "../ipc/types";

/** Bodies never change, so a loaded body is kept for the session. */
const bodies = new Map<number, MessageBody>();
const waiting = new Map<number, Promise<void>>();

export function loadBody(id: number): Promise<void> {
  if (bodies.has(id)) return Promise.resolve();
  let p = waiting.get(id);
  if (!p) {
    p = getBodies([id])
      .then((r) => {
        for (const b of r) bodies.set(b.id, b);
      })
      .finally(() => waiting.delete(id));
    waiting.set(id, p);
  }
  return p;
}

export const bodyOf = (id: number) => bodies.get(id);

const BASE_STYLE = `
html,body{margin:0;padding:0;background:transparent}
body{font:15px/26px -apple-system,BlinkMacSystemFont,"SF Pro Text",sans-serif;color:#141413;overflow-wrap:anywhere;-webkit-font-smoothing:antialiased}
img{max-width:100%;height:auto}
table{max-width:100%}
a{color:#4198D2}
blockquote{margin:0 0 0 8px;padding-left:12px;border-left:2px solid #EDEBE5;color:#73716D}
.ub-toggle{display:inline-block;margin:4px 0;color:#A3A09A;font-size:15px;line-height:15px;letter-spacing:2px;cursor:pointer;user-select:none}.ub-toggle:hover{color:#73716D}
.ub-quote{display:none}
.ub-quote.ub-open{display:block}
a.ub-link-focus{outline:2px solid #714CB6;outline-offset:2px;border-radius:2px}
`;

function srcdoc(body: MessageBody, remoteImages: boolean, quoteOpen: boolean): string {
  const csp = remoteImages ? "script-src 'none'" : "script-src 'none'; img-src data:";
  const quote = body.quoteHtml
    ? `<div class="ub-toggle" data-ub-toggle>•••</div><div class="ub-quote${quoteOpen ? " ub-open" : ""}">${body.quoteHtml}</div>`
    : "";
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${csp}"><style>${BASE_STYLE}</style></head><body>${body.html}${quote}</body></html>`;
}

/**
 * An expanded message body in a sandboxed `srcdoc` iframe. `allow-scripts`
 * is only there so WebKit fires the parent's listeners (bug 218086); the
 * CSP blocks every script in the message.
 */
export const MessageFrame = memo(function MessageFrame({
  id,
  hasBody,
  remoteImages,
  quoteOpen,
  onKey,
  onSelect,
}: {
  id: number;
  hasBody: boolean;
  remoteImages: boolean;
  quoteOpen: boolean;
  onKey: (e: KeyboardEvent) => void;
  /** Clicks inside the message body select the message. */
  onSelect: (id: number) => void;
}) {
  const [body, setBody] = useState(() => bodies.get(id));
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
      if (!doc) return;
      const fit = () => {
        el.style.height = `${doc.documentElement.scrollHeight}px`;
      };
      fit();
      ro = new ResizeObserver(fit);
      ro.observe(doc.documentElement);
      for (const img of Array.from(doc.images)) img.addEventListener("load", fit);
      doc.addEventListener("keydown", (e) => onKey(e));
      doc.addEventListener("mousedown", () => onSelect(id));
      doc.addEventListener("click", (e) => {
        const t = e.target as HTMLElement;
        if (t.closest("[data-ub-toggle]")) {
          doc.querySelector(".ub-quote")?.classList.toggle("ub-open");
          requestAnimationFrame(fit);
          return;
        }
        const a = t.closest("a");
        if (a) {
          // Links never navigate the app window.
          e.preventDefault();
          const href = a.getAttribute("href");
          if (href && /^(https?:|mailto:)/i.test(href)) {
            if (inTauri) void openUrl(href);
            else window.open(href, "_blank", "noopener");
          }
        }
      });
      doc.addEventListener("submit", (e) => e.preventDefault(), true);
    };
    el.addEventListener("load", setup);
    if (el.contentDocument?.readyState === "complete") setup();
    return () => {
      el.removeEventListener("load", setup);
      ro?.disconnect();
    };
  }, [body, onKey, onSelect, id, remoteImages, quoteOpen]);

  if (!body) {
    return (
      <div className="py-2 text-row text-text-3">
        {hasBody ? "Loading…" : "Downloading message…"}
      </div>
    );
  }
  return (
    <iframe
      ref={frame}
      data-frame={id}
      title={`message ${id}`}
      sandbox="allow-same-origin allow-scripts"
      srcDoc={srcdoc(body, remoteImages, quoteOpen)}
      className="block w-full border-0"
      style={{ height: 40 }}
    />
  );
});
