import { openUrlInCore } from "../ipc/bridge";

export function catchLinks(doc: Document): void {
  doc.addEventListener("click", (e) => {
    const view = doc.defaultView;
    const a = view && e.target instanceof view.Element ? e.target.closest("a") : null;
    if (!a) return;
    e.preventDefault();
    const href = a.getAttribute("href");
    if (href && /^(https?:|mailto:)/i.test(href)) {
      openUrlInCore(href).catch((err: unknown) => console.warn("open link", err));
    }
  });
  doc.addEventListener("submit", (e) => e.preventDefault(), true);
}
