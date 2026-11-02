import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { Toaster } from "sonner";

import { connect, inTauri, quitNow, subscribe } from "./ipc/bridge";
import { appHooks, onKeyDown } from "./keyboard/keys";
import { closeComposerOnScreen } from "./compose/start";
import { useChrome, type PaletteMode } from "./store/chrome";
import { listKey, useAppState, useSubscription, useSummary } from "./store/hooks";
import { ConversationView } from "./ui/ConversationView";
import { ComposeScreen } from "./ui/ComposeScreen";
import { FirstRun } from "./ui/FirstRun";
import { HintBar } from "./ui/HintBar";
import { ListView } from "./ui/ListView";
import { Palette } from "./ui/Palette";
import { Prefetch } from "./ui/Prefetch";
import { Rail } from "./ui/Rail";
import { Sheets } from "./ui/Sheets";
import { SendToasts } from "./ui/SendToasts";
import { TopBar } from "./ui/TopBar";

let started = false;

/** Keeps each account's summary and last list subscribed, so switching back paints at once. */
function AccountSubs({ account }: { account: number }) {
  useSubscription({ type: "account", account });
  const where = useChrome((s) => s.perAccount[account]?.where ?? null);
  useSubscription(where ? listKey(account, where) : null);
  return null;
}

/** Switching accounts closes the palette and search and an open composer (SPEC Accounts → Switching). */
export function switchAccount(account: number) {
  const c = useChrome.getState();
  if (c.account === account) return;
  closeComposerOnScreen();
  if (c.account !== null) {
    // An open conversation is not reopened; the list is.
    c.patchCurrent({ screen: { kind: "list" } });
  }
  c.setAccount(account);
  c.set({ inline: null, sheet: null, link: null });
  const cur = useChrome.getState().perAccount[account];
  if (cur && cur.screen.kind !== "list")
    useChrome.getState().patchCurrent({ screen: { kind: "list" } });
}

export function App() {
  const [ready, setReady] = useState(false);
  const app = useAppState();
  const account = useChrome((s) => s.account);
  const screen = useChrome((s) =>
    s.account === null ? null : (s.perAccount[s.account]?.screen ?? null),
  );
  const summary = useSummary(account);

  useEffect(() => {
    if (started) return;
    started = true;
    void connect().then(() => {
      subscribe({ type: "app" });
      setReady(true);
    });
    window.addEventListener("keydown", onKeyDown);
    // Cmd+Q: close an open composer as a second Esc would, then quit once it commits.
    if (inTauri) {
      void listen("quit-requested", () => {
        closeComposerOnScreen();
        setTimeout(() => void quitNow(), 150);
      });
    }
  }, []);

  // Pick an account on screen; follow removals.
  useEffect(() => {
    if (!app) return;
    const ids = app.accounts.map((a) => a.id);
    if (account === null || !ids.includes(account)) {
      useChrome.getState().setAccount(ids[0] ?? null);
    }
  }, [app, account]);

  useEffect(() => {
    appHooks.switchAccount = (index) => {
      const a = app?.accounts[index];
      if (a) switchAccount(a.id);
    };
    appHooks.splitIdOf = (kind) =>
      summary?.splits.find((s) => s.kind === kind && s.enabled)?.id ?? null;
    appHooks.openPalette = (mode: PaletteMode) => useChrome.getState().set({ palette: mode });
  }, [summary, app]);

  if (!ready || !app) return <div className="h-full bg-bg" />;
  if (app.accounts.length === 0) return <FirstRun />;

  return (
    <div className="flex h-full flex-col bg-bg text-text">
      {app.accounts.map((a) => (
        <AccountSubs key={a.id} account={a.id} />
      ))}
      <TopBar />
      <div className="flex min-h-0 flex-1">
        <Rail accounts={app.accounts} />
        <main className="relative flex min-w-0 flex-1 flex-col">
          {screen?.kind === "conversation" ? (
            <ConversationView
              key={`${screen.thread}:${screen.trashMode}`}
              thread={screen.thread}
              trashMode={screen.trashMode}
            />
          ) : screen?.kind === "compose" ? (
            <ComposeScreen draft={screen.draft} />
          ) : (
            <ListView />
          )}
        </main>
      </div>
      <HintBar />
      <Prefetch />
      <Palette />
      <Sheets />
      <SendToasts />
      <Toaster position="bottom-center" offset={28} toastOptions={{ className: "fade-in" }} />
    </div>
  );
}
