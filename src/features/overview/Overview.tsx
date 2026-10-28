import * as React from "react";
import {
  RefreshCw, ExternalLink, Mail, Star, Paperclip, Inbox, AlertTriangle, X, LayoutGrid, Archive, Trash2, MailOpen,
} from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import {
  isOverviewWindow, reloadOverview, setState, setUi, setView, useStore, type OverviewColumn,
} from "../../lib/store";
import type { Account, MessageSummary } from "../../lib/types";
import { cn, displayName, listDate } from "../../lib/utils";
import { Button, IconButton } from "../../components/ui/button";
import { EmptyState, Skeleton, Spinner } from "../../components/ui/misc";
import { Reader } from "../reader/Reader";
import * as A from "../actions";

/** Persona/story: someone running several businesses from one desk (Paul has 11 mailboxes) wants to see every inbox
 *  side by side and spot what needs answering without clicking account by account. One column per account, newest
 *  first, live; clicking a message opens it beside the columns without losing the overview. */
export function Overview({ dark }: { dark: boolean }) {
  const accounts = useStore((s) => s.accounts);
  const overview = useStore((s) => s.overview);
  const openId = useStore((s) => s.openId);
  const filter = useStore((s) => s.filter);
  const folders = useStore((s) => s.folders);
  const [syncing, setSyncing] = React.useState(false);
  const enabled = accounts.filter((a) => a.enabled).sort((a, b) => a.sortOrder - b.sortOrder);
  const standalone = isOverviewWindow();

  // First load, and reload when accounts/folders appear (a new account's Inbox shows up after its first sync).
  const inboxKey = folders.filter((f) => f.role === "inbox").map((f) => f.id).join(",");
  React.useEffect(() => { void reloadOverview(); }, [inboxKey, accounts.length]);

  const unreadTotal = enabled.reduce((n, a) => n + (folders.find((f) => f.accountId === a.id && f.role === "inbox")?.unread ?? 0), 0);

  return (
    <section aria-label="All inboxes" className="flex h-full min-w-0 flex-col bg-surface-inset">
      <header className="flex flex-wrap items-center gap-3 border-b border-border bg-surface px-5 py-3">
        {standalone && <img src="/icon.png" alt="" className="size-7 rounded-lg" />}
        <LayoutGrid className={cn("size-5 text-[var(--iw-red-text)]", standalone && "hidden")} />
        <h1 className="font-title text-[20px] leading-none">All inboxes</h1>
        <span className="rounded-full bg-primary px-2.5 py-0.5 text-xs font-semibold tabular-nums text-white" aria-label={`${unreadTotal} unread across all inboxes`}>
          {unreadTotal.toLocaleString()} unread
        </span>
        <span className="text-xs text-muted-foreground">{enabled.length} {enabled.length === 1 ? "account" : "accounts"}</span>
        <div className="flex-1" />
        <button
          type="button"
          aria-pressed={!!filter.unread}
          onClick={() => { setState({ filter: { ...filter, unread: filter.unread ? undefined : true } }); void reloadOverview(); }}
          className={cn("inline-flex h-8 items-center gap-1.5 rounded-full px-3 text-xs font-medium ring-1 ring-inset",
            filter.unread ? "bg-select-strong ring-[var(--iw-red)]/40" : "text-muted-foreground ring-border hover:bg-accent hover:text-foreground")}
        >
          <Mail className="size-3.5" /> Unread only
        </button>
        <Button variant="outline" size="sm" disabled={syncing} onClick={async () => {
          setSyncing(true);
          try { await api.syncNow(); } catch (e) { notify.error(e, "Couldn't start a sync"); } finally { setSyncing(false); }
        }}>
          <RefreshCw className={cn(syncing && "wm-spin")} /> Check all
        </Button>
        {!standalone && (
          <Button variant="outline" size="sm" onClick={() => api.overviewWindowOpen().catch((e) => notify.error(e, "Couldn't open the window"))}>
            <ExternalLink /> Open in its own window
          </Button>
        )}
      </header>

      <div className="flex min-h-0 flex-1">
        {enabled.length === 0 ? (
          <EmptyState icon={<Inbox className="size-6" />} title="No accounts yet" body="Add an account and its inbox appears here as a column."
            action={<Button onClick={() => setUi({ addAccount: true })}>Add account</Button>} />
        ) : (
          <div role="list" aria-label="Inboxes" className="flex min-w-0 flex-1 gap-3 overflow-x-auto p-3">
            {enabled.map((a) => (
              <Column key={a.id} account={a} col={overview[a.id]} openId={openId} />
            ))}
          </div>
        )}
        {openId && (
          <div className="flex w-[min(52%,860px)] min-w-[420px] flex-col border-l border-border bg-surface">
            <div className="flex items-center justify-end border-b border-border px-2 py-1">
              <IconButton label="Close message" shortcut="Esc" size="iconSm" onClick={() => setState({ openId: undefined, selected: [] })}><X /></IconButton>
            </div>
            <div className="min-h-0 flex-1"><Reader dark={dark} maximized={false} onToggleMax={() => { /* the overview has no list to hide */ }} /></div>
          </div>
        )}
      </div>
    </section>
  );
}

function Column({ account, col, openId }: { account: Account; col?: OverviewColumn; openId?: string }) {
  const inbox = useStore((s) => s.folders.find((f) => f.accountId === account.id && f.role === "inbox"));
  const sync = useStore((s) => s.sync[account.id]);
  const status = sync?.status ?? account.status;
  const unread = inbox?.unread ?? 0;

  return (
    <div role="listitem" aria-label={`${account.name}, ${unread} unread`}
      className="flex w-[340px] min-w-[300px] shrink-0 flex-col overflow-hidden rounded-card bg-surface shadow-[var(--shadow-card)] ring-1 ring-border xl:flex-1">
      <div aria-hidden className="h-1 w-full" style={{ background: account.color }} />
      <div className="flex items-center gap-2.5 border-b border-border px-3.5 py-2.5">
        <span aria-hidden className="flex size-8 shrink-0 items-center justify-center rounded-lg text-[13px] font-bold text-white" style={{ background: account.color }}>
          {account.name.slice(0, 1).toUpperCase()}
        </span>
        <div className="min-w-0 flex-1">
          <div className="truncate text-[13.5px] font-semibold">{account.name}</div>
          <div className="truncate text-[11.5px] text-muted-foreground">{account.email}</div>
        </div>
        {status === "syncing" && <Spinner className="size-3.5 border-[1.5px] text-muted-foreground" />}
        {status === "authError" && (
          <button type="button" title={account.statusMessage ?? "Sign-in needed"} onClick={() => setUi({ settings: `account:${account.id}` })}
            className="rounded p-1 text-[var(--iw-warn-text)] hover:bg-accent"><AlertTriangle className="size-4" /><span className="sr-only">Sign-in needed</span></button>
        )}
        {unread > 0 && <span className="rounded-full bg-primary px-2 py-0.5 text-[11px] font-bold tabular-nums text-white">{unread > 999 ? "999+" : unread}</span>}
        {!isOverviewWindow() && inbox && (
          <IconButton label={`Open ${account.name} inbox`} size="iconSm" onClick={() => setView({ kind: "folder", folderId: inbox.id })}><ExternalLink /></IconButton>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {!col || (col.loading && col.items.length === 0) ? (
          <div aria-hidden className="space-y-3 p-3.5">
            {Array.from({ length: 6 }).map((_, i) => <div key={i} className="space-y-1.5"><Skeleton className="h-3 w-1/2" /><Skeleton className="h-3 w-5/6" /></div>)}
            <span className="sr-only">Loading {account.name}</span>
          </div>
        ) : col.error ? (
          <div role="alert" className="m-3 rounded-lg bg-[var(--iw-red-wash)] p-3 text-xs text-[var(--iw-red-text)]">
            {col.error}
            <button type="button" className="ml-1 font-semibold underline" onClick={() => reloadOverview(inbox ? [inbox.id] : undefined)}>Try again</button>
          </div>
        ) : col.items.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center p-6 text-center">
            <Inbox className="mb-2 size-6 text-muted-foreground" />
            <div className="text-[13px] font-medium">Inbox zero</div>
            <div className="text-xs text-muted-foreground">Nothing waiting in {account.name}.</div>
          </div>
        ) : (
          <ul role="list">
            {col.items.map((m) => <ColumnRow key={m.id} m={m} open={openId === m.id} />)}
            {col.total > col.items.length && inbox && (
              <li className="p-3 text-center">
                {isOverviewWindow()
                  ? <span className="text-xs text-muted-foreground">Showing the newest {col.items.length} of {col.total.toLocaleString()}</span>
                  : <Button variant="ghost" size="sm" onClick={() => setView({ kind: "folder", folderId: inbox.id })}>See all {col.total.toLocaleString()}</Button>}
              </li>
            )}
          </ul>
        )}
      </div>
    </div>
  );
}

function ColumnRow({ m, open }: { m: MessageSummary; open: boolean }) {
  const unread = !m.flags.seen || m.threadUnread > 0;
  return (
    <li className="group relative border-b border-border/60 last:border-b-0">
      <button
        type="button"
        aria-current={open ? "true" : undefined}
        onClick={() => setState({ openId: m.id, selected: [m.id], cursor: m.id })}
        className={cn("block w-full px-3.5 py-2.5 text-left transition-colors", open ? "bg-select-strong" : "hover:bg-accent/70")}
      >
        <div className="flex items-center gap-2">
          {unread && <span aria-label="Unread" className="size-2 shrink-0 rounded-full bg-[var(--iw-red)]" />}
          <span className={cn("min-w-0 flex-1 truncate text-[13px]", unread ? "font-semibold" : "text-foreground/85")}>{displayName(m.from)}</span>
          {m.threadCount > 1 && <span className="text-[11px] tabular-nums text-muted-foreground">{m.threadCount}</span>}
          {m.flags.flagged && <Star className="size-3.5 fill-[var(--iw-warn-text)] text-[var(--iw-warn-text)]" aria-label="Starred" />}
          {m.hasAttachments && <Paperclip className="size-3.5 text-muted-foreground" aria-label="Has attachments" />}
          <time className={cn("shrink-0 text-[11.5px] tabular-nums", unread ? "font-semibold text-[var(--iw-red-text)]" : "text-muted-foreground")}>{listDate(m.date)}</time>
        </div>
        <div className={cn("mt-0.5 truncate text-[12.5px]", unread ? "font-medium text-foreground" : "text-foreground/75")}>{m.subject || "(no subject)"}</div>
        <div className="truncate text-[12px] text-muted-foreground">{m.snippet}</div>
      </button>
      <div className="absolute right-2 top-2 hidden items-center gap-0.5 rounded-lg bg-surface/95 p-0.5 shadow-theme-sm ring-1 ring-border group-hover:flex group-focus-within:flex">
        <IconButton label="Archive" size="iconSm" onClick={() => A.archive([m.id])}><Archive /></IconButton>
        <IconButton label="Delete" size="iconSm" onClick={() => A.trash([m.id])}><Trash2 /></IconButton>
        <IconButton label={unread ? "Mark as read" : "Mark as unread"} size="iconSm" onClick={() => A.toggleRead([m.id])}>{unread ? <MailOpen /> : <Mail />}</IconButton>
      </div>
    </li>
  );
}
