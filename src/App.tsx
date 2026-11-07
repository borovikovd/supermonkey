import { useEffect } from "react";
import { Toaster } from "sonner";

import { composerKind, useComposeState } from "./compose/session";
import { followAccounts } from "./navigation";
import { useChrome } from "./store/chrome";
import { listKey } from "./store/current";
import { useAppState, useNeedsSignin, useSubscription } from "./store/hooks";
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
import { SearchTips } from "./ui/SearchTips";
import { SignInAgain } from "./ui/SignInAgain";
import { TopBar } from "./ui/TopBar";

/** Keeps each account's summary and last list subscribed, so switching back paints at once. */
function AccountSubs({ account }: { account: number }) {
  useSubscription({ type: "account", account });
  const where = useChrome((s) => s.perAccount[account]?.where ?? null);
  useSubscription(where ? listKey(account, where) : null);
  return null;
}

export function App() {
  const app = useAppState();
  const account = useChrome((s) => s.account);
  const screen = useChrome((s) =>
    s.account === null ? null : (s.perAccount[s.account]?.screen ?? null),
  );
  const composer = useComposeState(composerKind);
  /** The `c` composer replaces the screen while it is open. */
  const composing = useComposeState((s) => (s.onScreen?.thread === null ? s.onScreen.draft : null));
  const signedOut = useNeedsSignin(account);
  const searchOpen = useChrome((s) => s.search !== null);

  // Pick an account on screen; follow removals.
  useEffect(() => {
    if (app) followAccounts(app.accounts.map((a) => a.id));
  }, [app]);

  if (!app) return <div className="h-full bg-bg" />;
  if (app.accounts.length === 0) return <FirstRun />;

  return (
    <div className="flex h-full flex-col bg-surface text-text">
      {app.accounts.map((a) => (
        <AccountSubs key={a.id} account={a.id} />
      ))}
      {/* The traffic lights sit in this strip; content starts below it. */}
      <div data-tauri-drag-region className="h-titlebar shrink-0" />
      <div className="flex min-h-0 flex-1">
        <Rail accounts={app.accounts} />
        {/* The white panel on the off-white canvas, as in Superhuman. */}
        <div className="relative mr-2 mb-2 flex min-w-0 flex-1 flex-col overflow-hidden rounded-xl border border-border-2 bg-bg shadow-panel">
          {signedOut && composer === null ? (
            // A composer already open stays, a reply with its conversation,
            // so nothing typed is lost; closing it shows this screen.
            <main className="relative flex min-h-0 min-w-0 flex-1 flex-col">
              <SignInAgain
                account={account ?? 0}
                email={app.accounts.find((a) => a.id === account)?.email ?? ""}
              />
            </main>
          ) : (
            <>
              <TopBar />
              <main className="relative flex min-h-0 min-w-0 flex-1 flex-col">
                {composing !== null ? (
                  <ComposeScreen draft={composing} />
                ) : screen?.kind === "conversation" ? (
                  <ConversationView
                    key={`${screen.thread}:${screen.trashMode}`}
                    thread={screen.thread}
                    trashMode={screen.trashMode}
                  />
                ) : searchOpen ? (
                  <div className="flex min-h-0 flex-1">
                    <div className="flex min-w-0 flex-1 flex-col">
                      <ListView />
                    </div>
                    <SearchTips />
                  </div>
                ) : (
                  <ListView />
                )}
              </main>
            </>
          )}
          <HintBar />
        </div>
      </div>
      <Prefetch />
      <Palette />
      <Sheets />
      <SendToasts />
      <Toaster position="bottom-center" offset={28} toastOptions={{ className: "fade-in" }} />
    </div>
  );
}
