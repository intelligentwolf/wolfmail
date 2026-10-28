import { api } from "../lib/api";
import { notify } from "../lib/notify";
import { getState, moveCursor, openComposer, selectAll, setUi, setView, setState } from "../lib/store";
import { modKey } from "../lib/utils";
import { modalOpen } from "../lib/dom";
import * as A from "./actions";
import { newDraftFor } from "./compose/drafts";

// Single-key shortcuts follow Gmail/Superhuman conventions (j/k, e, r, a, f, s, u, #) because most switchers know
// them (Jakob's law); Thunderbird's Ctrl+N / Ctrl+R / Ctrl+Shift+R / Ctrl+L / Delete / F5 are kept as well so
// Betterbird muscle memory works too.
export const SHORTCUTS: { keys: string; label: string; group: string }[] = [
  { group: "Navigate", keys: "J / ↓", label: "Next conversation" },
  { group: "Navigate", keys: "K / ↑", label: "Previous conversation" },
  { group: "Navigate", keys: "Shift+J / Shift+K", label: "Extend selection" },
  { group: "Navigate", keys: "Enter", label: "Open in reader" },
  { group: "Navigate", keys: "G then O", label: "All inboxes at once (one column per account)" },
  { group: "Navigate", keys: "G then I", label: "Go to all inboxes" },
  { group: "Navigate", keys: "G then S", label: "Go to starred" },
  { group: "Navigate", keys: "G then D", label: "Go to drafts" },
  { group: "Navigate", keys: "G then T", label: "Go to sent" },
  { group: "Navigate", keys: "/", label: "Filter this view" },
  { group: "Navigate", keys: `${modKey}+K`, label: "Command palette and search everything" },
  { group: "Act", keys: `C / ${modKey}+N`, label: "New message" },
  { group: "Act", keys: `R / ${modKey}+R`, label: "Reply" },
  { group: "Act", keys: `Shift+R / A / ${modKey}+Shift+R`, label: "Reply all" },
  { group: "Act", keys: `F / ${modKey}+L`, label: "Forward" },
  { group: "Act", keys: "E", label: "Archive" },
  { group: "Act", keys: "# / Delete", label: "Delete" },
  { group: "Act", keys: "!", label: "Mark as junk" },
  { group: "Act", keys: "S", label: "Star / unstar" },
  { group: "Act", keys: "U", label: "Mark read / unread" },
  { group: "Act", keys: "1 – 9", label: "Toggle tag 1–9" },
  { group: "Act", keys: "V", label: "Move to folder (palette)" },
  { group: "Act", keys: `${modKey}+A`, label: "Select all in view" },
  { group: "App", keys: "F5", label: "Check for new mail" },
  { group: "App", keys: "I", label: "Show / hide the IntelligentWolf panel" },
  { group: "App", keys: `${modKey}+,`, label: "Settings" },
  { group: "App", keys: "?", label: "Show these shortcuts" },
  { group: "Compose", keys: `${modKey}+Enter`, label: "Send" },
  { group: "Compose", keys: `${modKey}+S`, label: "Save draft" },
  { group: "Compose", keys: "Esc", label: "Minimise the composer" },
];

function typingTarget(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  if (!el) return false;
  if (el.isContentEditable) return true;
  const tag = el.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
}

let gPending = 0;

export function installShortcuts(): () => void {
  const onKey = (e: KeyboardEvent) => {
    const s = getState();
    const mod = e.ctrlKey || e.metaKey;
    const k = e.key;

    // Global combos work everywhere, even in fields.
    if (mod && k.toLowerCase() === "k") { e.preventDefault(); setUi({ palette: true }); return; }
    if (mod && k === ",") { e.preventDefault(); setUi({ settings: "general" }); return; }
    if (k === "F5") { e.preventDefault(); api.syncNow().catch((err) => notify.error(err, "Couldn't start a sync")); return; }

    if (typingTarget(e.target)) return;
    // A dialog is open (Radix sets aria-modal on it) — let it own the keyboard.
    if (modalOpen()) return;
    if (s.composers.some((c) => !c.minimized) && (e.target as HTMLElement | null)?.closest?.('[role="dialog"]')) return;

    if (mod) {
      const lk = k.toLowerCase();
      if (lk === "n") { e.preventDefault(); openComposer({ mode: "new", draft: newDraftFor(), quoteOnTop: true }); return; }
      if (lk === "r" && e.shiftKey) { e.preventDefault(); void A.reply(true); return; }
      if (lk === "r") { e.preventDefault(); void A.reply(false); return; }
      if (lk === "l") { e.preventDefault(); void A.forward(); return; }
      if (lk === "a") { e.preventDefault(); selectAll(); return; }
      return;
    }
    if (e.altKey) return;

    if (Date.now() - gPending < 1200) {
      gPending = 0;
      const map: Record<string, Parameters<typeof setView>[0]> = {
        o: { kind: "overview" },
        i: { kind: "unified", unified: "inbox" }, s: { kind: "unified", unified: "flagged" }, d: { kind: "unified", unified: "drafts" },
        t: { kind: "unified", unified: "sent" }, a: { kind: "unified", unified: "archive" }, u: { kind: "unified", unified: "unread" },
      };
      const v = map[k.toLowerCase()];
      if (v) { e.preventDefault(); setView(v); }
      return;
    }

    switch (k) {
      case "j": case "ArrowDown": e.preventDefault(); moveCursor(1); return;
      case "k": case "ArrowUp": e.preventDefault(); moveCursor(-1); return;
      case "J": e.preventDefault(); moveCursor(1, true); return;
      case "K": e.preventDefault(); moveCursor(-1, true); return;
      case "Home": e.preventDefault(); moveCursor(-1e9); return;
      case "End": e.preventDefault(); moveCursor(1e9); return;
      case "Enter":
        if (s.cursor) {
          e.preventDefault();
          setState({ openId: s.cursor, selected: [s.cursor] });
          if (s.settings?.readingPane === "off") window.dispatchEvent(new Event("wm:maximize-reader"));
        }
        return;
      case "g": gPending = Date.now(); return;
      case "/": e.preventDefault(); window.dispatchEvent(new Event("wm:focus-quickfilter")); return;
      case "c": e.preventDefault(); openComposer({ mode: "new", draft: newDraftFor(), quoteOnTop: true }); return;
      case "r": e.preventDefault(); void A.reply(false); return;
      case "R": case "a": e.preventDefault(); void A.reply(true); return;
      case "f": e.preventDefault(); void A.forward(); return;
      case "e": e.preventDefault(); void A.archive(); return;
      case "#": case "Delete": e.preventDefault(); void A.trash(); return;
      case "!": e.preventDefault(); void A.junk(true); return;
      case "s": e.preventDefault(); void A.toggleFlag(); return;
      case "u": e.preventDefault(); void A.toggleRead(); return;
      case "v": e.preventDefault(); setUi({ palette: true }); window.dispatchEvent(new CustomEvent("wm:palette-mode", { detail: "move" })); return;
      case "i": e.preventDefault(); if (s.iw?.connected) setUi({ iwPanel: !s.ui.iwPanel }); return;
      case "?": e.preventDefault(); setUi({ settings: "shortcuts" }); return;
      case "Escape":
        if (s.view.kind === "overview" && s.openId) { e.preventDefault(); setState({ openId: undefined, selected: [] }); return; }
        if (s.selected.length > 1) { e.preventDefault(); setState({ selected: s.cursor ? [s.cursor] : [] }); }
        return;
    }
    if (/^[1-9]$/.test(k)) {
      const tag = s.tags[Number(k) - 1];
      if (tag) { e.preventDefault(); void A.toggleTag(tag.key); }
    }
  };
  window.addEventListener("keydown", onKey);
  return () => window.removeEventListener("keydown", onKey);
}
