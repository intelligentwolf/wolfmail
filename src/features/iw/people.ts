import { useMemo, useSyncExternalStore } from "react";
import { api } from "../../lib/api";
import { getState, useStore } from "../../lib/store";
import type { Address, IwMatch, MessageSummary } from "../../lib/types";

/** Which addresses are people in IntelligentWolf, for highlighting rows and the right-click menu. Only addresses are
 *  sent (iw_people); answers are kept for TTL so scrolling the list does not ask again. `null` = asked, not a person. */
const TTL_MS = 10 * 60 * 1000;
type Entry = { match: IwMatch | null; at: number };

let cache = new Map<string, Entry>();
let version = 0;
const listeners = new Set<() => void>();
const pending = new Set<string>();
let timer: number | undefined;

function emit() { version++; listeners.forEach((l) => l()); }
function subscribe(l: () => void) { listeners.add(l); return () => { listeners.delete(l); }; }

/** Every address the user sends as (all accounts' identities), lowercased. */
export function useMyAddresses(): Set<string> {
  const accounts = useStore((s) => s.accounts);
  return useMemo(() => new Set(accounts.flatMap((a) => a.identities.map((i) => i.email.toLowerCase()))), [accounts]);
}

/** The person this row is about: the sender, or — for mail the user sent — the first recipient. */
export function otherParty(m: MessageSummary, mine: Set<string>): Address | undefined {
  return mine.has(m.from.email.toLowerCase()) ? m.to[0] : m.from;
}

/** Ask about these addresses (batched: everything requested in the same tick goes in one call). */
export function lookupPeople(emails: string[]) {
  if (!getState().iw?.connected) return;
  const now = Date.now();
  for (const raw of emails) {
    const e = raw.trim().toLowerCase();
    if (!e.includes("@")) continue;
    const hit = cache.get(e);
    if (!hit || now - hit.at > TTL_MS) pending.add(e);
  }
  if (!pending.size || timer !== undefined) return;
  timer = window.setTimeout(flush, 0);
}

async function flush() {
  timer = undefined;
  const batch = [...pending];
  pending.clear();
  try {
    const found = await api.iwPeople(batch);
    const at = Date.now();
    const next = new Map(cache);
    for (const e of batch) next.set(e, { match: found[e] ?? null, at });
    cache = next;
    emit();
  } catch {
    // Offline or not connected: the list simply shows no highlights, and the next list load asks again. Highlighting
    // is decoration, so it never raises an error toast of its own — the IW panel and the actions do that.
  }
}

/** Forget answers (after a contact is added or changed, or IntelligentWolf is connected/disconnected). */
export function invalidatePeople(email?: string) {
  if (email) { const next = new Map(cache); next.delete(email.toLowerCase()); cache = next; lookupPeople([email]); }
  else cache = new Map();
  emit();
}

export function useIwMatch(email: string | undefined): IwMatch | null | undefined {
  useSyncExternalStore(subscribe, () => version, () => version);
  return email ? cache.get(email.toLowerCase())?.match : undefined;
}
