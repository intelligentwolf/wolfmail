import * as React from "react";
import {
  Inbox, Send, FileText, Trash2, ShieldAlert, Archive, Star, MailOpen, Folder as FolderIcon, ChevronRight, PenSquare,
  Plus, Settings as SettingsIcon, LayoutGrid, AlarmClock, Filter, RefreshCw, AlertTriangle, WifiOff, Mails, CheckCheck, FolderPlus, Pencil, Upload,
} from "lucide-react";
import { useStore, getState, setView, toggleCollapsed, setUi, openComposer, refreshFolders } from "../../lib/store";
import { moveTo, copyTo } from "../actions";
import { IW_ENABLED } from "../../lib/features";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import type { Account, Folder, FolderRole, UnifiedView } from "../../lib/types";
import { cn, modKey } from "../../lib/utils";
import { Button, IconButton } from "../../components/ui/button";
import { Ctx, Spinner } from "../../components/ui/misc";
import { Dialog, ConfirmDialog } from "../../components/ui/dialog";
import { Input, Field } from "../../components/ui/form";
import { newDraftFor } from "../compose/drafts";
import { ModeSwitch } from "../pim/ModeSwitch";

const roleIcon: Record<NonNullable<FolderRole>, React.ComponentType<{ className?: string }>> = {
  inbox: Inbox, sent: Send, drafts: FileText, trash: Trash2, junk: ShieldAlert, archive: Archive, all: Mails, flagged: Star, outbox: Upload,
};
const roleOrder: Record<string, number> = { inbox: 0, drafts: 1, sent: 2, outbox: 3, archive: 4, all: 5, flagged: 6, junk: 7, trash: 8 };

const unifiedItems: { id: UnifiedView; label: string; icon: React.ComponentType<{ className?: string }> }[] = [
  { id: "inbox", label: "All inboxes", icon: Inbox },
  { id: "unread", label: "Unread", icon: MailOpen },
  { id: "flagged", label: "Starred", icon: Star },
  { id: "drafts", label: "Drafts", icon: FileText },
  { id: "sent", label: "Sent", icon: Send },
  { id: "archive", label: "Archive", icon: Archive },
  { id: "junk", label: "Junk", icon: ShieldAlert },
  { id: "trash", label: "Trash", icon: Trash2 },
  { id: "snoozed", label: "Snoozed", icon: AlarmClock },
];

export function Sidebar() {
  const accounts = useStore((s) => s.accounts);
  const folders = useStore((s) => s.folders);
  const view = useStore((s) => s.view);
  const sync = useStore((s) => s.sync);
  const outbox = useStore((s) => s.outbox);
  const showAssistant = useStore((s) => s.settings?.showAssistant ?? true);
  const [syncing, setSyncing] = React.useState(false);

  const unifiedUnread = React.useMemo(() => {
    const enabled = new Set(accounts.filter((a) => a.enabled).map((a) => a.id));
    const sum = (role: FolderRole) => folders.filter((f) => f.role === role && enabled.has(f.accountId)).reduce((n, f) => n + f.unread, 0);
    return { inbox: sum("inbox"), unread: sum("inbox"), drafts: folders.filter((f) => f.role === "drafts" && enabled.has(f.accountId)).reduce((n, f) => n + f.total, 0), junk: sum("junk") } as Partial<Record<UnifiedView, number>>;
  }, [accounts, folders]);

  const anySyncing = Object.values(sync).some((s) => s.status === "syncing");

  return (
    <nav aria-label="Mailboxes" className="flex h-full flex-col bg-surface">
      <div className="flex items-center gap-2 px-4 pb-3 pt-4">
        <img src="/icon.png" alt="" className="size-7 rounded-lg" />
        <span className="font-title text-[17px] tracking-tight">WolfMail</span>
        <div className="flex-1" />
        <IconButton
          label="Check for new mail"
          shortcut="F5"
          size="iconSm"
          onClick={async () => {
            setSyncing(true);
            try { await api.syncNow(); } catch (e) { notify.error(e, "Couldn't start a sync"); } finally { setSyncing(false); }
          }}
        >
          <RefreshCw className={cn((syncing || anySyncing) && "wm-spin")} />
        </IconButton>
      </div>
      <ModeSwitch />

      <div className="px-3 pb-3">
        <Button
          className="h-10 w-full justify-start gap-2.5 rounded-xl text-[13.5px] shadow-theme-sm"
          onClick={() => openComposer({ mode: "new", draft: newDraftFor(), quoteOnTop: true })}
        >
          <PenSquare className="size-[18px]" /> New message
          <span className="ml-auto text-[11px] font-normal opacity-75">{modKey}+N</span>
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-4">
        <SectionLabel>Everywhere</SectionLabel>
        <ul role="list">
          <li>
            <NavRow
              active={view.kind === "overview"}
              icon={<LayoutGrid className="size-[17px]" />}
              label="All inboxes at once"
              onClick={() => setView({ kind: "overview" })}
            />
          </li>
          {unifiedItems.map((it) => (
            <li key={it.id}>
              <NavRow
                active={view.kind === "unified" && view.unified === it.id}
                icon={<it.icon className="size-[17px]" />}
                label={it.label}
                count={unifiedUnread[it.id]}
                countMuted={it.id === "drafts"}
                onClick={() => setView({ kind: "unified", unified: it.id })}
              />
            </li>
          ))}
          {outbox.length > 0 && (
            <li>
              <NavRow active={false} icon={<Upload className="size-[17px]" />} label="Outbox" count={outbox.length} countMuted onClick={() => setUi({ outbox: true })} />
            </li>
          )}
        </ul>

        {accounts.map((a) => (
          <AccountTree key={a.id} account={a} folders={folders.filter((f) => f.accountId === a.id)} />
        ))}

        <div className="mt-3 space-y-0.5 px-1">
          <Button variant="ghost" size="sm" className="w-full justify-start text-muted-foreground" onClick={() => setUi({ addAccount: true })}>
            <Plus /> Add account
          </Button>
        </div>
      </div>

      {/* The assistant wolf (features/assistant) sits over the left of this row: keep it clear of him. */}
      <div className={cn("flex min-h-[52px] items-center gap-1 border-t border-border px-3 py-2", IW_ENABLED && showAssistant && "pl-[72px]")}>
        {IW_ENABLED && <IwBadge />}
        <div className="flex-1" />
        <IconButton label="Settings" shortcut={`${modKey}+,`} size="iconSm" onClick={() => setUi({ settings: "general" })}>
          <SettingsIcon />
        </IconButton>
      </div>
    </nav>
  );
}

function IwBadge() {
  const iw = useStore((s) => s.iw);
  if (iw?.connected) {
    return (
      <button type="button" onClick={() => setUi({ settings: "iw" })} className="flex min-w-0 items-center gap-2 rounded-md px-1.5 py-1 text-left hover:bg-accent">
        <span className="relative flex size-2"><span className="absolute inline-flex size-full rounded-full bg-[var(--iw-go)] opacity-60" /><span className="relative inline-flex size-2 rounded-full bg-[var(--iw-go)]" /></span>
        <span className="truncate text-xs text-muted-foreground">{iw.org?.name ?? "IntelligentWolf"}</span>
      </button>
    );
  }
  return (
    <button type="button" onClick={() => setUi({ iwConnect: true })} className="flex items-center gap-1.5 rounded-md px-1.5 py-1 text-xs text-muted-foreground hover:bg-accent hover:text-foreground">
      <img src="/icon.png" alt="" className="size-4 rounded" /> Connect IntelligentWolf
    </button>
  );
}

function SectionLabel({ children, right }: { children: React.ReactNode; right?: React.ReactNode }) {
  return (
    <div className="flex items-center px-2.5 pb-1 pt-3">
      <span className="text-[11px] font-semibold uppercase tracking-[0.08em] text-muted-foreground">{children}</span>
      <div className="flex-1" />
      {right}
    </div>
  );
}

function NavRow({
  active, icon, label, count, countMuted, onClick, depth = 0, color, onToggle, collapsed, hasChildren, dim, onDropIds,
}: {
  active: boolean; icon: React.ReactNode; label: string; count?: number; countMuted?: boolean; onClick: () => void;
  depth?: number; color?: string; onToggle?: () => void; collapsed?: boolean; hasChildren?: boolean; dim?: boolean;
  onDropIds?: (ids: string[], copy: boolean) => void;
}) {
  const [over, setOver] = React.useState(false);
  const dropProps = onDropIds
    ? {
        onDragOver: (e: React.DragEvent) => {
          if (!e.dataTransfer.types.includes("application/x-wolfmail-ids")) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = e.ctrlKey || e.altKey ? "copy" : "move";
          setOver(true);
        },
        onDragLeave: () => setOver(false),
        onDrop: (e: React.DragEvent) => {
          setOver(false);
          const raw = e.dataTransfer.getData("application/x-wolfmail-ids");
          if (!raw) return;
          e.preventDefault();
          try { onDropIds(JSON.parse(raw) as string[], e.ctrlKey || e.altKey); } catch { /* not ours */ }
        },
      }
    : {};
  return (
    <div
      {...dropProps}
      className={cn(
        "group relative flex h-8 items-center rounded-lg pr-2 text-[13px] transition-colors",
        active ? "bg-select-strong font-semibold text-foreground" : "text-foreground/85 hover:bg-accent",
        dim && "opacity-60",
        over && "bg-select-strong ring-2 ring-inset ring-[var(--iw-red)]",
      )}
      style={{ paddingLeft: 6 + depth * 14 }}
    >
      {active && <span aria-hidden className="absolute left-0 top-1.5 bottom-1.5 w-[3px] rounded-full" style={{ background: color ?? "var(--iw-red)" }} />}
      {hasChildren ? (
        <button type="button" aria-label={collapsed ? `Expand ${label}` : `Collapse ${label}`} aria-expanded={!collapsed} onClick={onToggle} className="flex size-5 items-center justify-center rounded text-muted-foreground hover:text-foreground">
          <ChevronRight className={cn("size-3.5 transition-transform", !collapsed && "rotate-90")} />
        </button>
      ) : (
        <span className="w-5" />
      )}
      <button type="button" onClick={onClick} aria-current={active ? "page" : undefined} className="flex h-full min-w-0 flex-1 items-center gap-2.5 text-left">
        <span className={cn("shrink-0", active ? "text-[var(--iw-red-text)]" : "text-muted-foreground")}>{icon}</span>
        <span className="truncate">{label}</span>
        {count ? (
          <span className={cn("ml-auto rounded-full px-1.5 text-[11px] tabular-nums", countMuted ? "text-muted-foreground" : "bg-primary/90 font-semibold text-white")}>
            {count > 9999 ? "9999+" : count}
          </span>
        ) : null}
      </button>
    </div>
  );
}

/** Drop from the list. Messages from another account can't be moved by IMAP in one step, so they are refused
 *  with a visible reason instead of silently doing nothing. */
function dropOnFolder(ids: string[], f: Folder, copy: boolean) {
  const items = getState().list.items.filter((m) => ids.includes(m.id));
  const same = items.filter((m) => m.accountId === f.accountId).map((m) => m.id);
  if (same.length < ids.length) notify.error({ kind: "invalid", message: "Messages can only be moved between folders of the same account." }, "Can't move there");
  if (!same.length) return;
  if (copy) void copyTo(f.id, same);
  else void moveTo(f.id, same);
}

function sortFolders(fs: Folder[]): Folder[] {
  return [...fs].sort((a, b) => {
    const ra = a.role ? roleOrder[a.role] ?? 50 : 100;
    const rb = b.role ? roleOrder[b.role] ?? 50 : 100;
    if (ra !== rb) return ra - rb;
    return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
  });
}

function AccountTree({ account, folders }: { account: Account; folders: Folder[] }) {
  const view = useStore((s) => s.view);
  const collapsed = useStore((s) => s.ui.collapsed);
  const sync = useStore((s) => s.sync[account.id]);
  const status = sync?.status ?? account.status;
  const [newFolderFor, setNewFolderFor] = React.useState<Folder | null | undefined>(undefined);
  const [renaming, setRenaming] = React.useState<Folder | null>(null);
  const [deleting, setDeleting] = React.useState<Folder | null>(null);

  const byParent = React.useMemo(() => {
    const m = new Map<string | undefined, Folder[]>();
    for (const f of folders) {
      const k = f.parentId;
      m.set(k, [...(m.get(k) ?? []), f]);
    }
    for (const [k, v] of m) m.set(k, sortFolders(v));
    return m;
  }, [folders]);

  const inbox = folders.find((f) => f.role === "inbox");
  const isCollapsed = collapsed[account.id];

  const render = (parent: string | undefined, depth: number): React.ReactNode =>
    (byParent.get(parent) ?? []).map((f) => {
      const kids = byParent.get(f.id);
      const Icon = f.role ? roleIcon[f.role] : FolderIcon;
      const c = collapsed[f.id] ?? true;
      return (
        <li key={f.id}>
          <Ctx.Root>
            <Ctx.Trigger asChild>
              <div>
                <NavRow
                  depth={depth}
                  active={view.kind === "folder" && view.folderId === f.id}
                  icon={<Icon className="size-[16px]" />}
                  label={f.name}
                  count={f.role === "sent" || f.role === "trash" || f.role === "all" ? undefined : f.role === "drafts" ? f.total : f.unread}
                  countMuted={f.role === "drafts" || f.role === "junk"}
                  color={account.color}
                  dim={!f.selectable}
                  hasChildren={!!kids?.length}
                  collapsed={c}
                  onToggle={() => toggleCollapsed(f.id)}
                  onClick={() => (f.selectable ? setView({ kind: "folder", folderId: f.id }) : toggleCollapsed(f.id))}
                  onDropIds={f.selectable ? (ids, copy) => dropOnFolder(ids, f, copy) : undefined}
                />
              </div>
            </Ctx.Trigger>
            <Ctx.Content>
              <Ctx.Item onSelect={() => api.folderMarkRead(f.id).then(refreshFolders).catch((e) => notify.error(e, "Couldn't mark the folder read"))}><CheckCheck /> Mark all as read</Ctx.Item>
              <Ctx.Item onSelect={() => api.filtersRun(f.id)
                .then((n) => notify.ok(n === 0 ? `No filters matched anything in ${f.name}` : `Filters acted on ${n} message${n === 1 ? "" : "s"} in ${f.name}`))
                .catch((e) => notify.error(e, "Couldn't run filters"))}><Filter /> Run filters on this folder</Ctx.Item>
              <Ctx.Item onSelect={() => setNewFolderFor(f)}><FolderPlus /> New subfolder…</Ctx.Item>
              {!f.role && <Ctx.Item onSelect={() => setRenaming(f)}><Pencil /> Rename…</Ctx.Item>}
              {!f.role && <><Ctx.Separator /><Ctx.Item danger onSelect={() => setDeleting(f)}><Trash2 /> Delete folder…</Ctx.Item></>}
            </Ctx.Content>
          </Ctx.Root>
          {kids?.length && !c ? <ul role="list">{render(f.id, depth + 1)}</ul> : null}
        </li>
      );
    });

  return (
    <section aria-label={account.name} className="mt-2">
      <Ctx.Root>
        <Ctx.Trigger asChild>
          <div className="group flex h-8 items-center gap-1.5 rounded-lg px-1.5 hover:bg-accent">
            <button
              type="button"
              aria-expanded={!isCollapsed}
              onClick={() => toggleCollapsed(account.id)}
              className="flex min-w-0 flex-1 items-center gap-2 text-left"
            >
              <ChevronRight className={cn("size-3.5 shrink-0 text-muted-foreground transition-transform", !isCollapsed && "rotate-90")} />
              <span aria-hidden className="size-2.5 shrink-0 rounded-full ring-2 ring-surface" style={{ background: account.color }} />
              <span className={cn("truncate text-[12.5px] font-semibold", !account.enabled && "text-muted-foreground line-through")}>{account.name}</span>
            </button>
            <AccountStatusIcon status={status} message={sync?.message ?? account.statusMessage} progress={sync?.progress} />
            {isCollapsed && inbox && inbox.unread > 0 && (
              <span className="rounded-full px-1.5 text-[11px] font-semibold tabular-nums text-[var(--iw-red-text)]">{inbox.unread}</span>
            )}
          </div>
        </Ctx.Trigger>
        <Ctx.Content>
          <Ctx.Item onSelect={() => api.syncNow(account.id).catch((e) => notify.error(e, "Couldn't start a sync"))}><RefreshCw /> Check for new mail</Ctx.Item>
          <Ctx.Item onSelect={() => setNewFolderFor(null)}><FolderPlus /> New folder…</Ctx.Item>
          <Ctx.Separator />
          <Ctx.Item onSelect={() => setUi({ settings: `account:${account.id}` })}><SettingsIcon /> Account settings…</Ctx.Item>
        </Ctx.Content>
      </Ctx.Root>
      {!isCollapsed && <ul role="list">{render(undefined, 0)}</ul>}

      <FolderNameDialog
        open={newFolderFor !== undefined}
        title={newFolderFor ? `New folder inside “${newFolderFor.name}”` : `New folder in ${account.name}`}
        confirm="Create folder"
        onClose={() => setNewFolderFor(undefined)}
        onSubmit={async (name) => {
          await api.folderCreate(account.id, name, newFolderFor?.id);
          await refreshFolders();
          notify.ok(`Folder “${name}” created`);
        }}
      />
      <FolderNameDialog
        open={!!renaming}
        initial={renaming?.name}
        title={`Rename “${renaming?.name ?? ""}”`}
        confirm="Rename"
        onClose={() => setRenaming(null)}
        onSubmit={async (name) => {
          if (!renaming) return;
          await api.folderRename(renaming.id, name);
          await refreshFolders();
          notify.ok(`Renamed to “${name}”`);
        }}
      />
      <ConfirmDialog
        open={!!deleting}
        onOpenChange={(o) => !o && setDeleting(null)}
        title={`Delete “${deleting?.name ?? ""}”?`}
        body={<>The folder and the {deleting?.total ?? 0} messages in it are deleted on the server for every device. This can't be undone.</>}
        confirmLabel="Delete folder"
        destructive
        onConfirm={async () => {
          if (!deleting) return;
          try {
            await api.folderDelete(deleting.id);
            await refreshFolders();
            notify.ok(`Deleted “${deleting.name}”`);
          } catch (e) { notify.error(e, "Couldn't delete the folder"); }
        }}
      />
    </section>
  );
}

function AccountStatusIcon({ status, message, progress }: { status: string; message?: string; progress?: { done: number; total: number } }) {
  if (status === "syncing")
    return (
      <span className="flex items-center gap-1 text-[10.5px] tabular-nums text-muted-foreground" title={message ?? "Syncing"}>
        <Spinner className="size-3 border-[1.5px]" />
        {progress && progress.total > 0 ? `${Math.floor((progress.done / progress.total) * 100)}%` : null}
        <span className="sr-only">Syncing</span>
      </span>
    );
  if (status === "authError")
    return (
      <button type="button" onClick={() => setUi({ settings: "accounts" })} title={message ?? "Sign-in needed"} className="rounded p-0.5 text-[var(--iw-warn-text)] hover:bg-accent">
        <AlertTriangle className="size-3.5" /><span className="sr-only">Sign-in needed: {message}</span>
      </button>
    );
  if (status === "offline")
    return <span title={message ?? "Offline"} className="text-muted-foreground"><WifiOff className="size-3.5" /><span className="sr-only">Offline</span></span>;
  if (status === "error")
    return <span title={message} className="text-[var(--iw-red-text)]"><AlertTriangle className="size-3.5" /><span className="sr-only">Error: {message}</span></span>;
  return null;
}

function FolderNameDialog({ open, title, confirm, initial, onClose, onSubmit }: {
  open: boolean; title: string; confirm: string; initial?: string; onClose: () => void; onSubmit: (name: string) => Promise<void>;
}) {
  const [name, setName] = React.useState("");
  const [err, setErr] = React.useState<string>();
  const [busy, setBusy] = React.useState(false);
  React.useEffect(() => { if (open) { setName(initial ?? ""); setErr(undefined); } }, [open, initial]);
  const submit = async (e?: React.FormEvent) => {
    e?.preventDefault();
    const n = name.trim();
    if (!n) { setErr("Enter a folder name"); return; }
    if (/[/\\]/.test(n)) { setErr("Folder names can't contain / or \\ — use New subfolder instead"); return; }
    setBusy(true);
    try { await onSubmit(n); onClose(); } catch (ex) { setErr((ex as { message?: string }).message ?? String(ex)); } finally { setBusy(false); }
  };
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()} title={title}
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button onClick={() => submit()} disabled={busy}>{busy && <Spinner />}{confirm}</Button></>}>
      <form onSubmit={submit}>
        <Field id="folder-name" label="Folder name" error={err}>
          <Input autoFocus value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
      </form>
    </Dialog>
  );
}
