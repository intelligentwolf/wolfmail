import { useSyncExternalStore } from "react";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { pimApi, pimEvents } from "./pimApi";
import { notify } from "./notify";
import { modalOpen } from "./dom";
import type { AddressBook, CalEvent, Calendar, PimAccount } from "./pimTypes";

// Calendar & contacts UI state, kept apart from the mail store (lib/store.ts) so the two grow independently.

export type Mode = "mail" | "calendar" | "contacts";
export type CalView = "month" | "week" | "day" | "agenda";

/** What the event editor is showing: an existing event (with the occurrence that was clicked) or a new one. */
export type EditorState =
  | { kind: "new"; draft: Partial<CalEvent> }
  | { kind: "edit"; eventId: string; occurrenceStart: number; recurring?: boolean }
  | null;

type PimState = {
  mode: Mode;
  loaded: boolean;
  accounts: PimAccount[];
  calendars: Calendar[];
  books: AddressBook[];
  /** Bumped whenever calendar data changes, so views refetch their ranges. */
  version: number;
  view: CalView;
  /** Any instant inside the period shown (ms). */
  anchor: number;
  editor: EditorState;
  accountDialog: boolean;
  importDialog: boolean;
};

const savedView = (() => {
  try {
    const v = localStorage.getItem("wm.cal.view");
    return v === "month" || v === "week" || v === "day" || v === "agenda" ? v : "week";
  } catch {
    return "week";
  }
})() as CalView;

let state: PimState = {
  mode: "mail", loaded: false, accounts: [], calendars: [], books: [], version: 0,
  view: savedView, anchor: Date.now(), editor: null, accountDialog: false, importDialog: false,
};
const listeners = new Set<() => void>();
export function getPim() { return state; }
export function setPim(p: Partial<PimState> | ((s: PimState) => Partial<PimState>)) {
  state = { ...state, ...(typeof p === "function" ? p(state) : p) };
  listeners.forEach((l) => l());
}
export function usePim<T>(sel: (s: PimState) => T): T {
  return useSyncExternalStore((l) => { listeners.add(l); return () => { listeners.delete(l); }; }, () => sel(state), () => sel(state));
}

export async function refreshPim() {
  try {
    const [accounts, calendars, books] = await Promise.all([pimApi.accountsList(), pimApi.calendarsList(), pimApi.addressBooksList()]);
    setPim((s) => ({ accounts, calendars, books, loaded: true, version: s.version + 1 }));
  } catch (e) {
    notify.error(e, "Couldn't load calendars and contacts");
  }
}

export function setMode(mode: Mode) {
  setPim({ mode });
  if (mode !== "mail" && !state.loaded) void refreshPim();
}

export function setCalView(view: CalView) {
  setPim({ view });
  try { localStorage.setItem("wm.cal.view", view); } catch { /* per-viewer convenience only */ }
}

export function step(dir: -1 | 1) {
  const d = new Date(state.anchor);
  if (state.view === "month") d.setMonth(d.getMonth() + dir, 1);
  else if (state.view === "week") d.setDate(d.getDate() + 7 * dir);
  else if (state.view === "agenda") d.setDate(d.getDate() + 30 * dir);
  else d.setDate(d.getDate() + dir);
  setPim({ anchor: d.getTime() });
}

export function newEvent(draft: Partial<CalEvent> = {}) {
  setPim({ editor: { kind: "new", draft } });
}

let booted = false;
/** Engine events + keyboard. Called once from the app shell. */
export function bootPim() {
  if (booted) return;
  booted = true;
  void refreshPim();
  void pimEvents.changed(() => { void refreshPim(); });
  void pimEvents.status((e) => {
    setPim((s) => ({ accounts: s.accounts.map((a) => (a.id === e.accountId ? { ...a, status: e.status, statusMessage: e.message } : a)) }));
  });
  void pimEvents.notice((e) => notify.error({ kind: "server", message: e.message }, "Calendar change not saved"));
  void pimEvents.reminder(async (r) => {
    const when = r.allDay ? "today" : new Date(r.start).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
    const body = `${when}${r.location ? ` · ${r.location}` : ""}`;
    notify.info(`${r.title} — ${body}`, { duration: 15000 });
    try {
      let ok = await isPermissionGranted();
      if (!ok) ok = (await requestPermission()) === "granted";
      if (ok) sendNotification({ title: r.title, body });
    } catch { /* no notification service on this desktop; the in-app toast above still shows */ }
  });
  window.addEventListener("keydown", onKey);
}

function typing(t: EventTarget | null) {
  const el = t as HTMLElement | null;
  return !!el && (el.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName));
}

/** Ctrl/⌘+1/2/3 switch Mail/Calendar/Contacts everywhere; calendar keys work while the calendar is shown. */
function onKey(e: KeyboardEvent) {
  const mod = e.ctrlKey || e.metaKey;
  if (mod && !e.shiftKey && !e.altKey && ["1", "2", "3"].includes(e.key)) {
    e.preventDefault();
    setMode(e.key === "1" ? "mail" : e.key === "2" ? "calendar" : "contacts");
    return;
  }
  if (state.mode !== "calendar" || mod || e.altKey || typing(e.target) || modalOpen()) return;
  const k = e.key;
  const map: Record<string, () => void> = {
    t: () => setPim({ anchor: Date.now() }),
    j: () => step(1), n: () => step(1), ArrowRight: () => step(1),
    k: () => step(-1), p: () => step(-1), ArrowLeft: () => step(-1),
    m: () => setCalView("month"), w: () => setCalView("week"), d: () => setCalView("day"), a: () => setCalView("agenda"),
    c: () => newEvent(),
  };
  const f = map[k];
  if (f) { e.preventDefault(); f(); }
}

export function calendarById(id: string | undefined): Calendar | undefined {
  return id ? state.calendars.find((c) => c.id === id) : undefined;
}
