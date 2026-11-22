import { composeBlurred } from "../compose/session";
import { typeAhead } from "../compose/start";
import { useChrome } from "../store/chrome";
import { context, runKey, type Ctx } from "./table";

export type KeyHandler = (e: KeyboardEvent) => boolean;

type Registered = "palette" | "suggestions";

const registered: Record<Registered, KeyHandler | null> = { palette: null, suggestions: null };

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
  modal: boolean;
};

const LAYERS: Layer[] = [
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
      if (composeBlurred() && !e.metaKey) e.preventDefault();
      return false;
    },
    modal: true,
  },
  {
    name: "search",
    active: (c) => c.searchField && !c.signedOut,
    take: (e, c, t) => runKey("search", e, c, t),
    modal: true,
  },
  { name: "field", active: (_c, e) => isTextField(e.target), take: () => false, modal: true },
  { name: "mail", active: () => true, take: (e, c, t) => runKey("mail", e, c, t), modal: true },
];

const MODIFIERS = new Set(["Shift", "Meta", "Control", "Alt", "AltGraph", "CapsLock", "Fn"]);

export function dispatchKey(e: KeyboardEvent) {
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

export function installKeys() {
  window.addEventListener("keydown", dispatchKey, { capture: true });
}
