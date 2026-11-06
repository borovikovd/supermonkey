import { Archive, Clock, File, Inbox, Send, Star } from "lucide-react";

import type { AccountInfo, View } from "../ipc/types";
import { goTo, openSheet, switchAccount } from "../navigation";
import { useCurrent } from "../store/hooks";
import { countLabel } from "./format";

const VIEWS: { view: View; icon: typeof Inbox; title: string }[] = [
  { view: { kind: "inbox" }, icon: Inbox, title: "Inbox" },
  { view: { kind: "done" }, icon: Archive, title: "Done (g e)" },
  { view: { kind: "snoozed" }, icon: Clock, title: "Snoozed (g h)" },
  { view: { kind: "starred" }, icon: Star, title: "Starred (g s)" },
  { view: { kind: "sent" }, icon: Send, title: "Sent (g t)" },
  { view: { kind: "drafts" }, icon: File, title: "Drafts (g d)" },
];

/** What the avatar's tooltip adds: sign in again, or the quiet sync status (SPEC Gmail errors). */
function status(a: AccountInfo): string {
  if (a.needsSignin) return " · Sign in again";
  if (a.syncStatus === "retrying") return " · Can't reach Gmail";
  return "";
}

function Avatar({ account }: { account: AccountInfo }) {
  const initial = (account.name || account.email).trim().charAt(0).toUpperCase();
  return (
    <span
      className="flex items-center justify-center rounded-full text-hint font-semibold"
      style={{ width: 24, height: 24, background: `${account.hue}26`, color: account.hue }}
    >
      {initial}
    </span>
  );
}

export function Rail({ accounts }: { accounts: AccountInfo[] }) {
  const { account, cur } = useCurrent();
  const active = cur?.where.kind === "view" ? cur.where.view.kind : null;
  return (
    <nav className="flex w-rail shrink-0 flex-col items-center pt-1 pb-4">
      {/* View icons in one quiet pill, as Superhuman groups them. */}
      <div className="flex flex-col items-center gap-1 rounded-2xl bg-fill/70 p-1">
        {VIEWS.map(({ view, icon: Icon, title }) => (
          <button
            key={view.kind}
            title={title}
            onClick={() => goTo(view)}
            className={`flex h-8 w-8 items-center justify-center rounded-lg ${
              active === view.kind ? "bg-bg text-text shadow-lift" : "text-text-2 hover:text-text"
            }`}
          >
            <Icon size={16} strokeWidth={1.5} />
          </button>
        ))}
      </div>
      <div className="flex-1" />
      <div className="flex flex-col items-center gap-2">
        {accounts.map((a, i) => (
          <button
            key={a.id}
            title={`${a.email} (Ctrl+${i + 1})${status(a)}`}
            // The account on screen opens its settings; another one switches to it.
            onClick={() => (a.id === account ? openSheet("settings") : switchAccount(a.id))}
            className={`relative rounded-full p-0.5 ${
              a.id === account ? "ring-avatar ring-accent" : "hover:ring-avatar hover:ring-border"
            }`}
          >
            <Avatar account={a} />
            {a.inboxUnread > 0 && (
              <span className="tabular absolute -top-1.5 left-1/2 ml-1 rounded-full bg-unread px-1 text-badge font-semibold text-inverse">
                {countLabel(a.inboxUnread)}
              </span>
            )}
            {a.needsSignin ? (
              <span
                data-marker="signin"
                className="absolute -right-0.5 -bottom-0.5 h-2 w-2 rounded-full bg-danger"
              />
            ) : (
              a.syncStatus === "retrying" && (
                <span
                  data-marker="retrying"
                  className="absolute -right-0.5 -bottom-0.5 h-2 w-2 rounded-full bg-text-3"
                />
              )
            )}
          </button>
        ))}
      </div>
    </nav>
  );
}
