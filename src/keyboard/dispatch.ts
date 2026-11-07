// The one keydown dispatcher (SPEC Keyboard). Every key, from the window or
// from inside a message, goes through the layers below, top first. A layer
// that takes a key runs it and stops it there, so no field, list, or library
// handler below sees it too; a modal layer also keeps the keys it does not
// take from the layers under it, which leaves them to the focused field and
// the browser. Surfaces that own a layer's state register its handler.

import { composeBlurred } from "../compose/session";
import { typeAhead } from "../compose/start";
import { useChrome } from "../store/chrome";
import { context, runKey, type Ctx } from "./table";

/** A surface's own key handler: true if it took the key. */
export type KeyHandler = (e: KeyboardEvent) => boolean;

/** Layers whose handler a mounted surface registers. */
type Registered = "palette" | "suggestions";

const registered: Record<Registered, KeyHandler | null> = { palette: null, suggestions: null };

/** Registers a surface's handler for its layer; returns the unregister. */
export function registerKeys(layer: Registered, handler: KeyHandler): () => void {
  registered[layer] = handler;
  return () => {
    if (registered[layer] === handler) registered[layer] = null;
  };
}

const isTextField = (el: EventTarget | null) =>
  el instanceof HTMLElement &&
  (el.isContentEditable ||
    el.tagName === "INPUT" ||
    el.tagName === "TEXTAREA" ||
    el.tagName === "SELECT");

type Layer = {
  name: string;
  active: (c: Ctx, e: KeyboardEvent) => boolean;
  take: (e: KeyboardEvent, c: Ctx, start: number) => boolean;
  /** Keys this layer does not take go no lower. */
  modal: boolean;
};

/** The layers, top first: SPEC Keyboard's precedence. */
const LAYERS: Layer[] = [
  // Account switching and Settings work everywhere, even while typing.
  {
    name: "global",
    active: () => true,
    take: (e, c, t) => runKey("global", e, c, t),
    modal: false,
  },
  {
    name: "sheet",
    active: (c) => c.sheet,
    take: (e, c, t) => runKey("sheet", e, c, t),
    modal: true,
  },
  {
    name: "palette",
    active: () => useChrome.getState().palette !== null,
    take: (e) => registered.palette?.(e) ?? false,
    modal: true,
  },
  // Typed after r, f, or Enter, before the reply editor has focus: the reply's text.
  { name: "typeAhead", active: () => true, take: typeAhead, modal: false },
  {
    name: "suggestions",
    active: () => registered.suggestions !== null,
    take: (e) => registered.suggestions?.(e) ?? false,
    modal: false,
  },
  {
    name: "compose",
    active: (c) => c.composer !== null,
    take: (e, c, t) => {
      if (runKey("compose", e, c, t)) return true;
      // Other keys do nothing while compose is blurred.
      if (composeBlurred() && !e.metaKey) e.preventDefault();
      return false;
    },
    modal: true,
  },
  {
    name: "search",
    active: (c) => c.searchField,
    take: (e, c, t) => runKey("search", e, c, t),
    modal: true,
  },
  // Any other text field keeps its keys.
  { name: "field", active: (_c, e) => isTextField(e.target), take: () => false, modal: true },
  { name: "mail", active: () => true, take: (e, c, t) => runKey("mail", e, c, t), modal: true },
];

/** Keys that only modify the next one: no layer takes them alone. */
const MODIFIERS = new Set(["Shift", "Meta", "Control", "Alt", "AltGraph", "CapsLock", "Fn"]);

/** Routes a keydown through the layers. */
export function dispatchKey(e: KeyboardEvent) {
  // Shift goes down before the `#` it types; it must not end the wait for `g`'s second key.
  if (MODIFIERS.has(e.key)) return;
  const start = performance.now();
  const c = context();
  if (c.account === null) return;
  for (const layer of LAYERS) {
    if (!layer.active(c, e)) continue;
    if (layer.take(e, c, start)) {
      e.stopPropagation();
      return;
    }
    if (layer.modal) return;
  }
}

/** Installs the dispatcher ahead of every other keydown listener; call once at startup. */
export function installKeys() {
  window.addEventListener("keydown", dispatchKey, { capture: true });
}
