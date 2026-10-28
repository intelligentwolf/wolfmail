import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";
import { format, isThisYear, isToday, isYesterday, differenceInCalendarDays } from "date-fns";
import type { Address } from "./types";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** List date: time today, "Yesterday", weekday within the week, "12 Mar" this year, "12/03/2025" before. */
export function listDate(ms: number): string {
  const d = new Date(ms);
  if (isToday(d)) return format(d, "HH:mm");
  if (isYesterday(d)) return "Yesterday";
  if (differenceInCalendarDays(new Date(), d) < 7) return format(d, "EEE");
  if (isThisYear(d)) return format(d, "d MMM");
  return format(d, "dd/MM/yyyy");
}

export function fullDate(ms: number): string {
  return format(new Date(ms), "EEE d MMM yyyy, HH:mm");
}

export function displayName(a?: Address): string {
  if (!a) return "(no sender)";
  return a.name?.trim() || a.email;
}

export function formatAddress(a: Address): string {
  return a.name ? `${a.name} <${a.email}>` : a.email;
}

export function initials(a?: Address): string {
  const n = displayName(a).replace(/["'<>]/g, "").trim();
  const parts = n.split(/[\s.@_-]+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[1][0]).toUpperCase();
}

/** Stable avatar hue from an address, so a sender keeps one colour everywhere. */
export function hueFor(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
  return h % 360;
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(1)} GB`;
}

const EMAIL_RE = /^[^\s@<>(),;:"]+@[^\s@<>(),;:"]+\.[^\s@<>(),;:"]+$/;
export function isEmail(s: string): boolean {
  return EMAIL_RE.test(s.trim());
}

/** Parse what a person typed or pasted into an address field: "Ann <a@b.com>, c@d.com; "Bob, Jr" <e@f.com>". */
export function parseAddresses(input: string): { ok: Address[]; bad: string[] } {
  const ok: Address[] = [];
  const bad: string[] = [];
  const parts: string[] = [];
  let cur = "";
  let inQuote = false;
  let inAngle = false;
  for (const ch of input) {
    if (ch === '"') inQuote = !inQuote;
    if (ch === "<") inAngle = true;
    if (ch === ">") inAngle = false;
    if ((ch === "," || ch === ";" || ch === "\n") && !inQuote && !inAngle) {
      parts.push(cur);
      cur = "";
    } else cur += ch;
  }
  parts.push(cur);
  for (const raw of parts) {
    const p = raw.trim();
    if (!p) continue;
    const m = p.match(/^(.*)<([^>]+)>$/);
    if (m) {
      const email = m[2].trim();
      const name = m[1].trim().replace(/^"|"$/g, "").trim();
      if (isEmail(email)) ok.push(name ? { name, email } : { email });
      else bad.push(p);
    } else if (isEmail(p)) ok.push({ email: p });
    else bad.push(p);
  }
  return { ok, bad };
}

export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform + navigator.userAgent);
export const modKey = isMac ? "⌘" : "Ctrl";

export function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

export function stripReplyPrefix(subject: string): string {
  return subject.replace(/^((re|fwd?|aw|sv|antw)\s*(\[\d+\])?\s*:\s*)+/i, "").trim();
}
