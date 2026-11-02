// Palette items for a blurred composer (send, attach, discard) and the From picker.
// Filled in with the composer.

type Item = { id: string; label: string; keys?: string; run: () => void; keep?: boolean };

export function draftPaletteItems(_draft?: string): Item[] {
  return [];
}
