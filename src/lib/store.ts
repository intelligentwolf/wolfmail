import { useSyncExternalStore } from "react";
import { api, events } from "./api";
import { notify } from "./notify";
import type {
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
  outbox: [],
  sync: {},
  view: { kind: "unified", unified: "inbox" },
  filter: {},
  search: "",
  list: { items: [], total: 0, loading: true, version: 0 },
  overview: {},
  selected: [],
  composers: [],
  ui: { settings: false, addAccount: false, importer: false, palette: false, iwConnect: false, iwPanel: true, shortcuts: false, outbox: false, collapsed: loadCollapsed() },
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
    const [accounts, folders, settings, tags, outbox] = await Promise.all([
      api.accountsList(), api.foldersList(), api.settingsGet(), api.tagsList(), api.outboxList(),
    ]);
    // outbox_list also returns recently sent items (engine keeps them briefly); the store tracks only pending ones.
    setState({ accounts, folders, settings, tags, outbox: outbox.filter((o) => o.state !== "sent"), ready: true, ...(isOverviewWindow() ? { view: { kind: "overview" as const } } : {}) });
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
  void events.undoable((u) => {
    notify.info(u.label, {
      duration: Math.max(1000, u.expiresAt - Date.now()),
      action: { label: "Undo", onClick: () => { api.undo(u.token).catch((err) => notify.error(err, "Couldn't undo")); } },
    });
  });
}

export async function refreshAccounts() {
  try { setState({ accounts: await api.accountsList() }); } catch (e) { notify.error(e, "Couldn't load accounts"); }
}
export async function refreshFolders() {
  try { setState({ folders: await api.foldersList() }); } catch (e) { notify.error(e, "Couldn't load folders"); }
}

// ── list ────────────────────────────────────────────────────────────────────
let listSeq = 0;
export async function reloadList(opts: { keepSelection?: boolean } = {}) {
  const seq = ++listSeq;
  const s = state;
  if (s.view.kind === "overview") {
    setState((st) => ({ list: { ...st.list, loading: false } }));
    return reloadOverview();
  }
  setState((st) => ({ list: { ...st.list, loading: true, error: undefined } }));
  try {
    const page = await api.messagesList({
      ...viewSource(s.view),
      filter: s.filter,
      search: s.search.trim() || undefined,
      threaded: s.settings?.threaded ?? true,
      sort: "dateDesc",
      offset: 0,
      limit: Math.max(PAGE, opts.keepSelection ? state.list.items.length : 0),
    });
    if (seq !== listSeq) return;
    const ids = new Set(page.items.map((m) => m.id));
    setState((st) => ({
      list: { items: page.items, total: page.total, loading: false, version: st.list.version + 1 },
      ...(opts.keepSelection
        ? { selected: st.selected.filter((id) => ids.has(id)), openId: st.openId }
        : { selected: [], cursor: page.items[0]?.id, anchor: undefined, openId: undefined }),
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
  try {
    const page = await api.messagesList({
      ...viewSource(s.view),
      filter: s.filter, search: s.search.trim() || undefined, threaded: s.settings?.threaded ?? true,
      sort: "dateDesc", offset: s.list.items.length, limit: PAGE,
    });
    if (seq !== listSeq) return;
    setState((st) => ({ list: { ...st.list, items: [...st.list.items, ...page.items], total: page.total } }));
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
      try {
        const page = await api.messagesList({
          folderId: inbox.id, filter: state.filter, search: state.search.trim() || undefined,
          threaded: state.settings?.threaded ?? true, sort: "dateDesc", offset: 0, limit: OVERVIEW_PER_COLUMN,
        });
        col = { items: page.items, total: page.total, loading: false };
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

/** After a move/trash/archive the rows vanish; put the cursor on the next row like every desktop client does. */
export function removeRowsLocally(ids: string[]) {
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
