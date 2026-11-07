// Link clicks in a frame that shows mail HTML: the message frame and the
// composer's quoted text (SPEC Reading).

import { openUrlInCore } from "../ipc/bridge";

/**
 * Catches clicks on the frame's links: `http`, `https`, and `mailto` open in
 * the default browser or mail app, others do nothing, and none navigates the
 * frame or the app window. Forms never submit.
 */
export function catchLinks(doc: Document): void {
  doc.addEventListener("click", (e) => {
    // Nodes in the frame belong to the frame's realm, not the app window's.
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
