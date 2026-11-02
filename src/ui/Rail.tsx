import { Archive, Clock, File, Inbox, Send, Star } from "lucide-react";

import { switchAccount } from "../App";
import { goTo } from "../commands";
import type { AccountInfo, View } from "../ipc/types";
import { useCurrent } from "../store/hooks";
import { countLabel } from "./TopBar";

const VIEWS: { view: View; icon: typeof Inbox; title: string }[] = [
  { view: { kind: "inbox" }, icon: Inbox, title: "Inbox" },
  { view: { kind: "done" }, icon: Archive, title: "Done (g e)" },
  { view: { kind: "snoozed" }, icon: Clock, title: "Snoozed (g h)" },
  { view: { kind: "starred" }, icon: Star, title: "Starred (g s)" },
  { view: { kind: "sent" }, icon: Send, title: "Sent (g t)" },
  { view: { kind: "drafts" }, icon: File, title: "Drafts (g d)" },
];

export function Avatar({ account, size = 24 }: { account: AccountInfo; size?: number }) {
  const initial = (account.name || account.email).trim().charAt(0).toUpperCase();
  return (
    <span
      className="flex items-center justify-center rounded-full text-hint font-semibold"
      style={{ width: size, height: size, background: `${account.hue}26`, color: account.hue }}
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
      <div className="flex flex-col items-center gap-1 rounded-[12px] bg-fill/70 p-1">
        {VIEWS.map(({ view, icon: Icon, title }) => (
          <button
            key={view.kind}
            title={title}
            onClick={() => goTo({ kind: "view", view })}
            className={`flex h-8 w-8 items-center justify-center rounded-[8px] ${
              active === view.kind
                ? "bg-bg text-text shadow-[0_1px_2px_rgb(20_20_19/0.08)]"
                : "text-text-2 hover:text-text"
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
            title={`${a.email} (Ctrl+${i + 1})${a.needsSignin ? " · Sign in again" : ""}`}
            onClick={() => switchAccount(a.id)}
            className={`relative rounded-full p-0.5 ${a.id === account ? "ring-[1.5px] ring-accent" : ""}`}
          >
            <Avatar account={a} />
            {a.inboxUnread > 0 && (
              <span className="tabular absolute -top-1.5 left-1/2 ml-1 rounded-full bg-unread px-1 text-[9px] leading-[13px] font-semibold text-white">
                {countLabel(a.inboxUnread)}
              </span>
            )}
            {a.needsSignin && (
              <span className="absolute -right-0.5 -bottom-0.5 h-2 w-2 rounded-full bg-danger" />
            )}
          </button>
        ))}
      </div>
    </nav>
  );
}
