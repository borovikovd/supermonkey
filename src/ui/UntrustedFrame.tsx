import { useEffect, useRef } from "react";

import { openUrlInCore } from "../ipc/bridge";

const TOKENS = [
  "--color-text",
  "--color-text-2",
  "--color-text-3",
  "--color-link",
  "--color-border-2",
  "--color-accent",
  "--font-sans",
] as const;

let tokens: string | undefined;

function frameTokens(): string {
  if (tokens === undefined) {
    const root = getComputedStyle(document.documentElement);
    tokens = `:root{${TOKENS.map((name) => `${name}:${root.getPropertyValue(name).trim()}`).join(";")}}`;
  }
  return tokens;
}

function frameCsp(remoteImages: boolean): string {
  return remoteImages
    ? "script-src 'none'"
    : "default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:";
}

function frameDocument(html: string, css: string, remoteImages: boolean): string {
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${frameCsp(remoteImages)}"><style>${frameTokens()}${css}</style></head><body>${html}</body></html>`;
}

function catchLinks(doc: Document, signal: AbortSignal): void {
  doc.addEventListener(
    "click",
    (e) => {
      const view = doc.defaultView;
      const a = view && e.target instanceof view.Element ? e.target.closest("a") : null;
      if (!a) return;
      e.preventDefault();
      const href = a.getAttribute("href");
      if (href && /^(https?:|mailto:)/i.test(href)) {
        openUrlInCore(href).catch((err: unknown) => console.warn("open link", err));
      }
    },
    { signal },
  );
  doc.addEventListener("submit", (e) => e.preventDefault(), { capture: true, signal });
}

export function UntrustedFrame({
  html,
  css,
  remoteImages,
  title,
  frameId,
  className,
  onFit,
  onReady,
}: {
  html: string;
  css: string;
  remoteImages: boolean;
  title: string;
  frameId?: number;
  className: string;
  onFit?: () => void;
  onReady?: (doc: Document, fit: () => void, signal: AbortSignal) => void;
}) {
  const frame = useRef<HTMLIFrameElement>(null);

  useEffect(() => {
    const el = frame.current;
    if (!el) return;
    let ro: ResizeObserver | null = null;
    let wired: AbortController | null = null;
    const setup = () => {
      const doc = el.contentDocument;
      if (doc?.URL !== "about:srcdoc") return;
      ro?.disconnect();
      wired?.abort();
      const { signal } = (wired = new AbortController());
      const fit = () => {
        el.style.height = `${doc.documentElement.scrollHeight}px`;
        onFit?.();
      };
      fit();
      ro = new ResizeObserver(fit);
      ro.observe(doc.documentElement);
      for (const img of Array.from(doc.images)) img.addEventListener("load", fit, { signal });
      catchLinks(doc, signal);
      onReady?.(doc, fit, signal);
    };
    el.addEventListener("load", setup);
    if (el.contentDocument?.readyState === "complete") setup();
    return () => {
      el.removeEventListener("load", setup);
      ro?.disconnect();
      wired?.abort();
    };
  }, [onFit, onReady]);

  return (
    <iframe
      ref={frame}
      data-frame={frameId}
      title={title}
      // oxlint-disable-next-line react/iframe-missing-sandbox -- WebKit never fires the parent's listeners in a frame without allow-scripts; the CSP runs nothing
      sandbox="allow-same-origin allow-scripts"
      srcDoc={frameDocument(html, css, remoteImages)}
      className={className}
      style={{ height: 0, colorScheme: "light" }}
    />
  );
}
