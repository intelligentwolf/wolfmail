// Calendar date helpers. Timed events are instants (ms); they are shown in the viewer's own zone. All-day events
// are stored as midnight UTC of their date(s) with an exclusive end (engine: pim/model.rs Event), so their dates
// are read with the UTC getters. Editing in another zone uses Intl to find that zone's offset at the instant.

export const DAY = 86_400_000;

export const localZone = (): string => {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
};

export function zoneList(): string[] {
  try {
    const all = (Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf?.("timeZone");
    if (all?.length) return all;
  } catch { /* older WebKit */ }
  return [localZone(), "UTC"];
}

/** Monday 00:00 local of the week containing `ms`. */
export function startOfWeek(ms: number): number {
  const d = new Date(ms);
  const dow = (d.getDay() + 6) % 7;
  d.setHours(0, 0, 0, 0);
  d.setDate(d.getDate() - dow);
  return d.getTime();
}

export function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

export function addDays(ms: number, n: number): number {
  const d = new Date(ms);
  d.setDate(d.getDate() + n);
  return d.getTime();
}

export function startOfMonth(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  d.setDate(1);
  return d.getTime();
}

export function sameDay(a: number, b: number): boolean {
  const x = new Date(a);
  const y = new Date(b);
  return x.getFullYear() === y.getFullYear() && x.getMonth() === y.getMonth() && x.getDate() === y.getDate();
}

/** Local-midnight of the calendar date an all-day value (midnight UTC) stands for. */
export function allDayToLocal(ms: number): number {
  const d = new Date(ms);
  return new Date(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate()).getTime();
}

/** Midnight UTC for the local calendar date of `ms` (the all-day storage form). */
export function localDateToAllDay(ms: number): number {
  const d = new Date(ms);
  return Date.UTC(d.getFullYear(), d.getMonth(), d.getDate());
}

function parts(ms: number, tz: string) {
  const f = new Intl.DateTimeFormat("en-GB", {
    timeZone: tz, year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23",
  });
  const o: Record<string, number> = {};
  for (const p of f.formatToParts(new Date(ms))) if (p.type !== "literal") o[p.type] = Number(p.value);
  return o as { year: number; month: number; day: number; hour: number; minute: number; second: number };
}

/** Offset (ms) of `tz` from UTC at instant `ms`. */
function offsetAt(ms: number, tz: string): number {
  const p = parts(ms, tz);
  return Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute, p.second) - Math.floor(ms / 1000) * 1000;
}

/** "2026-03-29" + "01:30" in `tz` → instant. Two passes settle DST edges (gap times move forward). */
export function zonedToMs(date: string, time: string, tz: string): number {
  const [y, m, d] = date.split("-").map(Number);
  const [hh, mm] = (time || "00:00").split(":").map(Number);
  const guess = Date.UTC(y, m - 1, d, hh, mm);
  let ms = guess - offsetAt(guess, tz);
  ms = guess - offsetAt(ms, tz);
  return ms;
}

/** Instant → { date: "YYYY-MM-DD", time: "HH:MM" } as seen in `tz`. */
export function msToZoned(ms: number, tz: string): { date: string; time: string } {
  const p = parts(ms, tz);
  const pad = (n: number) => String(n).padStart(2, "0");
  return { date: `${p.year}-${pad(p.month)}-${pad(p.day)}`, time: `${pad(p.hour)}:${pad(p.minute)}` };
}

export function utcDateString(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`;
}

export function dateStringToUtc(s: string): number {
  const [y, m, d] = s.split("-").map(Number);
  return Date.UTC(y, m - 1, d);
}

export const fmtTime = (ms: number) => new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
export const fmtDay = (ms: number) => new Date(ms).toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
export const fmtLongDay = (ms: number) => new Date(ms).toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long", year: "numeric" });
export const fmtMonth = (ms: number) => new Date(ms).toLocaleDateString(undefined, { month: "long", year: "numeric" });

/** "Tue 3 Mar, 09:00 – 10:00" / "Tue 3 Mar (all day)" for summaries and screen readers. */
export function fmtRange(start: number, end: number, allDay: boolean): string {
  if (allDay) {
    const s = allDayToLocal(start);
    const lastDay = allDayToLocal(end) - DAY;
    return lastDay > s ? `${fmtDay(s)} – ${fmtDay(lastDay)} (all day)` : `${fmtDay(s)} (all day)`;
  }
  return sameDay(start, end) ? `${fmtDay(start)}, ${fmtTime(start)} – ${fmtTime(end)}` : `${fmtDay(start)} ${fmtTime(start)} – ${fmtDay(end)} ${fmtTime(end)}`;
}

/** Readable foreground (dark ink or white) on a calendar colour — WCAG relative luminance, pick the higher contrast. */
export function inkOn(hex: string): string {
  const n = hex.replace("#", "");
  if (n.length !== 6) return "#ffffff";
  const ch = [0, 2, 4].map((i) => {
    const c = parseInt(n.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  const L = 0.2126 * ch[0] + 0.7152 * ch[1] + 0.0722 * ch[2];
  const vsWhite = 1.05 / (L + 0.05);
  const vsInk = (L + 0.05) / (0.0075 + 0.05); // #17141a: L = 0.2126·0.00857 + 0.7152·0.00687 + 0.0722·0.01034 ≈ 0.0075
  return vsWhite >= vsInk ? "#ffffff" : "#17141a";
}
