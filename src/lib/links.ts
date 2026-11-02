/**
 * Web links written in an event's location or description ("https://meet.google.com/abc-defg-hij",
 * "www.example.com/agenda", "meet.google.com/abc-defg-hij"), in the order they appear, each once, as https/http
 * addresses that can be opened. Location first, so a meeting link put there is the first one.
 */
const LINK_RE = /\bhttps?:\/\/[^\s<>"'`]+|\bwww\.[^\s<>"'`]+|\b(?:meet\.google\.com|teams\.microsoft\.com|teams\.live\.com|(?:[\w-]+\.)?zoom\.us)\/[^\s<>"'`]+/gi;

export function findLinks(...texts: (string | undefined | null)[]): string[] {
  const out: string[] = [];
  for (const t of texts) {
    for (const m of (t ?? "").match(LINK_RE) ?? []) {
      // Punctuation that ends a sentence or closes brackets around the link is not part of it.
      const raw = m.replace(/[.,;:!?)\]}>]+$/, "");
      const url = /^https?:\/\//i.test(raw) ? raw : `https://${raw}`;
      try {
        const u = new URL(url);
        if ((u.protocol === "https:" || u.protocol === "http:") && !out.includes(u.href)) out.push(u.href);
      } catch { /* not an address after all */ }
    }
  }
  return out;
}

/** "meet.google.com/abc-defg-hij" — a link as a person reads it, without the scheme or a trailing slash. */
export function linkLabel(url: string): string {
  return url.replace(/^https?:\/\//i, "").replace(/\/$/, "");
}
