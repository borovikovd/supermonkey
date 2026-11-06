// Links inside the open conversation's messages (SPEC Reading, Keyboard):
// `Tab` cycles them, `Enter` opens the one reached, `Esc` clears it.

import { useChrome } from "../store/chrome";

const frames = () => Array.from(document.querySelectorAll<HTMLIFrameElement>("iframe[data-frame]"));

/** Every link in the expanded messages, in order, with its message id. */
function allLinks(): [number, HTMLAnchorElement][] {
  const links: [number, HTMLAnchorElement][] = [];
  for (const f of frames()) {
    const id = Number(f.dataset.frame);
    for (const a of Array.from(
      f.contentDocument?.querySelectorAll<HTMLAnchorElement>("a[href]") ?? [],
    )) {
      links.push([id, a]);
    }
  }
  return links;
}

/** Moves to the next link, or the previous one going back. */
export function cycleLink(back: boolean) {
  const c = useChrome.getState();
  const links = allLinks();
  for (const [, a] of links) a.classList.remove("ub-link-focus");
  if (links.length === 0) return;
  const current = c.link;
  let at = current
    ? links.findIndex(
        ([id], i) => id === current[0] && i - links.findIndex(([x]) => x === id) === current[1],
      )
    : -1;
  at = (at + (back ? -1 : 1) + links.length) % links.length;
  const picked = links[at];
  if (!picked) return;
  const [id, a] = picked;
  const first = links.findIndex(([x]) => x === id);
  a.classList.add("ub-link-focus");
  a.scrollIntoView({ block: "nearest" });
  a.ownerDocument.defaultView?.frameElement?.scrollIntoView({ block: "nearest" });
  c.setLink([id, at - first]);
}

/** Opens the link reached with Tab, as a click would. */
export function openLink() {
  const link = useChrome.getState().link;
  if (!link) return;
  const f = document.querySelector<HTMLIFrameElement>(`iframe[data-frame="${link[0]}"]`);
  f?.contentDocument?.querySelectorAll<HTMLAnchorElement>("a[href]")[link[1]]?.click();
}

/** Clears the link reached with Tab. */
export function clearLink() {
  for (const f of frames()) {
    f.contentDocument
      ?.querySelectorAll(".ub-link-focus")
      .forEach((x) => x.classList.remove("ub-link-focus"));
  }
  useChrome.getState().setLink(null);
}
