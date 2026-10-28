import * as React from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  Archive, Trash2, Star, Paperclip, MailOpen, Mail, Search, X, Inbox, SearchX, AlertTriangle, CornerUpLeft, Forward,
  Tag as TagIcon, FolderInput, ShieldAlert, Filter, CheckSquare, RefreshCw, AlarmClock, AlarmClockOff, UserRound, UserPlus, UserPen, ExternalLink,
} from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { notify } from "../../lib/notify";
import type { IwMatch } from "../../lib/types";
import { lookupPeople, otherParty, useIwMatch, useMyAddresses } from "../iw/people";
import {
  useStore, getState, selectRow, loadMore, setFilter, setSearch, reloadList, selectAll, folderById, accountById,
} from "../../lib/store";
import type { MessageSummary, ViewFilter } from "../../lib/types";
import { format } from "date-fns";
import { cn, displayName, listDate } from "../../lib/utils";
import { Avatar, Ctx, EmptyState, Skeleton, Menu } from "../../components/ui/misc";
import { Button, IconButton } from "../../components/ui/button";
import * as A from "../actions";
import { MoveMenuItems } from "./MoveMenu";
import { SnoozeItems } from "../snooze/SnoozeMenu";

const viewTitles: Record<string, string> = {
  inbox: "All inboxes", unread: "Unread", snoozed: "Snoozed", flagged: "Starred", drafts: "Drafts", sent: "Sent", archive: "Archive", junk: "Junk", trash: "Trash",
};

export function MessageList() {
  const list = useStore((s) => s.list);
  const view = useStore((s) => s.view);
  const filter = useStore((s) => s.filter);
  const search = useStore((s) => s.search);
  const selected = useStore((s) => s.selected);
  const cursor = useStore((s) => s.cursor);
  const settings = useStore((s) => s.settings);
  const accountsCount = useStore((s) => s.accounts.length);
  const folder = useStore((s) => (s.view.kind === "folder" ? s.folders.find((f) => f.id === (s.view as { folderId: string }).folderId) : undefined));
  const account = useStore((s) => (folder ? s.accounts.find((a) => a.id === folder.accountId) : undefined));

  const layout = settings?.listLayout ?? "multiline";
  const compact = settings?.density === "compact";
  const rowH = layout === "table" ? (compact ? 30 : 36) : compact ? 70 : 84;

  // Highlight people who are in IntelligentWolf: ask about the rows' addresses (only addresses go, batched and cached).
  const iwConnected = useStore((s) => !!s.iw?.connected);
  const mine = useMyAddresses();
  React.useEffect(() => {
    if (!iwConnected) return;
    lookupPeople(list.items.map((m) => otherParty(m, mine)?.email ?? "").filter(Boolean));
  }, [list.items, iwConnected, mine]);

  const parentRef = React.useRef<HTMLDivElement>(null);
  const virt = useVirtualizer({ count: list.items.length, getScrollElement: () => parentRef.current, estimateSize: () => rowH, overscan: 12 });

  React.useEffect(() => { virt.measure(); }, [rowH, virt]);

  // Keep the keyboard cursor in view.
  React.useEffect(() => {
    if (!cursor) return;
    const i = list.items.findIndex((m) => m.id === cursor);
    if (i >= 0) virt.scrollToIndex(i, { align: "auto" });
  }, [cursor, list.items, virt]);

  const items = virt.getVirtualItems();
  React.useEffect(() => {
    const last = items[items.length - 1];
    if (last && last.index >= list.items.length - 25) void loadMore();
  }, [items, list.items.length]);

  const title = view.kind === "unified" ? viewTitles[view.unified] : view.kind === "overview" ? "All inboxes" : folder?.name ?? "Folder";
  const selectedSet = React.useMemo(() => new Set(selected), [selected]);
  const showAccount = view.kind === "unified" && accountsCount > 1;
  const isDraftView = (view.kind === "unified" && view.unified === "drafts") || folder?.role === "drafts";
  const filtered = !!search.trim() || Object.values(filter).some(Boolean);

  return (
    <section aria-label={`${title} messages`} className="flex h-full min-w-0 flex-col bg-surface">
      <header className="flex flex-col gap-2.5 border-b border-border px-4 pb-3 pt-4">
        <div className="flex items-center gap-2">
          {account && <span aria-hidden className="size-2.5 rounded-full" style={{ background: account.color }} />}
          <h1 className="min-w-0 truncate font-title text-[19px] leading-none">{title}</h1>
          {account && <span className="truncate text-xs text-muted-foreground">{account.name}</span>}
          <div className="flex-1" />
          <span className="text-xs tabular-nums text-muted-foreground" aria-live="polite">
            {list.loading && list.items.length === 0 ? "" : `${list.total.toLocaleString()} ${list.total === 1 ? "conversation" : "conversations"}`}
          </span>
        </div>
        <QuickFilter />
      </header>

      {selected.length > 1 && <BulkBar count={selected.length} />}

      <div
        ref={parentRef}
        className="relative min-h-0 flex-1 overflow-y-auto outline-none"
        role="listbox"
        aria-label="Messages"
        aria-multiselectable="true"
        aria-activedescendant={cursor ? `row-${cursor}` : undefined}
        tabIndex={0}
        data-message-list=""
      >
        {list.loading && list.items.length === 0 ? (
          <ListSkeleton rowH={rowH} layout={layout} />
        ) : list.error ? (
          <EmptyState tone="error" icon={<AlertTriangle className="size-6" />} title="Couldn't load messages" body={list.error}
            action={<Button variant="outline" onClick={() => reloadList()}><RefreshCw /> Try again</Button>} />
        ) : list.items.length === 0 ? (
          filtered ? (
            <EmptyState tone="filtered" icon={<SearchX className="size-6" />} title="No messages match"
              body={search ? `Nothing in ${title} matches “${search}”.` : "Nothing here matches the filters you picked."}
              action={<Button variant="outline" onClick={() => { setSearch(""); setFilter({}); }}>Clear filters</Button>} />
          ) : (
            view.kind === "unified" && view.unified === "snoozed" ? (
              <EmptyState icon={<AlarmClock className="size-6" />} title="Nothing snoozed"
                body="Snooze a conversation (press B, or right-click it) and it disappears until the time you choose, then comes back unread." />
            ) : (
              <EmptyState icon={<Inbox className="size-6" />} title={view.kind === "unified" && view.unified === "inbox" ? "Inbox zero" : `${title} is empty`}
                body={view.kind === "unified" && view.unified === "inbox" ? "Every inbox is clear. New mail shows up here the moment it arrives." : "There are no messages in this folder."} />
            )
          )
        ) : (
          <div style={{ height: virt.getTotalSize() + (layout === "table" ? 28 : 0), position: "relative" }}>
            {layout === "table" && <TableHeader />}
            {items.map((vi) => {
              const m = list.items[vi.index];
              return (
                <div key={m.id} style={{ position: "absolute", top: 0, left: 0, right: 0, height: vi.size, transform: `translateY(${vi.start + (layout === "table" ? 28 : 0)}px)` }}>
                  <Row m={m} layout={layout} compact={compact} selected={selectedSet.has(m.id)} isCursor={cursor === m.id} showAccount={showAccount} isDraft={isDraftView} />
                </div>
              );
            })}
          </div>
        )}
      </div>
    </section>
  );
}

function TableHeader() {
  return (
    <div className="sticky top-0 z-10 grid h-7 grid-cols-[28px_minmax(140px,1.1fr)_minmax(200px,3fr)_86px] items-center border-b border-border bg-surface px-3 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
      <span className="sr-only">Star</span><span>From</span><span>Subject</span><span className="text-right">Date</span>
    </div>
  );
}

function ListSkeleton({ rowH, layout }: { rowH: number; layout: string }) {
  return (
    <div aria-hidden className="space-y-0">
      {Array.from({ length: 12 }).map((_, i) => (
        <div key={i} className="flex items-center gap-3 border-b border-border/60 px-4" style={{ height: rowH }}>
          {layout === "multiline" && <Skeleton className="size-9 rounded-full" />}
          <div className="flex-1 space-y-2">
            <Skeleton className="h-3" style={{ width: `${30 + ((i * 37) % 40)}%` }} />
            {layout === "multiline" && <Skeleton className="h-3" style={{ width: `${50 + ((i * 53) % 40)}%` }} />}
            {layout === "multiline" && <Skeleton className="h-2.5 w-4/5" />}
          </div>
        </div>
      ))}
      <span className="sr-only">Loading messages</span>
    </div>
  );
}

function QuickFilter() {
  const search = useStore((s) => s.search);
  const filter = useStore((s) => s.filter);
  const tags = useStore((s) => s.tags);
  const ref = React.useRef<HTMLInputElement>(null);

  React.useEffect(() => {
    const h = (e: Event) => { ref.current?.focus(); ref.current?.select(); e.preventDefault(); };
    window.addEventListener("wm:focus-quickfilter", h);
    return () => window.removeEventListener("wm:focus-quickfilter", h);
  }, []);

  const chip = (key: keyof ViewFilter, label: string, icon: React.ReactNode) => {
    const on = !!filter[key];
    return (
      <button
        type="button"
        aria-pressed={on}
        onClick={() => setFilter({ ...filter, [key]: on ? undefined : true })}
        className={cn("inline-flex h-7 items-center gap-1.5 rounded-full px-2.5 text-xs font-medium ring-1 ring-inset transition-colors [&_svg]:size-3.5",
          on ? "bg-select-strong text-foreground ring-[var(--iw-red)]/40" : "text-muted-foreground ring-border hover:bg-accent hover:text-foreground")}
      >
        {icon}{label}
      </button>
    );
  };
  const activeTag = tags.find((t) => t.key === filter.tag);

  return (
    <div className="flex flex-col gap-2">
      <div className="relative">
        <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
        <input
          ref={ref}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") { setSearch(""); (document.querySelector("[data-message-list]") as HTMLElement | null)?.focus(); }
            if (e.key === "ArrowDown" || e.key === "Enter") { (document.querySelector("[data-message-list]") as HTMLElement | null)?.focus(); e.preventDefault(); }
          }}
          placeholder="Filter this view — from:, subject:, has:attachment"
          aria-label="Quick filter"
          className="h-8 w-full rounded-control bg-input-background pl-8 pr-14 text-[13px] placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-[var(--iw-red)] focus:shadow-focus-ring"
        />
        {search ? (
          <button type="button" aria-label="Clear filter text" onClick={() => setSearch("")} className="absolute right-1.5 top-1/2 flex size-6 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:bg-accent">
            <X className="size-3.5" />
          </button>
        ) : (
          <kbd className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 rounded border border-border px-1 text-[10.5px] text-muted-foreground">/</kbd>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        {chip("unread", "Unread", <Mail />)}
        {chip("flagged", "Starred", <Star />)}
        {chip("attachments", "Attachments", <Paperclip />)}
        <Menu.Root>
          <Menu.Trigger asChild>
            <button type="button" className={cn("inline-flex h-7 items-center gap-1.5 rounded-full px-2.5 text-xs font-medium ring-1 ring-inset [&_svg]:size-3.5",
              activeTag ? "bg-select-strong ring-[var(--iw-red)]/40" : "text-muted-foreground ring-border hover:bg-accent hover:text-foreground")}>
              {activeTag ? <span className="size-2 rounded-full" style={{ background: activeTag.color }} /> : <TagIcon />}
              {activeTag ? activeTag.name : "Tag"}
            </button>
          </Menu.Trigger>
          <Menu.Content align="start">
            <Menu.Item onSelect={() => setFilter({ ...filter, tag: undefined })}><Filter /> Any tag</Menu.Item>
            <Menu.Separator />
            {tags.map((t) => (
              <Menu.Item key={t.key} onSelect={() => setFilter({ ...filter, tag: t.key })}>
                <span className="size-2.5 rounded-full" style={{ background: t.color }} /> {t.name}
              </Menu.Item>
            ))}
          </Menu.Content>
        </Menu.Root>
      </div>
    </div>
  );
}

function BulkBar({ count }: { count: number }) {
  return (
    <div role="toolbar" aria-label={`${count} messages selected`} className="flex items-center gap-1 border-b border-border bg-select px-3 py-1.5">
      <span className="mr-2 text-xs font-semibold tabular-nums">{count} selected</span>
      <IconButton label="Archive" shortcut="E" size="iconSm" onClick={() => A.archive()}><Archive /></IconButton>
      <IconButton label="Delete" shortcut="Del" size="iconSm" onClick={() => A.trash()}><Trash2 /></IconButton>
      <IconButton label="Mark read / unread" shortcut="U" size="iconSm" onClick={() => A.toggleRead()}><MailOpen /></IconButton>
      <IconButton label="Star" shortcut="S" size="iconSm" onClick={() => A.toggleFlag()}><Star /></IconButton>
      <IconButton label="Junk" shortcut="!" size="iconSm" onClick={() => A.junk(true)}><ShieldAlert /></IconButton>
      <Menu.Root>
        <Menu.Trigger asChild><Button variant="ghost" size="sm"><AlarmClock /> Snooze</Button></Menu.Trigger>
        <Menu.Content><SnoozeItems kind="menu" onPick={() => window.dispatchEvent(new CustomEvent("wm:snooze-pick"))} /></Menu.Content>
      </Menu.Root>
      <Menu.Root>
        <Menu.Trigger asChild><Button variant="ghost" size="sm"><FolderInput /> Move</Button></Menu.Trigger>
        <Menu.Content><MoveMenuItems kind="menu" onPick={(fid) => A.moveTo(fid)} /></Menu.Content>
      </Menu.Root>
      <div className="flex-1" />
      <Button variant="ghost" size="sm" onClick={selectAll}><CheckSquare /> Select all</Button>
    </div>
  );
}

const Row = React.memo(function Row({
  m, layout, compact, selected, isCursor, showAccount, isDraft,
}: { m: MessageSummary; layout: "multiline" | "table"; compact: boolean; selected: boolean; isCursor: boolean; showAccount: boolean; isDraft: boolean }) {
  const tags = useStore((s) => s.tags);
  const account = accountById(m.accountId);
  const unread = !m.flags.seen || m.threadUnread > 0;
  const sentView = useStore((s) => (s.view.kind === "unified" ? s.view.unified === "sent" : folderById((s.view as { folderId: string }).folderId)?.role === "sent"));
  const who = sentView || isDraft ? (m.to.length ? m.to.map((a) => displayName(a)).join(", ") : "(no recipients)") : displayName(m.from);
  const mtags = m.tags.map((k) => tags.find((t) => t.key === k)).filter(Boolean) as { key: string; name: string; color: string }[];
  const mine = useMyAddresses();
  const iwMatch = useIwMatch(otherParty(m, mine)?.email);

  const onClick = (e: React.MouseEvent) => {
    if (e.shiftKey) selectRow(m.id, "range");
    else if (e.ctrlKey || e.metaKey) selectRow(m.id, "toggle");
    else selectRow(m.id, "single");
  };
  const onDouble = () => { if (isDraft) void A.editDraft(m.id); else window.dispatchEvent(new CustomEvent("wm:maximize-reader")); };
  const onDragStart = (e: React.DragEvent) => {
    const ids = selected ? getState().selected : [m.id];
    e.dataTransfer.setData("application/x-wolfmail-ids", JSON.stringify(ids));
    e.dataTransfer.effectAllowed = "copyMove";
  };

  const hover = (
    <div className="absolute right-2 top-1/2 hidden -translate-y-1/2 items-center gap-0.5 rounded-lg bg-surface/95 p-0.5 shadow-theme-sm ring-1 ring-border group-hover:flex">
      <IconButton label="Archive" shortcut="E" size="iconSm" onClick={(e) => { e.stopPropagation(); void A.archive([m.id]); }}><Archive /></IconButton>
      <IconButton label="Delete" shortcut="Del" size="iconSm" onClick={(e) => { e.stopPropagation(); void A.trash([m.id]); }}><Trash2 /></IconButton>
      <IconButton label={unread ? "Mark as read" : "Mark as unread"} shortcut="U" size="iconSm" onClick={(e) => { e.stopPropagation(); void A.toggleRead([m.id]); }}>
        {unread ? <MailOpen /> : <Mail />}
      </IconButton>
    </div>
  );

  const star = (
    <button
      type="button"
      aria-label={m.flags.flagged ? "Unstar" : "Star"}
      aria-pressed={m.flags.flagged}
      onClick={(e) => { e.stopPropagation(); void A.toggleFlag([m.id]); }}
      className={cn("flex size-6 items-center justify-center rounded hover:bg-accent", m.flags.flagged ? "text-[var(--iw-warn-text)]" : "text-muted-foreground/50 opacity-0 group-hover:opacity-100 focus-visible:opacity-100")}
    >
      <Star className={cn("size-4", m.flags.flagged && "fill-current")} />
    </button>
  );

  const body =
    layout === "table" ? (
      <div className="grid h-full grid-cols-[28px_minmax(140px,1.1fr)_minmax(200px,3fr)_86px] items-center gap-0 px-3 text-[13px]">
        {star}
        <span className={cn("flex min-w-0 items-center gap-2 truncate pr-3", unread && "font-semibold")}>
          {showAccount && account && <span aria-hidden className="size-2 shrink-0 rounded-full" style={{ background: account.color }} />}
          <span className="truncate">{who}</span>
          {iwMatch && <IwBadge match={iwMatch} />}
          {m.threadCount > 1 && <span className="shrink-0 text-[11px] text-muted-foreground tabular-nums">{m.threadCount}</span>}
        </span>
        <span className="flex min-w-0 items-center gap-2 pr-3">
          {mtags.map((t) => <span key={t.key} className="size-2 shrink-0 rounded-full" style={{ background: t.color }} title={t.name} />)}
          <span className={cn("truncate", unread && "font-semibold")}>{m.subject || "(no subject)"}</span>
          <span className="truncate text-muted-foreground">— {m.snippet}</span>
        </span>
        <span className={cn("flex items-center justify-end gap-1.5 text-xs tabular-nums", unread ? "font-semibold text-foreground" : "text-muted-foreground")}>
          {m.hasAttachments && <Paperclip className="size-3.5 text-muted-foreground" aria-label="Has attachments" />}
          {listDate(m.date)}
        </span>
      </div>
    ) : (
      <div className={cn("flex h-full gap-3 px-4", compact ? "py-2" : "py-3")}>
        <div className="relative mt-0.5 self-start">
          <Avatar address={sentView ? m.to[0] : m.from} size={compact ? 30 : 36} />
          {iwMatch && <span aria-hidden className="pointer-events-none absolute -inset-[3px] rounded-full ring-2 ring-[var(--iw-red)]/70" />}
          {showAccount && account && (
            <span aria-hidden title={account.name} className="absolute -bottom-0.5 -right-0.5 size-3 rounded-full ring-2 ring-surface" style={{ background: account.color }} />
          )}
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            {unread && <span aria-hidden className="size-2 shrink-0 rounded-full bg-[var(--iw-red)]" />}
            <span className={cn("min-w-0 truncate text-[13.5px]", unread ? "font-semibold" : "font-medium text-foreground/85")}>{who}</span>
            {iwMatch && <IwBadge match={iwMatch} />}
            {m.threadCount > 1 && (
              <span className="shrink-0 rounded-full bg-surface-raised px-1.5 text-[11px] font-medium tabular-nums text-muted-foreground ring-1 ring-border">{m.threadCount}</span>
            )}
            {m.flags.answered && <CornerUpLeft className="size-3.5 shrink-0 text-muted-foreground" aria-label="Replied" />}
            {m.flags.forwarded && <Forward className="size-3.5 shrink-0 text-muted-foreground" aria-label="Forwarded" />}
            <div className="flex-1" />
            {m.hasAttachments && <Paperclip className="size-3.5 shrink-0 text-muted-foreground" aria-label="Has attachments" />}
            {m.snoozedUntil ? (
              <span className="inline-flex shrink-0 items-center gap-1 text-xs font-medium text-[var(--iw-warn-text)]" title="Comes back then">
                <AlarmClock className="size-3.5" aria-hidden /> {format(new Date(m.snoozedUntil), "EEE HH:mm")}
                <span className="sr-only">snoozed until {new Date(m.snoozedUntil).toLocaleString()}</span>
              </span>
            ) : (
              <time className={cn("shrink-0 text-xs tabular-nums", unread ? "font-semibold text-[var(--iw-red-text)]" : "text-muted-foreground")} dateTime={new Date(m.date).toISOString()}>
                {listDate(m.date)}
              </time>
            )}
          </div>
          <div className="mt-0.5 flex items-center gap-2">
            <span className={cn("min-w-0 truncate text-[13px]", unread ? "font-semibold text-foreground" : "text-foreground/80")}>{m.subject || "(no subject)"}</span>
            <div className="flex-1" />
            {star}
          </div>
          {!compact && (
            <div className="mt-0.5 flex items-center gap-1.5">
              {mtags.map((t) => (
                <span key={t.key} className="inline-flex shrink-0 items-center gap-1 rounded-full px-1.5 py-px text-[10.5px] font-semibold" style={{ background: `${t.color}22`, color: t.color }}>
                  {t.name}
                </span>
              ))}
              <span className="min-w-0 truncate text-[12.5px] text-muted-foreground">{m.snippet}</span>
            </div>
          )}
        </div>
      </div>
    );

  return (
    <Ctx.Root onOpenChange={(o) => { if (o && !selected) selectRow(m.id, "single"); }}>
      <Ctx.Trigger asChild>
        <div
          id={`row-${m.id}`}
          role="option"
          aria-selected={selected}
          draggable
          onDragStart={onDragStart}
          onClick={onClick}
          onDoubleClick={onDouble}
          className={cn(
            "group relative h-full cursor-default border-b border-border/60 transition-colors duration-[var(--dur-fast)]",
            selected ? "bg-select-strong" : "hover:bg-accent/70",
            isCursor && "shadow-[inset_3px_0_0_var(--iw-red)]",
          )}
        >
          {body}
          {hover}
        </div>
      </Ctx.Trigger>
      <RowMenu m={m} isDraft={isDraft} iwMatch={iwMatch} />
    </Ctx.Root>
  );
});

/** "In IntelligentWolf" marker on a row: the person's kind in words, never colour alone (WCAG 1.4.1). */
function IwBadge({ match }: { match: IwMatch }) {
  return (
    <span title={`${match.name} is a ${match.kind} in IntelligentWolf`}
      className="inline-flex shrink-0 items-center gap-0.5 rounded-full bg-[var(--iw-red-wash)] px-1.5 py-px text-[10.5px] font-semibold text-[var(--iw-red-text)]">
      <UserRound aria-hidden className="size-3" />{match.kind === "lead" ? "Lead" : "Contact"}
    </span>
  );
}

function RowMenu({ m, isDraft, iwMatch }: { m: MessageSummary; isDraft: boolean; iwMatch: IwMatch | null | undefined }) {
  const tags = useStore((s) => s.tags);
  const iwConnected = useStore((s) => !!s.iw?.connected);
  const mine = useMyAddresses();
  const person = otherParty(m, mine);
  const unread = !m.flags.seen;
  return (
    <Ctx.Content>
      {isDraft && <Ctx.Item onSelect={() => A.editDraft(m.id)}>Edit draft</Ctx.Item>}
      <Ctx.Item shortcut="R" onSelect={() => A.reply(false, m.id)}><CornerUpLeft /> Reply</Ctx.Item>
      <Ctx.Item shortcut="Shift+R" onSelect={() => A.reply(true, m.id)}><CornerUpLeft /> Reply all</Ctx.Item>
      <Ctx.Item shortcut="F" onSelect={() => A.forward(m.id)}><Forward /> Forward</Ctx.Item>
      <Ctx.Separator />
      <Ctx.Item shortcut="E" onSelect={() => A.archive()}><Archive /> Archive</Ctx.Item>
      <Ctx.Item shortcut="U" onSelect={() => A.toggleRead()}>{unread ? <MailOpen /> : <Mail />} {unread ? "Mark as read" : "Mark as unread"}</Ctx.Item>
      <Ctx.Item shortcut="S" onSelect={() => A.toggleFlag()}><Star /> {m.flags.flagged ? "Unstar" : "Star"}</Ctx.Item>
      <Ctx.Sub label="Tag" icon={<TagIcon />}>
        {tags.map((t, i) => (
          <Ctx.Item key={t.key} shortcut={i < 9 ? String(i + 1) : undefined} onSelect={() => A.toggleTag(t.key)}>
            <span className="size-2.5 rounded-full" style={{ background: t.color }} /> {t.name}{m.tags.includes(t.key) ? " ✓" : ""}
          </Ctx.Item>
        ))}
      </Ctx.Sub>
      {m.snoozedUntil
        ? <Ctx.Item onSelect={() => A.unsnooze([m.id])}><AlarmClockOff /> Un-snooze</Ctx.Item>
        : <Ctx.Sub label="Snooze" icon={<AlarmClock />}><SnoozeItems kind="ctx" onPick={() => window.dispatchEvent(new CustomEvent("wm:snooze-pick"))} /></Ctx.Sub>}
      <Ctx.Item onSelect={() => window.dispatchEvent(new CustomEvent("wm:filter-from", { detail: m }))}><Filter /> Create filter from this sender…</Ctx.Item>
      {iwConnected && person && !isDraft && (
        <>
          <Ctx.Separator />
          {iwMatch ? (
            <>
              <Ctx.Item onSelect={() => openUrl(iwMatch.url).catch((e) => notify.error(e, "Couldn't open IntelligentWolf"))}>
                <ExternalLink /> Open {iwMatch.name} in IntelligentWolf
              </Ctx.Item>
              <Ctx.Item onSelect={() => window.dispatchEvent(new CustomEvent("wm:iw-update", { detail: m.id }))}>
                <UserPen /> Update {iwMatch.name} from this email…
              </Ctx.Item>
            </>
          ) : iwMatch === null ? (
            <Ctx.Item onSelect={() => window.dispatchEvent(new CustomEvent("wm:iw-update", { detail: m.id }))}>
              <UserPlus /> Add {displayName(person)} to IntelligentWolf…
            </Ctx.Item>
          ) : null}
        </>
      )}
      <Ctx.Sub label="Move to" icon={<FolderInput />}><MoveMenuItems kind="ctx" onPick={(fid) => A.moveTo(fid)} /></Ctx.Sub>
      <Ctx.Sub label="Copy to" icon={<FolderInput />}><MoveMenuItems kind="ctx" onPick={(fid) => A.copyTo(fid)} /></Ctx.Sub>
      <Ctx.Separator />
      <Ctx.Item shortcut="!" onSelect={() => A.junk(true)}><ShieldAlert /> Mark as junk</Ctx.Item>
      <Ctx.Item danger shortcut="Del" onSelect={() => A.trash()}><Trash2 /> Delete</Ctx.Item>
    </Ctx.Content>
  );
}
