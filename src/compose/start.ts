// Starting composers: `c`, `r`, `Enter` (reply all), `f`, and resuming drafts.
// Filled in with the composer.

export type ReplyMode = "reply" | "replyAll" | "forward" | "resumeDraft";

export function startCompose() {}

export function startReply(_mode: ReplyMode, _draft?: string) {}

/** Closes a composer on screen as a second Esc would (before switching, quitting, etc.). */
export function closeComposerOnScreen() {}
