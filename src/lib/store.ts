import { useSyncExternalStore } from "react";
import { api, events } from "./api";
import { notify } from "./notify";
import type {
  SecretsStatus,
  Account, AccountStatus, DraftInput, Folder, IwStatus, MessageSummary, OutboxItem, Settings, Tag, UnifiedView, ViewFilter,
} from "./types";

export type View = { kind: "unified"; unified: UnifiedView } | { kind: "folder"; folderId: string } | { kind: "overview" };

/** One column of the All-inboxes overview: the newest messages of one account's Inbox. */
export type OverviewColumn = { items: MessageSummary[]; total: number; loading: boolean; error?: string };

export type ComposeMode = "new" | "reply" | "replyAll" | "forward" | "draft";
export type Composer = {
  key: string; mode: ComposeMode; draft: DraftInput; minimized: boolean; sourceId?: string;
  quoteHtml?: string; quoteOnTop: boolean; expanded?: boolean;
};

export type State = {
  ready: boolean;
  bootError?: string;
  accounts: Account[];
  folders: Folder[];
  settings: Settings | null;
  tags: Tag[];
  iw: IwStatus | null;
  /** Label of the latest move/delete that Ctrl/⌘+Z would undo, or null (undoLast). */
  lastUndo: string | null;
  secrets: SecretsStatus | null;
  outbox: OutboxItem[];
  sync: Record<string, { status: AccountStatus; message?: string; progress?: { done: number; total: number } }>;

  view: View;
  filter: ViewFilter;
  search: string;

  list: { items: MessageSummary[]; total: number; loading: boolean; error?: string; version: number };
  overview: Record<string, OverviewColumn>; // accountId → column
  selected: string[];      // message (row) ids
  cursor?: string;         // keyboard focus row
  anchor?: string;         // shift-select anchor
  openId?: string;         // row shown in the reader

  composers: Composer[];
  ui: {
    settings: false | string;      // false or the section id
    addAccount: boolean;
    importer: boolean;
    palette: boolean;
    iwConnect: boolean;
    iwPanel: boolean;
    sidebar: boolean;              // the mailboxes column on the left
    shortcuts: boolean;
    outbox: boolean;
    collapsed: Record<string, boolean>; // account id / folder id → collapsed in sidebar
  };
};

const PAGE = 200;

let state: State = {
  ready: false,
  accounts: [],
  folders: [],
  settings: null,
  tags: [],
  iw: null,
  lastUndo: null,
  secrets: null,
  outbox: [],
  sync: {},
  view: { kind: "unified", unified: "inbox" },
  filter: {},
  search: "",
  list: { items: [], total: 0, loading: true, version: 0 },
  overview: {},
  selected: [],
  composers: [],
  ui: { settings: false, addAccount: false, importer: false, palette: false, iwConnect: false, iwPanel: loadShown("wm.iwPanel"), sidebar: loadShown("wm.sidebar"), shortcuts: false, outbox: false, collapsed: loadCollapsed() },
};

const listeners = new Set<() => void>();
export function getState() { return state; }
export function setState(patch: Partial<State> | ((s: State) => Partial<State>)) {
  const p = typeof patch === "function" ? patch(state) : patch;
  state = { ...state, ...p };
  listeners.forEach((l) => l());
}
export function setUi(patch: Partial<State["ui"]>) {
  setState((s) => ({ ui: { ...s.ui, ...patch } }));
}
function subscribe(l: () => void) { listeners.add(l); return () => { listeners.delete(l); }; }

export function useStore<T>(sel: (s: State) => T): T {
  return useSyncExternalStore(subscribe, () => sel(state), () => sel(state));
}

/** The side panels remember whether they were shown (Paul, 29 Oct 2026: "most of the time i'm just looking at the
 *  emails themselves"). Per-viewer convenience, so localStorage; shown when nothing is stored or storage fails. */
function loadShown(key: string): boolean {
  try { return localStorage.getItem(key) !== "0"; } catch { return true; }
}
export function setPanel(panel: "sidebar" | "iwPanel", shown: boolean) {
  try { localStorage.setItem(panel === "sidebar" ? "wm.sidebar" : "wm.iwPanel", shown ? "1" : "0"); } catch { /* per-viewer convenience only */ }
  setState((s) => ({ ui: { ...s.ui, [panel]: shown } }));
}
export function togglePanel(panel: "sidebar" | "iwPanel") {
  setPanel(panel, !state.ui[panel]);
}

function loadCollapsed(): Record<string, boolean> {
  try { return JSON.parse(localStorage.getItem("wm.collapsed") || "{}"); } catch { return {}; }
}
export function toggleCollapsed(id: string) {
  setState((s) => {
    const collapsed = { ...s.ui.collapsed, [id]: !s.ui.collapsed[id] };
    try { localStorage.setItem("wm.collapsed", JSON.stringify(collapsed)); } catch { /* per-viewer convenience only */ }
    return { ui: { ...s.ui, collapsed } };
  });
}

// ── boot ────────────────────────────────────────────────────────────────────
/** True in the separate "All inboxes" window (opened at index.html#overview). */
export function isOverviewWindow(): boolean {
  return typeof window !== "undefined" && window.location.hash === "#overview";
}

let booted = false;
export async function boot() {
  // React StrictMode runs effects twice in development; a second boot would register every engine listener twice.
  if (booted) return;
  booted = true;
  try {
    const [accounts, folders, settings, tags, outbox, secrets] = await Promise.all([
      api.accountsList(), api.foldersList(), api.settingsGet(), api.tagsList(), api.outboxList(), api.secretsStatus(),
    ]);
    // outbox_list also returns recently sent items (engine keeps them briefly); the store tracks only pending ones.
    setState({ accounts, folders, settings, tags, secrets, outbox: outbox.filter((o) => o.state !== "sent"), ready: true, ...(isOverviewWindow() ? { view: { kind: "overview" as const } } : {}) });
    api.iwStatus().then((iw) => setState({ iw })).catch(() => setState({ iw: null }));
    await reloadList();
  } catch (e) {
    const msg = (e as { message?: string }).message ?? String(e);
    setState({ ready: true, bootError: msg });
  }

  void events.syncStatus((e) => {
    setState((s) => ({ sync: { ...s.sync, [e.accountId]: { status: e.status, message: e.message, progress: e.progress } } }));
    if (e.status === "authError" || e.status === "ok") void refreshAccounts();
  });
  void events.mailChanged((e) => {
    void refreshFolders();
    const v = state.view;
    if (v.kind === "overview" || isOverviewWindow()) { void reloadOverview(e.folderIds); return; }
    const visible = v.kind === "unified" || e.folderIds.includes(v.folderId);
    if (visible) void reloadList({ keepSelection: true });
  });
  void events.outboxChanged((item) => {
    setState((s) => {
      const others = s.outbox.filter((o) => o.id !== item.id);
      return { outbox: item.state === "sent" ? others : [...others, item] };
    });
    if (item.state === "sent") notify.ok(`Sent “${item.draft.subject || "(no subject)"}”`);
    if (item.state === "failed") notify.error({ kind: "server", message: item.error ?? "The message could not be sent. It is still in the Outbox." }, "Message not sent");
  });
  // No toast for moves and deletes (Paul, 28 Oct 2026: "get rid of the 'deleted 1 message undo' it makes messages
  // flicker — if they want to undo they can do ctrl z or use the menu"): the engine's undo tokens are kept here, and
  // Ctrl/⌘+Z, the command palette and the message menus undo the latest one (undoLast).
  void events.undoable((u) => {
    const now = Date.now();
    undoStack = [...undoStack.filter((x) => x.expiresAt > now), u].slice(-UNDO_KEEP);
    setState({ lastUndo: u.label });
  });
}

/** Recent undoable moves/deletes (engine `undoable` events), newest last. */
const UNDO_KEEP = 20;
let undoStack: { token: string; label: string; expiresAt: number }[] = [];

/** Undo the most recent move or delete that can still be undone. */
export async function undoLast() {
  const now = Date.now();
  undoStack = undoStack.filter((x) => x.expiresAt > now);
  const u = undoStack.pop();
  setState({ lastUndo: undoStack.length ? undoStack[undoStack.length - 1].label : null });
  if (!u) { notify.info("Nothing to undo"); return; }
  try {
    await api.undo(u.token);
    notify.ok(`Undone: ${u.label.charAt(0).toLowerCase()}${u.label.slice(1)}`);
  } catch (e) { notify.error(e, "Couldn't undo"); }
}

export async function refreshAccounts() {
  try { setState({ accounts: await api.accountsList() }); } catch (e) { notify.error(e, "Couldn't load accounts"); }
}
export async function refreshFolders() {
  try { setState({ folders: await api.foldersList() }); } catch (e) { notify.error(e, "Couldn't load folders"); }
}

// ── list ────────────────────────────────────────────────────────────────────
let listSeq = 0;
/** In an unread-only list the rows on screen stay put after they're read, so opening or selecting one doesn't make
 *  it vanish (Paul, 28 Oct 2026: selecting in Unread kept "refreshing" as each click marked a row read). A fresh
 *  load — new view, filter or search — starts from unread rows again. */
function keepIds(s: State, refresh: boolean): string[] | undefined {
  const unreadOnly = (s.view.kind === "unified" && s.view.unified === "unread") || !!s.filter.unread;
  return refresh && unreadOnly ? s.list.items.map((m) => m.id) : undefined;
}
export async function reloadList(opts: { keepSelection?: boolean } = {}) {
  const seq = ++listSeq;
  const s = state;
  if (s.view.kind === "overview") {
    setState((st) => ({ list: { ...st.list, loading: false } }));
    return reloadOverview();
  }
  setState((st) => ({ list: { ...st.list, loading: true, error: undefined } }));
  const gen = ++fetchGen;
  try {
    const page = await api.messagesList({
      ...viewSource(s.view),
      filter: s.filter,
      search: s.search.trim() || undefined,
      threaded: s.settings?.threaded ?? true,
      sort: "dateDesc",
      offset: 0,
      limit: Math.max(PAGE, opts.keepSelection ? state.list.items.length : 0),
      keepIds: keepIds(s, !!opts.keepSelection),
    });
    if (seq !== listSeq) return;
    const items = withoutRemoved(page.items, gen);
    const ids = new Set(items.map((m) => m.id));
    setState((st) => ({
      list: { items, total: page.total - (page.items.length - items.length), loading: false, version: st.list.version + 1 },
      ...(opts.keepSelection
        ? { selected: st.selected.filter((id) => ids.has(id)), openId: st.openId }
        : { selected: [], cursor: items[0]?.id, anchor: undefined, openId: undefined }),
    }));
  } catch (e) {
    if (seq !== listSeq) return;
    const msg = (e as { message?: string }).message ?? String(e);
    setState((st) => ({ list: { ...st.list, loading: false, error: msg } }));
  }
}

let loadingMore = false;
export async function loadMore() {
  const s = state;
  if (s.view.kind === "overview" || loadingMore || s.list.loading || s.list.items.length >= s.list.total) return;
  loadingMore = true;
  const seq = listSeq;
  const gen = ++fetchGen;
  try {
    const page = await api.messagesList({
      ...viewSource(s.view),
      filter: s.filter, search: s.search.trim() || undefined, threaded: s.settings?.threaded ?? true,
      sort: "dateDesc", offset: s.list.items.length, limit: PAGE, keepIds: keepIds(s, true),
    });
    if (seq !== listSeq) return;
    const items = withoutRemoved(page.items, gen);
    setState((st) => ({ list: { ...st.list, items: [...st.list.items, ...items], total: page.total - (page.items.length - items.length) } }));
  } catch (e) {
    notify.error(e, "Couldn't load more messages");
  } finally {
    loadingMore = false;
  }
}

function viewSource(v: View): { folderId?: string; unified?: UnifiedView } {
  if (v.kind === "folder") return { folderId: v.folderId };
  if (v.kind === "unified") return { unified: v.unified };
  return { unified: "inbox" };
}

// ── All-inboxes overview ────────────────────────────────────────────────────
const OVERVIEW_PER_COLUMN = 60;
let overviewSeq = 0;
/** Load every enabled account's Inbox in parallel; each column fails or succeeds on its own. */
export async function reloadOverview(onlyFolderIds?: string[]) {
  const seq = ++overviewSeq;
  const s = state;
  const cols = s.accounts
    .filter((a) => a.enabled)
    .map((a) => ({ a, inbox: s.folders.find((f) => f.accountId === a.id && f.role === "inbox") }))
    .filter(({ inbox }) => !onlyFolderIds || (inbox && onlyFolderIds.includes(inbox.id)));
  setState((st) => {
    const overview = { ...st.overview };
    for (const { a } of cols) overview[a.id] = { ...(overview[a.id] ?? { items: [], total: 0 }), loading: true, error: undefined };
    return { overview };
  });
  await Promise.all(cols.map(async ({ a, inbox }) => {
    let col: OverviewColumn;
    if (!inbox) {
      col = { items: [], total: 0, loading: false, error: a.status === "authError" ? "Sign-in needed before this inbox can sync." : "Inbox not synced yet." };
    } else {
      const gen = ++fetchGen;
      try {
        const page = await api.messagesList({
          folderId: inbox.id, filter: state.filter, search: state.search.trim() || undefined,
          threaded: state.settings?.threaded ?? true, sort: "dateDesc", offset: 0, limit: OVERVIEW_PER_COLUMN,
        });
        const items = withoutRemoved(page.items, gen);
        col = { items, total: page.total - (page.items.length - items.length), loading: false };
      } catch (e) {
        col = { items: [], total: 0, loading: false, error: (e as { message?: string }).message ?? String(e) };
      }
    }
    if (seq !== overviewSeq) return;
    setState((st) => ({ overview: { ...st.overview, [a.id]: col } }));
  }));
}

/** Every summary the UI currently knows about — the list plus the overview columns — so actions and the reader
 *  work the same whichever view the message was clicked in. */
export function knownRows(): MessageSummary[] {
  const s = state;
  return [...s.list.items, ...Object.values(s.overview).flatMap((c) => c.items)];
}
export function findRow(id: string | undefined): MessageSummary | undefined {
  if (!id) return undefined;
  return state.list.items.find((m) => m.id === id) ?? Object.values(state.overview).flatMap((c) => c.items).find((m) => m.id === id);
}

/** Selector: the summary row shown in the reader, from the list or an overview column (stable reference). */
export function selectOpenRow(s: State): MessageSummary | undefined {
  if (!s.openId) return undefined;
  const inList = s.list.items.find((m) => m.id === s.openId);
  if (inList) return inList;
  for (const c of Object.values(s.overview)) {
    const m = c.items.find((x) => x.id === s.openId);
    if (m) return m;
  }
  return undefined;
}

export function setView(view: View): Promise<void> {
  setState({ view, filter: {}, search: "" });
  return reloadList();
}
export function setFilter(filter: ViewFilter) {
  setState({ filter });
  void reloadList();
}
let searchTimer: ReturnType<typeof setTimeout> | undefined;
export function setSearch(search: string) {
  setState({ search });
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => void reloadList(), 160);
}

// ── selection ───────────────────────────────────────────────────────────────
export function selectRow(id: string, mode: "single" | "toggle" | "range" = "single") {
  setState((s) => {
    if (mode === "toggle") {
      const has = s.selected.includes(id);
      const selected = has ? s.selected.filter((x) => x !== id) : [...s.selected, id];
      return { selected, cursor: id, anchor: id, openId: selected.length === 1 ? selected[0] : s.openId };
    }
    if (mode === "range" && s.anchor) {
      const ids = s.list.items.map((m) => m.id);
      const a = ids.indexOf(s.anchor);
      const b = ids.indexOf(id);
      if (a >= 0 && b >= 0) {
        const [lo, hi] = a < b ? [a, b] : [b, a];
        return { selected: ids.slice(lo, hi + 1), cursor: id };
      }
    }
    return { selected: [id], cursor: id, anchor: id, openId: id };
  });
}
export function moveCursor(delta: number, extend = false) {
  const s = state;
  const ids = s.list.items.map((m) => m.id);
  if (ids.length === 0) return;
  const cur = s.cursor ? ids.indexOf(s.cursor) : -1;
  const next = ids[Math.min(ids.length - 1, Math.max(0, cur + delta))];
  if (extend) selectRow(next, "range");
  else selectRow(next, "single");
  if (next && ids.indexOf(next) > ids.length - 30) void loadMore();
}
export function selectedMessages(): MessageSummary[] {
  const set = new Set(state.selected);
  return knownRows().filter((m, i, all) => set.has(m.id) && all.findIndex((x) => x.id === m.id) === i);
}
export function selectAll() {
  setState((s) => ({ selected: s.list.items.map((m) => m.id) }));
}

/** Rows the user just moved away (delete/archive/move/junk/snooze) stay hidden while the engine does the server
 *  round trip: it changes the server first and its database only after, and background sync meanwhile sends
 *  mail://changed, so a list reload in between read the old rows and the deleted mail "came back, then disappeared
 *  again" (Paul, 29 Oct 2026). id → the last fetch that may still hold stale rows: Infinity while the action is in
 *  flight, then the fetch generation at the moment it finished. Only a fetch started after that is trusted. */
const removedRows = new Map<string, number>();
let fetchGen = 0;

function withoutRemoved(items: MessageSummary[], gen: number): MessageSummary[] {
  for (const [id, upTo] of removedRows) if (gen > upTo) removedRows.delete(id);
  return removedRows.size ? items.filter((m) => !removedRows.has(m.id)) : items;
}

/** The engine call behind a removeRowsLocally finished: on success the rows stay hidden from fetches that started
 *  before now; on failure they are released so the reload that follows shows them again. */
export function settleRemoved(ids: string[], ok: boolean) {
  for (const id of ids) {
    if (ok) removedRows.set(id, fetchGen);
    else removedRows.delete(id);
  }
}

/** After a move/trash/archive the rows vanish; put the cursor on the next row like every desktop client does.
 *  Pair with settleRemoved once the engine call ends. */
export function removeRowsLocally(ids: string[]) {
  for (const id of ids) removedRows.set(id, Infinity);
  setState((s) => {
    const gone = new Set(ids);
    const overview = Object.fromEntries(Object.entries(s.overview).map(([k, c]) => {
      const items = c.items.filter((m) => !gone.has(m.id));
      return [k, { ...c, items, total: Math.max(0, c.total - (c.items.length - items.length)) }];
    }));
    const all = s.list.items;
    const firstIdx = all.findIndex((m) => gone.has(m.id));
    const items = all.filter((m) => !gone.has(m.id));
    const next = items[Math.min(Math.max(firstIdx, 0), items.length - 1)];
    return {
      overview,
      list: { ...s.list, items, total: Math.max(0, s.list.total - (all.length - items.length)) },
      selected: next ? [next.id] : [],
      cursor: next?.id,
      anchor: next?.id,
      openId: s.openId && gone.has(s.openId) ? next?.id : s.openId,
    };
  });
}

export function patchRows(ids: string[], fn: (m: MessageSummary) => MessageSummary) {
  const set = new Set(ids);
  setState((s) => ({
    list: { ...s.list, items: s.list.items.map((m) => (set.has(m.id) ? fn(m) : m)) },
    overview: Object.fromEntries(Object.entries(s.overview).map(([k, c]) => [k, { ...c, items: c.items.map((m) => (set.has(m.id) ? fn(m) : m)) }])),
  }));
}

// ── helpers ─────────────────────────────────────────────────────────────────
export function accountById(id: string | undefined): Account | undefined {
  return id ? state.accounts.find((a) => a.id === id) : undefined;
}
export function folderById(id: string | undefined): Folder | undefined {
  return id ? state.folders.find((f) => f.id === id) : undefined;
}
export function roleFolder(accountId: string, role: Folder["role"]): Folder | undefined {
  return state.folders.find((f) => f.accountId === accountId && f.role === role);
}

export async function updateSettings(patch: Partial<Settings>) {
  const before = state.settings;
  if (before) setState({ settings: { ...before, ...patch } });
  try {
    setState({ settings: await api.settingsSet(patch) });
    if ("threaded" in patch) void reloadList();
  } catch (e) {
    setState({ settings: before });
    notify.error(e, "Couldn't save the setting");
  }
}

let composerSeq = 0;
export function openComposer(c: Omit<Composer, "key" | "minimized">) {
  const key = `c${++composerSeq}`;
  setState((s) => ({ composers: [...s.composers.map((x) => ({ ...x, minimized: true })), { ...c, key, minimized: false }] }));
  return key;
}
export function closeComposer(key: string) {
  setState((s) => ({ composers: s.composers.filter((c) => c.key !== key) }));
}
export function updateComposer(key: string, patch: Partial<Composer>) {
  setState((s) => ({ composers: s.composers.map((c) => (c.key === key ? { ...c, ...patch } : c)) }));
}

/** Open a search hit: switch to its folder and select it, so the reader and list stay in step. */
export async function openSearchHit(h: { id: string; folderId: string; threadId: string }) {
  await setView({ kind: "folder", folderId: h.folderId });
  const row = state.list.items.find((m) => m.id === h.id || m.threadId === h.threadId);
  if (row) setState({ selected: [row.id], cursor: row.id, openId: row.id });
  else notify.info("That message is further down this folder — type in the filter box to narrow the list.");
}
