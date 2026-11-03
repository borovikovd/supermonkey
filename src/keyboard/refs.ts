// Hooks and refs the keyboard handler reads, set by the components that own
// them. A leaf module, so nothing here imports back into the UI.

import type { ConversationData } from "../ipc/types";
import type { PaletteMode } from "../store/chrome";

/** Set by the conversation view so keys can act on its data. */
export const conversationRef: { data: ConversationData | null } = { data: null };

/** Set by App: account switching and split lookup. */
export const appHooks: {
  switchAccount: (index: number) => void;
  splitIdOf: (kind: string) => number | null;
  openPalette: (mode: PaletteMode) => void;
  focusSearch: () => void;
  linkKey: (e: KeyboardEvent) => boolean;
  /** The next highlight move shows its split header at the top of the list. */
  headerToTop: () => void;
  /** Closes the composer on screen as a second Esc would (set by main, to avoid an import cycle). */
  closeComposer: () => void;
} = {
  switchAccount: () => {},
  splitIdOf: () => null,
  openPalette: () => {},
  focusSearch: () => {},
  linkKey: () => false,
  closeComposer: () => {},
  headerToTop: () => {},
};
