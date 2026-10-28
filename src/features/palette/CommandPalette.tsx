import * as React from "react";
import { Command } from "cmdk";
import * as D from "@radix-ui/react-dialog";
import {
  Search, PenSquare, Inbox, Star, FileText, Send, Archive, Trash2, Settings, RefreshCw, Moon, Sun, Rows3, Rows2, Folder,
  UserPlus, Upload, Keyboard, Mail, Link2, Download, Sparkles, LayoutGrid, AppWindow,
} from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { getState, openComposer, openSearchHit, setUi, setView, updateSettings, useStore } from "../../lib/store";
import type { SearchHit } from "../../lib/types";
import { displayName, listDate } from "../../lib/utils";
import { Avatar } from "../../components/ui/misc";
import { newDraftFor } from "../compose/drafts";
import { IW_ENABLED } from "../../lib/features";
import * as A from "../actions";

/** Ctrl+K: one box for everything. Typing searches every account's local index (FTS5) as well as commands and
 *  folders; `v` opens it straight in move-to-folder mode. Grouped results keep it scannable (Hick's law). */
export function CommandPalette() {
  const open = useStore((s) => s.ui.palette);
  const folders = useStore((s) => s.folders);
  const accounts = useStore((s) => s.accounts);
  const settings = useStore((s) => s.settings);
  const iw = useStore((s) => s.iw);
  const [q, setQ] = React.useState("");
  const [mode, setMode] = React.useState<"all" | "move">("all");
  const [hits, setHits] = React.useState<SearchHit[]>([]);
  const [searching, setSearching] = React.useState(false);
  const [searchError, setSearchError] = React.useState<string>();

  React.useEffect(() => {
    const h = (e: Event) => setMode(((e as CustomEvent).detail as "move") ?? "all");
    window.addEventListener("wm:palette-mode", h);
    return () => window.removeEventListener("wm:palette-mode", h);
  }, []);
  React.useEffect(() => { if (!open) { setQ(""); setHits([]); setMode("all"); setSearchError(undefined); } }, [open]);

  React.useEffect(() => {
    if (!open || mode !== "all" || q.trim().length < 2) { setHits([]); setSearching(false); return; }
    let live = true;
    setSearching(true);
    const t = setTimeout(() => {
      api.search(q.trim(), 30)
        .then((r) => { if (live) { setHits(r); setSearchError(undefined); } })
        .catch((e) => live && setSearchError((e as { message?: string }).message ?? String(e)))
        .finally(() => live && setSearching(false));
    }, 120);
    return () => { live = false; clearTimeout(t); };
  }, [q, open, mode]);

  const close = () => setUi({ palette: false });
  const run = (fn: () => void) => () => { close(); fn(); };
  const acctName = (id: string) => accounts.find((a) => a.id === id)?.name ?? "";
  const s = getState();
  const selAccount = s.list.items.find((m) => s.selected.includes(m.id))?.accountId;

  const item = "flex h-10 cursor-default items-center gap-3 rounded-lg px-3 text-[13px] aria-selected:bg-select-strong [&_svg]:size-4 [&_svg]:text-muted-foreground";
  const group = "px-1 [&_[cmdk-group-heading]]:px-3 [&_[cmdk-group-heading]]:pb-1 [&_[cmdk-group-heading]]:pt-3 [&_[cmdk-group-heading]]:text-[11px] [&_[cmdk-group-heading]]:font-semibold [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:tracking-wide [&_[cmdk-group-heading]]:text-muted-foreground";

  return (
    <D.Root open={open} onOpenChange={(o) => setUi({ palette: o })}>
      <D.Portal>
        <D.Overlay className="fixed inset-0 z-50 bg-black/35 backdrop-blur-[2px]" />
        <D.Content aria-modal="true" className="fixed left-1/2 top-[12vh] z-50 w-[min(680px,calc(100vw-32px))] -translate-x-1/2 overflow-hidden rounded-card bg-popover shadow-[var(--shadow-pop)] ring-1 ring-border">
          <D.Title className="sr-only">{mode === "move" ? "Move to folder" : "Command palette"}</D.Title>
          <D.Description className="sr-only">Search mail, folders and commands. Use the arrow keys and Enter.</D.Description>
          <Command label="Command palette" shouldFilter={true} loop>
            <div className="flex items-center gap-3 border-b border-border px-4">
              <Search className="size-[18px] text-muted-foreground" />
              <Command.Input
                value={q}
                onValueChange={setQ}
                autoFocus
                placeholder={mode === "move" ? "Move to which folder?" : "Search all mail, jump to a folder, or run a command…"}
                className="h-14 flex-1 bg-transparent text-[15px] outline-none placeholder:text-muted-foreground"
              />
              {searching && <span className="text-xs text-muted-foreground" aria-live="polite">Searching…</span>}
            </div>
            <Command.List className="max-h-[min(60vh,520px)] overflow-y-auto pb-2">
              <Command.Empty className="px-4 py-10 text-center text-[13px] text-muted-foreground">
                {searchError ? <span role="alert" className="text-[var(--iw-red-text)]">Search failed: {searchError}</span> : searching ? "Searching…" : "Nothing matches. Try fewer words, or from:name / subject:word."}
              </Command.Empty>

              {mode === "move" ? (
                <Command.Group heading={selAccount ? `Move to — ${acctName(selAccount)}` : "Select a message first"} className={group}>
                  {folders.filter((f) => f.accountId === selAccount && f.selectable).map((f) => (
                    <Command.Item key={f.id} value={`${f.path} ${f.name}`} onSelect={run(() => A.moveTo(f.id))} className={item}>
                      <Folder /> <span style={{ paddingLeft: f.depth * 10 }}>{f.name}</span>
                    </Command.Item>
                  ))}
                </Command.Group>
              ) : (
                <>
                  {hits.length > 0 && (
                    <Command.Group heading="Messages" className={group}>
                      {hits.map((h) => (
                        <Command.Item key={h.id} value={`msg-${h.id} ${q}`} onSelect={run(() => openSearchHit(h))} className={`${item} h-14`}>
                          <Avatar address={h.from} size={30} />
                          <div className="min-w-0 flex-1">
                            <div className="flex items-center gap-2">
                              <span className="truncate font-medium">{displayName(h.from)}</span>
                              <span className="truncate text-xs text-muted-foreground">· {acctName(h.accountId)}</span>
                              <span className="ml-auto shrink-0 text-xs tabular-nums text-muted-foreground">{listDate(h.date)}</span>
                            </div>
                            <div className="truncate text-xs text-muted-foreground"><span className="text-foreground/85">{h.subject}</span> — {h.highlight || h.snippet}</div>
                          </div>
                        </Command.Item>
                      ))}
                    </Command.Group>
                  )}
                  <Command.Group heading="Actions" className={group}>
                    <Command.Item value="all inboxes at once overview columns" onSelect={run(() => setView({ kind: "overview" }))} className={item}><LayoutGrid /> All inboxes at once</Command.Item>
                    <Command.Item value="all inboxes own window pop out overview" onSelect={run(() => api.overviewWindowOpen().catch((e) => notify.error(e, "Couldn't open the window")))} className={item}><AppWindow /> Open All inboxes in its own window</Command.Item>
                    {IW_ENABLED && <Command.Item value="ask find email describe ai search" onSelect={run(() => window.dispatchEvent(new CustomEvent("wm:ask", { detail: q.trim() || null })))} className={item}><Sparkles /> Ask — find an email by describing it</Command.Item>}
                    <Command.Item value="new message compose write" onSelect={run(() => openComposer({ mode: "new", draft: newDraftFor(), quoteOnTop: true }))} className={item}><PenSquare /> New message</Command.Item>
                    <Command.Item value="check mail sync refresh" onSelect={run(() => api.syncNow().catch((e) => notify.error(e, "Couldn't start a sync")))} className={item}><RefreshCw /> Check for new mail</Command.Item>
                    <Command.Item value="reply" onSelect={run(() => A.reply(false))} className={item}><Mail /> Reply to the open message</Command.Item>
                    <Command.Item value="archive" onSelect={run(() => A.archive())} className={item}><Archive /> Archive selected</Command.Item>
                    <Command.Item value="move to folder" onSelect={() => { setMode("move"); setQ(""); }} className={item}><Folder /> Move selected to…</Command.Item>
                    <Command.Item value="theme dark light toggle" onSelect={run(() => updateSettings({ theme: settings?.theme === "dark" ? "light" : "dark" }))} className={item}>
                      {settings?.theme === "dark" ? <Sun /> : <Moon />} Switch to {settings?.theme === "dark" ? "light" : "dark"} theme
                    </Command.Item>
                    <Command.Item value="layout list multiline table" onSelect={run(() => updateSettings({ listLayout: settings?.listLayout === "table" ? "multiline" : "table" }))} className={item}>
                      {settings?.listLayout === "table" ? <Rows2 /> : <Rows3 />} Use the {settings?.listLayout === "table" ? "multi-line" : "table"} message list
                    </Command.Item>
                    <Command.Item value="threads conversations group" onSelect={run(() => updateSettings({ threaded: !settings?.threaded }))} className={item}><Rows3 /> {settings?.threaded ? "Show individual messages" : "Group into conversations"}</Command.Item>
                    <Command.Item value="add account" onSelect={run(() => setUi({ addAccount: true }))} className={item}><UserPlus /> Add an email account</Command.Item>
                    <Command.Item value="import betterbird thunderbird" onSelect={run(() => setUi({ importer: true }))} className={item}><Download /> Import from Betterbird or Thunderbird</Command.Item>
                    <Command.Item value="outbox queued scheduled" onSelect={run(() => setUi({ outbox: true }))} className={item}><Upload /> Outbox and scheduled mail</Command.Item>
                    {!IW_ENABLED ? null : iw?.connected
                      ? <Command.Item value="intelligentwolf panel crm" onSelect={run(() => setUi({ iwPanel: !getState().ui.iwPanel }))} className={item}><Sparkles /> Show / hide the IntelligentWolf panel</Command.Item>
                      : <Command.Item value="connect intelligentwolf crm" onSelect={run(() => setUi({ iwConnect: true }))} className={item}><Link2 /> Connect IntelligentWolf</Command.Item>}
                    <Command.Item value="settings preferences" onSelect={run(() => setUi({ settings: "general" }))} className={item}><Settings /> Settings</Command.Item>
                    <Command.Item value="keyboard shortcuts help" onSelect={run(() => setUi({ settings: "shortcuts" }))} className={item}><Keyboard /> Keyboard shortcuts</Command.Item>
                  </Command.Group>
                  <Command.Group heading="Go to" className={group}>
                    {([["inbox", "All inboxes", Inbox], ["flagged", "Starred", Star], ["drafts", "Drafts", FileText], ["sent", "Sent", Send], ["archive", "Archive", Archive], ["trash", "Trash", Trash2]] as const).map(([id, label, Icon]) => (
                      <Command.Item key={id} value={`go ${label}`} onSelect={run(() => setView({ kind: "unified", unified: id }))} className={item}><Icon /> {label}</Command.Item>
                    ))}
                    {folders.filter((f) => f.selectable).map((f) => (
                      <Command.Item key={f.id} value={`folder ${acctName(f.accountId)} ${f.path}`} onSelect={run(() => setView({ kind: "folder", folderId: f.id }))} className={item}>
                        <Folder /> <span className="truncate">{f.name}</span>
                        <span className="ml-auto truncate pl-4 text-xs text-muted-foreground">{acctName(f.accountId)}</span>
                      </Command.Item>
                    ))}
                  </Command.Group>
                </>
              )}
            </Command.List>
          </Command>
        </D.Content>
      </D.Portal>
    </D.Root>
  );
}


