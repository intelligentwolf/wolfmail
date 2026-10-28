import { api } from "../lib/api";
import { notify } from "../lib/notify";
import {
  findRow, getState, openComposer, patchRows, reloadList, refreshFolders, removeRowsLocally, selectedMessages, setUi,
} from "../lib/store";
import type { MessageSummary } from "../lib/types";
import { forwardDraft, replyDraft, splitDraftHtml } from "./compose/drafts";

// One place for every message action, so the toolbar, context menu, keyboard and command palette behave the same.
// Pattern: optimistic local change → engine call → on failure, reload the list from the engine (the source of truth)
// and show a visible error. Undo for moves comes from the engine's `undoable` event (store.ts).

function targets(ids?: string[]): MessageSummary[] {
  if (ids) return ids.map((id) => findRow(id)).filter((m): m is MessageSummary => !!m);
  return selectedMessages();
}

/** In conversation view a row stands for its whole thread, but its id is only the newest message. Acting on that
 *  one id left the rest of the conversation behind, so a deleted row "came back" showing the next message (Paul,
 *  28 Oct 2026). Expand each multi-message row to the thread's messages in the same folder (for Gmail, where one
 *  row per message lives in All Mail, that is the whole conversation — Gmail's own behaviour). */
async function conversationIds(rows: MessageSummary[]): Promise<string[]> {
  const ids = new Set(rows.map((r) => r.id));
  await Promise.all(
    rows.filter((r) => r.threadCount > 1).map(async (r) => {
      try {
        const members = await api.threadGet(r.threadId, r.accountId);
        for (const m of members) if (m.folderId === r.folderId) ids.add(m.id);
      } catch {
        // Fall back to the row itself; the action still runs and the list reload shows the truth.
      }
    }),
  );
  return [...ids];
}

async function guarded(label: string, fn: () => Promise<unknown>) {
  try {
    await fn();
  } catch (e) {
    notify.error(e, label);
    void reloadList({ keepSelection: true });
  } finally {
    void refreshFolders();
  }
}

export async function archive(ids?: string[]) {
  const rows = targets(ids);
  if (!rows.length) return;
  removeRowsLocally(rows.map((m) => m.id));
  await guarded("Couldn't archive", async () => { const all = await conversationIds(rows); await api.archive(all); });
}

export async function trash(ids?: string[]) {
  const rows = targets(ids);
  if (!rows.length) return;
  removeRowsLocally(rows.map((m) => m.id));
  await guarded("Couldn't delete", async () => { const all = await conversationIds(rows); await api.trash(all); });
}

export async function junk(isJunk: boolean, ids?: string[]) {
  const rows = targets(ids);
  if (!rows.length) return;
  removeRowsLocally(rows.map((m) => m.id));
  await guarded(isJunk ? "Couldn't mark as junk" : "Couldn't mark as not junk", async () => api.junk(await conversationIds(rows), isJunk));
}

export async function moveTo(folderId: string, ids?: string[]) {
  const rows = targets(ids);
  if (!rows.length) return;
  removeRowsLocally(rows.map((m) => m.id));
  await guarded("Couldn't move", async () => api.move(await conversationIds(rows), folderId));
}

export async function copyTo(folderId: string, ids?: string[]) {
  const t = targets(ids).map((m) => m.id);
  if (!t.length) return;
  await guarded("Couldn't copy", async () => {
    await api.copy(t, folderId);
    notify.ok(t.length === 1 ? "Copied" : `Copied ${t.length} messages`);
  });
}

export async function setRead(read: boolean, ids?: string[]) {
  const t = targets(ids);
  if (!t.length) return;
  // A conversation counts as unread while any message in it is (threadUnread), not just its newest one.
  const changed = t.filter((m) => (read ? !m.flags.seen || m.threadUnread > 0 : m.flags.seen)).map((m) => m.id);
  if (!changed.length) return;
  patchRows(changed, (m) => ({ ...m, flags: { ...m.flags, seen: read }, threadUnread: read ? 0 : Math.max(1, m.threadUnread) }));
  // Marking a conversation read covers its unread older messages too; marking unread only needs the newest.
  const rowsChanged = t.filter((m) => changed.includes(m.id));
  await guarded("Couldn't change read status", async () =>
    read ? api.setFlags(await conversationIds(rowsChanged), ["seen"], []) : api.setFlags(changed, [], ["seen"]));
}

export async function toggleRead(ids?: string[]) {
  const t = targets(ids);
  if (!t.length) return;
  const anyUnread = t.some((m) => !m.flags.seen);
  await setRead(anyUnread, t.map((m) => m.id));
}

export async function toggleFlag(ids?: string[]) {
  const t = targets(ids);
  if (!t.length) return;
  const flag = t.some((m) => !m.flags.flagged);
  const idList = t.map((m) => m.id);
  patchRows(idList, (m) => ({ ...m, flags: { ...m.flags, flagged: flag } }));
  await guarded("Couldn't star", () => (flag ? api.setFlags(idList, ["flagged"], []) : api.setFlags(idList, [], ["flagged"])));
}

export async function toggleTag(tag: string, ids?: string[]) {
  const t = targets(ids);
  if (!t.length) return;
  const add = t.some((m) => !m.tags.includes(tag));
  const idList = t.map((m) => m.id);
  patchRows(idList, (m) => ({ ...m, tags: add ? [...new Set([...m.tags, tag])] : m.tags.filter((x) => x !== tag) }));
  await guarded("Couldn't change the tag", () => (add ? api.setTags(idList, [tag], []) : api.setTags(idList, [], [tag])));
}

export async function reply(all: boolean, id?: string) {
  const target = id ?? getState().openId ?? getState().selected[0];
  if (!target) return;
  try {
    const msg = await api.messageGet(target);
    openComposer({ mode: all ? "replyAll" : "reply", ...replyDraft(msg, all), sourceId: msg.id });
  } catch (e) {
    notify.error(e, "Couldn't open the message to reply");
  }
}

export async function forward(id?: string) {
  const target = id ?? getState().openId ?? getState().selected[0];
  if (!target) return;
  try {
    const msg = await api.messageGet(target);
    openComposer({ mode: "forward", ...forwardDraft(msg), sourceId: msg.id });
  } catch (e) {
    notify.error(e, "Couldn't open the message to forward");
  }
}

export async function editDraft(id: string) {
  try {
    const draft = await api.draftGet(id);
    const parts = splitDraftHtml(draft.html);
    openComposer({ mode: "draft", draft: { ...draft, id: draft.id ?? id, html: parts.body }, quoteHtml: parts.quote, quoteOnTop: parts.quoteOnTop });
  } catch (e) {
    notify.error(e, "Couldn't open the draft");
  }
}

export async function unsubscribe(id: string) {
  try {
    const r = await api.unsubscribe(id);
    notify.ok(
      r.method === "oneClick" ? "Unsubscribed — the sender was told automatically"
        : r.method === "mailto" ? "Unsubscribe email sent"
        : "Opened the sender's unsubscribe page in your browser",
    );
  } catch (e) {
    notify.error(e, "Couldn't unsubscribe");
  }
}

export function openSettings(section = "general") { setUi({ settings: section }); }

/** Snooze: hidden until `until`, then back unread with a notification. Kept on this computer only. */
export async function snooze(until: number, ids?: string[]) {
  const rows = targets(ids);
  if (!rows.length) return;
  removeRowsLocally(rows.map((m) => m.id));
  try {
    const t = await conversationIds(rows);
    await api.snooze(t, until);
    notify.ok(`Snoozed until ${new Date(until).toLocaleString(undefined, { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" })}`, {
      action: { label: "Undo", onClick: () => { api.unsnooze(t).then(() => reloadList({ keepSelection: true })).catch((e) => notify.error(e, "Couldn't un-snooze")); } },
    });
  } catch (e) {
    notify.error(e, "Couldn't snooze");
    void reloadList({ keepSelection: true });
  } finally {
    void refreshFolders();
  }
}

export async function unsnooze(ids?: string[]) {
  const rows = targets(ids);
  if (!rows.length) return;
  removeRowsLocally(rows.map((m) => m.id));
  await guarded("Couldn't un-snooze", async () => api.unsnooze(await conversationIds(rows)));
}
