import * as React from "react";
import { save } from "@tauri-apps/plugin-dialog";
import {
  Archive, Trash2, CornerUpLeft, ReplyAll, Forward, Star, MailOpen, ShieldAlert, MoreHorizontal, FolderInput, Tag as TagIcon, Undo2,
  Paperclip, Download, ExternalLink, ImageOff, ShieldCheck, Code, BellOff, ChevronDown, Mail, PanelRightOpen, PanelRightClose,
  FileText, FileImage, FileArchive, FileSpreadsheet, Maximize2, Minimize2, AlarmClock, AlarmClockOff,
} from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { selectOpenRow, useStore, patchRows, accountById, togglePanel, undoLast } from "../../lib/store";
import type { Attachment, MessageFull, MessageSummary } from "../../lib/types";
import { bytes, cn, displayName, formatAddress, fullDate, listDate } from "../../lib/utils";
import { Avatar, EmptyState, Menu, Skeleton, Spinner } from "../../components/ui/misc";
import { Button, IconButton } from "../../components/ui/button";
import { Dialog } from "../../components/ui/dialog";
import { MessageBody } from "./MessageBody";
import { MoveMenuItems } from "../list/MoveMenu";
import { SnoozeItems } from "../snooze/SnoozeMenu";
import * as A from "../actions";
import { InviteCard } from "../calendar/InviteCard";

export function Reader({ dark, maximized, onToggleMax }: { dark: boolean; maximized: boolean; onToggleMax: () => void }) {
  const openId = useStore((s) => s.openId);
  const selected = useStore((s) => s.selected);
  const row = useStore(selectOpenRow);
  const iwPanel = useStore((s) => s.ui.iwPanel);

  if (selected.length > 1) {
    return (
      <div className="h-full bg-surface"><EmptyState icon={<Mail className="size-6" />} title={`${selected.length} conversations selected`}
        body="Archive, delete, move or tag them together with the bar above the list, or right-click any of them." /></div>
    );
  }
  if (!openId || !row) {
    return <div className="h-full bg-surface"><EmptyState icon={<Mail className="size-6" />} title="Nothing open" body="Pick a conversation on the left. Use J and K to move through the list and Enter to open." /></div>;
  }
  return <Conversation key={row.id} row={row} dark={dark} maximized={maximized} onToggleMax={onToggleMax} iwPanel={iwPanel} />;
}

/** True once `loading` has lasted `ms`. Opening an email used to paint a placeholder, then a second one, then the
 *  message, even when it loaded at once — a flash on every click (Paul, 29 Oct 2026). Loads quicker than this show
 *  nothing; slower ones get the skeleton. */
function useSlow(loading: boolean, ms = 300): boolean {
  const [slow, setSlow] = React.useState(false);
  React.useEffect(() => {
    if (!loading) { setSlow(false); return; }
    const t = setTimeout(() => setSlow(true), ms);
    return () => clearTimeout(t);
  }, [loading, ms]);
  return slow;
}

function Conversation({ row, dark, maximized, onToggleMax, iwPanel }: { row: MessageSummary; dark: boolean; maximized: boolean; onToggleMax: () => void; iwPanel: boolean }) {
  // A single message needs no thread fetch, so it renders on the first paint instead of after a placeholder.
  const [members, setMembers] = React.useState<MessageSummary[] | null>(() => (row.threadCount > 1 ? null : [row]));
  const [error, setError] = React.useState<string>();
  const iwConnected = useStore((s) => !!s.iw?.connected);
  const slow = useSlow(!members && !error);

  React.useEffect(() => {
    setError(undefined);
    if (row.threadCount <= 1) { setMembers([row]); return; }
    let live = true;
    setMembers(null);
    api.threadGet(row.threadId, row.accountId)
      .then((ms) => live && setMembers(ms.length ? ms : [row]))
      .catch((e) => live && setError((e as { message?: string }).message ?? String(e)));
    return () => { live = false; };
  }, [row.id, row.threadId, row.threadCount, row.accountId]); // eslint-disable-line react-hooks/exhaustive-deps

  const latest = members ? members[members.length - 1] : row;

  return (
    <article aria-label={row.subject || "(no subject)"} className="flex h-full min-w-0 flex-col bg-surface">
      <Toolbar row={row} latestId={latest.id} maximized={maximized} onToggleMax={onToggleMax} iwPanel={iwPanel} iwConnected={iwConnected} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-[920px] px-6 pb-16 pt-5">
          <h2 className="selectable font-title text-[22px] leading-tight">{row.subject || "(no subject)"}</h2>
          <SubjectMeta row={row} count={members?.length ?? row.threadCount} />
          {error ? (
            <div role="alert" className="mt-6 rounded-table bg-[var(--iw-red-wash)] p-4 text-sm text-[var(--iw-red-text)]">Couldn't load this conversation: {error}</div>
          ) : !members ? (
            slow ? <MessageSkeleton /> : null
          ) : (
            <div className="mt-5 space-y-3">
              {members.map((m, i) => (
                <MessageCard key={m.id} summary={m} defaultOpen={i === members.length - 1 || !m.flags.seen} dark={dark} topRowId={row.id} />
              ))}
            </div>
          )}
        </div>
      </div>
    </article>
  );
}

function SubjectMeta({ row, count }: { row: MessageSummary; count: number }) {
  const tags = useStore((s) => s.tags);
  const account = accountById(row.accountId);
  const folder = useStore((s) => s.folders.find((f) => f.id === row.folderId));
  const mtags = row.tags.map((k) => tags.find((t) => t.key === k)).filter(Boolean) as { key: string; name: string; color: string }[];
  return (
    <div className="mt-2 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
      {account && (
        <span className="inline-flex items-center gap-1.5 rounded-full bg-surface-raised px-2 py-0.5 ring-1 ring-border">
          <span className="size-2 rounded-full" style={{ background: account.color }} />{account.name}{folder ? ` · ${folder.name}` : ""}
        </span>
      )}
      {count > 1 && <span>{count} messages</span>}
      {mtags.map((t) => (
        <button key={t.key} type="button" onClick={() => A.toggleTag(t.key, [row.id])} title={`Remove tag ${t.name}`}
          className="rounded-full px-2 py-0.5 font-semibold" style={{ background: `${t.color}22`, color: t.color }}>{t.name} ×</button>
      ))}
    </div>
  );
}

function Toolbar({ row, latestId, maximized, onToggleMax, iwPanel, iwConnected }: {
  row: MessageSummary; latestId: string; maximized: boolean; onToggleMax: () => void; iwPanel: boolean; iwConnected: boolean;
}) {
  const tags = useStore((s) => s.tags);
  const lastUndo = useStore((s) => s.lastUndo);
  const [source, setSource] = React.useState<string | null>(null);
  return (
    <div role="toolbar" aria-label="Message actions" className="flex items-center gap-0.5 border-b border-border px-3 py-2">
      <Button size="sm" onClick={() => A.reply(false, latestId)}><CornerUpLeft /> Reply</Button>
      <IconButton label="Reply all" shortcut="Shift+R" onClick={() => A.reply(true, latestId)}><ReplyAll /></IconButton>
      <IconButton label="Forward" shortcut="F" onClick={() => A.forward(latestId)}><Forward /></IconButton>
      <span className="mx-1.5 h-5 w-px bg-border" />
      <IconButton label="Archive" shortcut="E" onClick={() => A.archive([row.id])}><Archive /></IconButton>
      <IconButton label="Delete" shortcut="Del" onClick={() => A.trash([row.id])}><Trash2 /></IconButton>
      <IconButton label="Junk" shortcut="!" onClick={() => A.junk(true, [row.id])}><ShieldAlert /></IconButton>
      <Menu.Root>
        <IconButtonTrigger label="Move to folder"><FolderInput /></IconButtonTrigger>
        <Menu.Content><MoveMenuItems kind="menu" onPick={(fid) => A.moveTo(fid, [row.id])} /></Menu.Content>
      </Menu.Root>
      <Menu.Root>
        <IconButtonTrigger label="Tag"><TagIcon /></IconButtonTrigger>
        <Menu.Content>
          {tags.map((t, i) => (
            <Menu.Check key={t.key} checked={row.tags.includes(t.key)} onCheckedChange={() => A.toggleTag(t.key, [row.id])}>
              <span className="size-2.5 rounded-full" style={{ background: t.color }} /> {t.name}
              {i < 9 && <span className="ml-auto pl-4 text-[11px] text-muted-foreground">{i + 1}</span>}
            </Menu.Check>
          ))}
        </Menu.Content>
      </Menu.Root>
      <IconButton label={row.flags.flagged ? "Unstar" : "Star"} shortcut="S" onClick={() => A.toggleFlag([row.id])}>
        <Star className={cn(row.flags.flagged && "fill-[var(--iw-warn-text)] text-[var(--iw-warn-text)]")} />
      </IconButton>
      <IconButton label="Mark as unread" shortcut="U" onClick={() => A.setRead(false, [row.id])}><MailOpen /></IconButton>
      {row.snoozedUntil ? (
        <IconButton label="Un-snooze" onClick={() => A.unsnooze([row.id])}><AlarmClockOff /></IconButton>
      ) : (
        <Menu.Root>
          <IconButtonTrigger label="Snooze (B)"><AlarmClock /></IconButtonTrigger>
          <Menu.Content><SnoozeItems kind="menu" ids={[row.id]} onPick={() => window.dispatchEvent(new CustomEvent("wm:snooze-pick", { detail: [row.id] }))} /></Menu.Content>
        </Menu.Root>
      )}
      <Menu.Root>
        <IconButtonTrigger label="More"><MoreHorizontal /></IconButtonTrigger>
        <Menu.Content align="end">
          {lastUndo && <Menu.Item onSelect={() => void undoLast()}><Undo2 /> Undo: {lastUndo}</Menu.Item>}
          <Menu.Item onSelect={() => api.messageRaw(latestId).then(setSource).catch((e) => notify.error(e, "Couldn't load the source"))}><Code /> View source</Menu.Item>
          <Menu.Item onSelect={() => A.junk(false, [row.id])}><ShieldCheck /> Not junk</Menu.Item>
          <Menu.Sub label="Copy to" icon={<FolderInput />}><MoveMenuItems kind="menu" onPick={(fid) => A.copyTo(fid, [row.id])} /></Menu.Sub>
        </Menu.Content>
      </Menu.Root>
      <div className="flex-1" />
      <IconButton label={maximized ? "Restore layout" : "Expand reader"} onClick={onToggleMax}>{maximized ? <Minimize2 /> : <Maximize2 />}</IconButton>
      {iwConnected && (
        <IconButton label={iwPanel ? "Hide IntelligentWolf panel" : "Show IntelligentWolf panel"} shortcut="I" onClick={() => togglePanel("iwPanel")}>
          {iwPanel ? <PanelRightClose /> : <PanelRightOpen />}
        </IconButton>
      )}
      <Dialog open={source !== null} onOpenChange={(o) => !o && setSource(null)} title="Message source" className="w-[min(960px,calc(100vw-32px))]">
        <pre className="selectable max-h-[70vh] overflow-auto whitespace-pre-wrap break-all rounded-lg bg-surface-inset p-4 font-mono text-xs">{source}</pre>
      </Dialog>
    </div>
  );
}

const IconButtonTrigger = React.forwardRef<HTMLButtonElement, { label: string; children: React.ReactNode }>(function T({ label, children, ...p }, ref) {
  return <Menu.Trigger asChild><IconButton ref={ref} label={label} {...p}>{children}</IconButton></Menu.Trigger>;
});

function MessageSkeleton() {
  return (
    <div aria-hidden className="mt-6 space-y-4 rounded-card bg-surface p-5 ring-1 ring-border">
      <div className="flex items-center gap-3"><Skeleton className="size-10 rounded-full" /><div className="flex-1 space-y-2"><Skeleton className="h-3 w-1/3" /><Skeleton className="h-3 w-1/2" /></div></div>
      <Skeleton className="h-3 w-full" /><Skeleton className="h-3 w-11/12" /><Skeleton className="h-3 w-4/5" /><Skeleton className="h-3 w-2/3" />
    </div>
  );
}

function MessageCard({ summary, defaultOpen, dark, topRowId }: { summary: MessageSummary; defaultOpen: boolean; dark: boolean; topRowId: string }) {
  const [open, setOpen] = React.useState(defaultOpen);
  const [full, setFull] = React.useState<MessageFull | null>(null);
  const [error, setError] = React.useState<string>();
  const [loadingRemote, setLoadingRemote] = React.useState(false);
  const [remoteLoaded, setRemoteLoaded] = React.useState(false);
  const [details, setDetails] = React.useState(false);
  const settings = useStore((s) => s.settings);
  const slow = useSlow(open && !full && !error);

  React.useEffect(() => {
    if (!open || full) return;
    let live = true;
    api.messageGet(summary.id)
      .then((m) => { if (live) setFull(m); })
      .catch((e) => live && setError((e as { message?: string }).message ?? String(e)));
    return () => { live = false; };
  }, [open, full, summary.id]);

  // Mark read after the configured delay while the message is open (settings.markReadAfterMs; -1 = never).
  React.useEffect(() => {
    if (!open || !full || full.flags.seen) return;
    const delay = settings?.markReadAfterMs ?? 0;
    if (delay < 0) return;
    const t = setTimeout(() => {
      api.setFlags([summary.id], ["seen"], [])
        .then(() => {
          setFull((f) => (f ? { ...f, flags: { ...f.flags, seen: true } } : f));
          patchRows([topRowId], (m) => ({
            ...m,
            threadUnread: Math.max(0, m.threadUnread - 1),
            flags: { ...m.flags, seen: m.id === summary.id ? true : m.flags.seen },
          }));
        })
        .catch((e) => notify.error(e, "Couldn't mark as read"));
    }, delay);
    return () => clearTimeout(t);
  }, [open, full, settings?.markReadAfterMs, summary.id, topRowId]);

  if (!open) {
    return (
      <button type="button" onClick={() => setOpen(true)} className="flex w-full items-center gap-3 rounded-card bg-surface px-4 py-3 text-left ring-1 ring-border hover:bg-accent/60">
        <Avatar address={summary.from} size={30} />
        <span className={cn("w-44 shrink-0 truncate text-[13px]", !summary.flags.seen && "font-semibold")}>{displayName(summary.from)}</span>
        <span className="min-w-0 flex-1 truncate text-[13px] text-muted-foreground">{summary.snippet}</span>
        <span className="shrink-0 text-xs tabular-nums text-muted-foreground">{listDate(summary.date)}</span>
      </button>
    );
  }

  const m = full;
  return (
    <div className="rounded-card bg-surface shadow-[var(--shadow-card)] ring-1 ring-border">
      <div className="flex items-start gap-3 px-5 pt-4">
        <Avatar address={summary.from} size={40} />
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-2">
            <button type="button" onClick={() => setOpen(false)} className="min-w-0 truncate text-left text-[14px] font-semibold hover:underline" title="Collapse">
              {displayName(summary.from)}
            </button>
            <span className="selectable min-w-0 truncate text-xs text-muted-foreground">&lt;{summary.from.email}&gt;</span>
            <div className="flex-1" />
            <time className="shrink-0 text-xs tabular-nums text-muted-foreground" title={fullDate(summary.date)}>{fullDate(summary.date)}</time>
          </div>
          <button type="button" onClick={() => setDetails((d) => !d)} aria-expanded={details} className="mt-0.5 flex max-w-full items-center gap-1 text-left text-xs text-muted-foreground hover:text-foreground">
            <span className="truncate">to {summary.to.map((a) => displayName(a)).join(", ") || "(no recipients)"}{summary.cc.length ? `, cc ${summary.cc.map((a) => displayName(a)).join(", ")}` : ""}</span>
            <ChevronDown className={cn("size-3.5 shrink-0 transition-transform", details && "rotate-180")} />
          </button>
          {details && m && (
            <dl className="selectable mt-2 grid grid-cols-[64px_1fr] gap-x-3 gap-y-1 rounded-lg bg-surface-inset p-3 text-xs">
              <dt className="text-muted-foreground">From</dt><dd>{formatAddress(m.from)}</dd>
              {m.replyTo.length > 0 && <><dt className="text-muted-foreground">Reply-To</dt><dd>{m.replyTo.map(formatAddress).join(", ")}</dd></>}
              <dt className="text-muted-foreground">To</dt><dd>{m.to.map(formatAddress).join(", ") || "—"}</dd>
              {m.cc.length > 0 && <><dt className="text-muted-foreground">Cc</dt><dd>{m.cc.map(formatAddress).join(", ")}</dd></>}
              {m.bcc.length > 0 && <><dt className="text-muted-foreground">Bcc</dt><dd>{m.bcc.map(formatAddress).join(", ")}</dd></>}
              <dt className="text-muted-foreground">Date</dt><dd>{fullDate(m.date)}</dd>
              {m.authResults && (
                <><dt className="text-muted-foreground">Checks</dt>
                  <dd className="flex flex-wrap gap-1.5">
                    {(["spf", "dkim", "dmarc"] as const).map((k) => m.authResults?.[k] && <AuthPill key={k} k={k.toUpperCase()} v={m.authResults[k]!} />)}
                  </dd></>
              )}
            </dl>
          )}
        </div>
      </div>

      {m?.listUnsubscribe && (
        <div className="mx-5 mt-3 flex items-center gap-2 text-xs text-muted-foreground">
          <BellOff className="size-3.5" /> This is a mailing list.
          <button type="button" className="font-medium text-[var(--iw-red-text)] hover:underline" onClick={() => A.unsubscribe(summary.id)}>Unsubscribe</button>
        </div>
      )}
      {m?.invite && <InviteCard messageId={summary.id} invite={m.invite} />}

      {m && (m.remoteContentBlocked || m.trackersRemoved > 0) && !remoteLoaded && (
        <div className="mx-5 mt-3 flex flex-wrap items-center gap-x-3 gap-y-1.5 rounded-lg bg-surface-raised px-3 py-2 text-xs ring-1 ring-border">
          <ImageOff className="size-4 text-muted-foreground" />
          <span className="min-w-0 flex-1">
            {m.remoteContentBlocked ? "Remote images are blocked to protect your privacy." : ""}
            {m.trackersRemoved > 0 ? ` ${m.trackersRemoved} tracking ${m.trackersRemoved === 1 ? "pixel" : "pixels"} removed.` : ""}
          </span>
          {m.remoteContentBlocked && (
            <Button size="sm" variant="outline" disabled={loadingRemote} onClick={async () => {
              setLoadingRemote(true);
              try { setFull(await api.messageLoadRemote(summary.id)); setRemoteLoaded(true); }
              catch (e) { notify.error(e, "Couldn't load the images"); }
              finally { setLoadingRemote(false); }
            }}>{loadingRemote && <Spinner />} Load images</Button>
          )}
        </div>
      )}

      <div className="px-5 pb-5 pt-3">
        {error ? (
          <div role="alert" className="rounded-lg bg-[var(--iw-red-wash)] p-3 text-sm text-[var(--iw-red-text)]">
            Couldn't load this message: {error}{" "}
            <button type="button" className="font-semibold underline" onClick={() => { setError(undefined); setFull(null); }}>Try again</button>
          </div>
        ) : !m ? (
          slow ? <div aria-hidden className="space-y-2 py-2"><Skeleton className="h-3 w-full" /><Skeleton className="h-3 w-5/6" /><Skeleton className="h-3 w-2/3" /></div> : null
        ) : (
          <>
            <MessageBody html={m.html} text={m.text} plainText={m.plainText} remote={remoteLoaded} dark={dark} />
            <Attachments message={m} />
            <div className="mt-4 flex gap-2">
              <Button variant="outline" size="sm" onClick={() => A.reply(false, m.id)}><CornerUpLeft /> Reply</Button>
              <Button variant="outline" size="sm" onClick={() => A.reply(true, m.id)}><ReplyAll /> Reply all</Button>
              <Button variant="outline" size="sm" onClick={() => A.forward(m.id)}><Forward /> Forward</Button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function AuthPill({ k, v }: { k: string; v: string }) {
  const pass = /^pass/i.test(v);
  const fail = /^(fail|softfail|permerror)/i.test(v);
  return (
    <span className={cn("rounded-full px-2 py-px text-[11px] font-semibold ring-1",
      pass ? "text-[var(--iw-go-text)] ring-[var(--iw-go-text)]/40" : fail ? "text-[var(--iw-red-text)] ring-[var(--iw-red)]/40" : "text-muted-foreground ring-border")}>
      {pass ? "✓" : fail ? "✕" : "•"} {k} {v}
    </span>
  );
}

function fileIcon(a: Attachment) {
  if (a.mime.startsWith("image/")) return FileImage;
  if (/zip|compressed|tar|gzip|7z|rar/.test(a.mime)) return FileArchive;
  if (/sheet|excel|csv/.test(a.mime)) return FileSpreadsheet;
  return FileText;
}

function Attachments({ message }: { message: MessageFull }) {
  const list = message.attachments.filter((a) => !a.inline);
  if (!list.length) return null;
  const total = list.reduce((n, a) => n + a.size, 0);
  return (
    <div className="mt-4 border-t border-border pt-3">
      <div className="mb-2 flex items-center gap-2 text-xs text-muted-foreground">
        <Paperclip className="size-3.5" /> {list.length} {list.length === 1 ? "attachment" : "attachments"} · {bytes(total)}
      </div>
      <ul role="list" className="flex flex-wrap gap-2">
        {list.map((a) => {
          const Icon = fileIcon(a);
          return (
            <li key={a.id} className="group flex w-60 items-center gap-2.5 rounded-lg bg-surface-raised px-3 py-2 ring-1 ring-border">
              <Icon className="size-5 shrink-0 text-[var(--iw-red-text)]" />
              <div className="min-w-0 flex-1">
                <div className="truncate text-[12.5px] font-medium" title={a.filename}>{a.filename}</div>
                <div className="text-[11px] text-muted-foreground">{bytes(a.size)}</div>
              </div>
              <IconButton label={`Open ${a.filename}`} size="iconSm" onClick={() => api.attachmentOpen(message.id, a.id).catch((e) => notify.error(e, "Couldn't open the attachment"))}><ExternalLink /></IconButton>
              <IconButton label={`Save ${a.filename}`} size="iconSm" onClick={async () => {
                const path = await save({ defaultPath: a.filename });
                if (!path) return;
                try { await api.attachmentSave(message.id, a.id, path); notify.ok(`Saved ${a.filename}`); }
                catch (e) { notify.error(e, "Couldn't save the attachment"); }
              }}><Download /></IconButton>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
