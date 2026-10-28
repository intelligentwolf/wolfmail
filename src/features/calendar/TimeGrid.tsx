import * as React from "react";
import { Repeat, Users, Lock } from "lucide-react";
import type { Occurrence } from "../../lib/pimTypes";
import { newEvent, setPim } from "../../lib/pim";
import { cn } from "../../lib/utils";
import { reschedule } from "./actions";
import { DAY, addDays, allDayToLocal, fmtTime, inkOn, localDateToAllDay, sameDay, startOfDay } from "./time";

const HOUR = 48; // px per hour
const SNAP = 15; // minutes
const MIN_PX = HOUR / 60;

type Drag =
  | { kind: "create"; day: number; fromMin: number; toMin: number }
  | { kind: "move"; occ: Occurrence; dayShift: number; minShift: number; startY: number; startX: number; moved: boolean }
  | { kind: "resize"; occ: Occurrence; minShift: number; startY: number; moved: boolean };

const snap = (m: number) => Math.round(m / SNAP) * SNAP;

/** Lay overlapping events side by side: greedy columns per overlapping cluster. */
function layout(items: Occurrence[]): { o: Occurrence; col: number; cols: number }[] {
  const sorted = [...items].sort((a, b) => a.start - b.start || b.end - a.end);
  const out: { o: Occurrence; col: number; cols: number }[] = [];
  let cluster: { o: Occurrence; col: number }[] = [];
  let clusterEnd = -Infinity;
  const flush = () => {
    const cols = Math.max(1, ...cluster.map((c) => c.col + 1));
    cluster.forEach((c) => out.push({ ...c, cols }));
    cluster = [];
  };
  for (const o of sorted) {
    if (o.start >= clusterEnd) flush();
    const used = new Set(cluster.filter((c) => c.o.end > o.start).map((c) => c.col));
    let col = 0;
    while (used.has(col)) col++;
    cluster.push({ o, col });
    clusterEnd = Math.max(clusterEnd, o.end);
  }
  flush();
  return out;
}

export function TimeGrid({ days, items }: { days: number[]; items: Occurrence[] }) {
  const scroller = React.useRef<HTMLDivElement>(null);
  const [drag, setDrag] = React.useState<Drag | null>(null);
  const dragRef = React.useRef<Drag | null>(null);
  dragRef.current = drag;
  const [now, setNow] = React.useState(Date.now());
  React.useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(t);
  }, []);
  // Open at 07:30 (or near "now" when today is visible).
  React.useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const today = days.some((d) => sameDay(d, Date.now()));
    const mins = today ? Math.max(0, (Date.now() - startOfDay(Date.now())) / 60_000 - 120) : 7.5 * 60;
    el.scrollTop = mins * MIN_PX;
  }, [days.length]); // eslint-disable-line react-hooks/exhaustive-deps

  const timed = items.filter((o) => !o.allDay && o.status !== "CANCELLED");
  const allDay = items.filter((o) => o.allDay);

  const minuteAt = (clientY: number, col: HTMLElement) => {
    const r = col.getBoundingClientRect();
    return Math.min(24 * 60, Math.max(0, (clientY - r.top) / MIN_PX));
  };
  const dayAt = (clientX: number, clientY: number): number | null => {
    const el = document.elementFromPoint(clientX, clientY)?.closest<HTMLElement>("[data-day]");
    return el ? Number(el.dataset.day) : null;
  };

  const onColumnDown = (e: React.PointerEvent<HTMLDivElement>, day: number) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("[data-occ]")) return;
    const m = snap(minuteAt(e.clientY, e.currentTarget));
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    setDrag({ kind: "create", day, fromMin: m, toMin: m });
  };

  const onMove = (e: React.PointerEvent) => {
    const d = dragRef.current;
    if (!d) return;
    if (d.kind === "create") {
      const col = document.querySelector<HTMLElement>(`[data-day="${d.day}"]`);
      if (col) setDrag({ ...d, toMin: snap(minuteAt(e.clientY, col)) });
    } else if (d.kind === "move") {
      const minShift = snap((e.clientY - d.startY) / MIN_PX);
      const hovered = dayAt(e.clientX, e.clientY);
      const dayShift = hovered === null ? d.dayShift : Math.round((hovered - startOfDay(d.occ.start)) / DAY);
      const moved = d.moved || Math.abs(e.clientY - d.startY) > 4 || Math.abs(e.clientX - d.startX) > 4;
      setDrag({ ...d, minShift, dayShift, moved });
    } else {
      const minShift = snap((e.clientY - d.startY) / MIN_PX);
      setDrag({ ...d, minShift, moved: d.moved || Math.abs(e.clientY - d.startY) > 4 });
    }
  };

  const onUp = async () => {
    const d = dragRef.current;
    setDrag(null);
    if (!d) return;
    if (d.kind === "create") {
      const a = Math.min(d.fromMin, d.toMin);
      let b = Math.max(d.fromMin, d.toMin);
      if (b - a < SNAP) b = a + 60; // a click makes a one-hour event
      newEvent({ start: d.day + a * 60_000, end: d.day + b * 60_000, allDay: false });
    } else if (d.kind === "move") {
      if (!d.moved) {
        setPim({ editor: { kind: "edit", eventId: d.occ.eventId, occurrenceStart: d.occ.occurrenceStart, recurring: d.occ.recurring } });
        return;
      }
      const delta = d.minShift * 60_000;
      const start = addDays(d.occ.start, d.dayShift) + delta;
      await reschedule(d.occ, start, start + (d.occ.end - d.occ.start));
    } else if (d.moved) {
      const end = Math.max(d.occ.start + SNAP * 60_000, d.occ.end + d.minShift * 60_000);
      await reschedule(d.occ, d.occ.start, end);
    }
  };

  const ghostFor = (o: Occurrence): { start: number; end: number } | null => {
    if (!drag || drag.kind === "create" || drag.occ.eventId !== o.eventId || drag.occ.occurrenceStart !== o.occurrenceStart) return null;
    if (drag.kind === "move") {
      const s = addDays(o.start, drag.dayShift) + drag.minShift * 60_000;
      return { start: s, end: s + (o.end - o.start) };
    }
    return { start: o.start, end: Math.max(o.start + SNAP * 60_000, o.end + drag.minShift * 60_000) };
  };

  const cols = `56px repeat(${days.length}, minmax(0, 1fr))`;

  return (
    <div className="flex h-full min-h-0 flex-col" onPointerMove={onMove} onPointerUp={onUp} onPointerCancel={() => setDrag(null)}>
      {/* Day headers + all-day lane */}
      <div className="grid border-b border-border bg-surface" style={{ gridTemplateColumns: cols }}>
        <div />
        {days.map((d) => {
          const today = sameDay(d, now);
          return (
            <button
              key={d}
              type="button"
              onClick={() => setPim({ view: "day", anchor: d })}
              className="flex flex-col items-center gap-0.5 border-l border-border py-2 hover:bg-accent/60"
              aria-label={`Show ${new Date(d).toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" })}`}
            >
              <span className={cn("text-[11px] font-semibold uppercase tracking-wide", today ? "text-[var(--iw-red-text)]" : "text-muted-foreground")}>
                {new Date(d).toLocaleDateString(undefined, { weekday: "short" })}
              </span>
              <span className={cn("flex size-8 items-center justify-center rounded-full text-[17px] tabular-nums", today ? "bg-primary font-semibold text-white" : "font-medium")}>
                {new Date(d).getDate()}
              </span>
            </button>
          );
        })}
        <div className="flex items-start justify-end pr-2 pt-1 text-[10.5px] text-muted-foreground">all-day</div>
        {days.map((d) => {
          const list = allDay.filter((o) => allDayToLocal(o.start) <= d && allDayToLocal(o.end) > d);
          return (
            <div
              key={d}
              className="min-h-7 space-y-0.5 border-l border-border p-0.5"
              onDragOver={(e) => e.preventDefault()}
              onDrop={async (e) => {
                const raw = e.dataTransfer.getData("application/x-wolfmail-occ");
                if (!raw) return;
                const o = JSON.parse(raw) as Occurrence;
                const start = localDateToAllDay(d);
                await reschedule(o, start, start + (o.end - o.start), true);
              }}
              onDoubleClick={() => newEvent({ start: localDateToAllDay(d), end: localDateToAllDay(d) + DAY, allDay: true })}
            >
              {list.map((o) => (
                <button
                  key={o.eventId + o.occurrenceStart}
                  type="button"
                  draggable={!o.readOnly}
                  onDragStart={(e) => e.dataTransfer.setData("application/x-wolfmail-occ", JSON.stringify(o))}
                  onClick={() => setPim({ editor: { kind: "edit", eventId: o.eventId, occurrenceStart: o.occurrenceStart, recurring: o.recurring } })}
                  className={cn("block w-full truncate rounded px-1.5 py-0.5 text-left text-[11.5px] font-medium", o.status === "CANCELLED" && "line-through opacity-60")}
                  style={{ background: o.color, color: inkOn(o.color) }}
                  title={o.title}
                >
                  {o.title}
                </button>
              ))}
            </div>
          );
        })}
      </div>

      {/* Hours */}
      <div ref={scroller} className="relative min-h-0 flex-1 overflow-y-auto select-none">
        <div className="grid" style={{ gridTemplateColumns: cols, height: 24 * HOUR }}>
          <div className="relative">
            {Array.from({ length: 24 }).map((_, h) => (
              <div key={h} className="absolute right-2 -translate-y-1/2 text-[10.5px] tabular-nums text-muted-foreground" style={{ top: h * HOUR }}>
                {h === 0 ? "" : new Date(2020, 0, 1, h).toLocaleTimeString(undefined, { hour: "numeric" })}
              </div>
            ))}
          </div>
          {days.map((day) => {
            const dayEnd = day + DAY;
            const mine = timed.filter((o) => o.start < dayEnd && o.end > day);
            const placed = layout(mine);
            const isToday = sameDay(day, now);
            return (
              <div
                key={day}
                data-day={day}
                onPointerDown={(e) => onColumnDown(e, day)}
                className={cn("relative border-l border-border", isToday && "bg-[var(--iw-red-wash)]")}
                style={{
                  backgroundImage: "linear-gradient(to bottom, var(--border) 1px, transparent 1px)",
                  backgroundSize: `100% ${HOUR}px`,
                }}
                aria-label={`${new Date(day).toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" })}: drag to create an event`}
              >
                {isToday && (
                  <div aria-hidden className="pointer-events-none absolute inset-x-0 z-20 h-0.5 bg-[var(--iw-red)]" style={{ top: ((now - day) / 60_000) * MIN_PX }}>
                    <span className="absolute -left-1 -top-1 size-2.5 rounded-full bg-[var(--iw-red)]" />
                  </div>
                )}
                {drag?.kind === "create" && drag.day === day && (
                  <div className="pointer-events-none absolute inset-x-1 z-10 rounded-md bg-primary/25 ring-2 ring-primary"
                    style={{ top: Math.min(drag.fromMin, drag.toMin) * MIN_PX, height: Math.max(SNAP, Math.abs(drag.toMin - drag.fromMin)) * MIN_PX }}>
                    <span className="px-1.5 text-[11px] font-semibold">
                      {fmtTime(day + Math.min(drag.fromMin, drag.toMin) * 60_000)} – {fmtTime(day + Math.max(drag.fromMin + SNAP, drag.toMin) * 60_000)}
                    </span>
                  </div>
                )}
                {placed.map(({ o, col, cols: n }) => {
                  const g = ghostFor(o);
                  const s = Math.max(g?.start ?? o.start, day);
                  const en = Math.min(g?.end ?? o.end, dayEnd);
                  const top = ((s - day) / 60_000) * MIN_PX;
                  const height = Math.max(18, ((en - s) / 60_000) * MIN_PX - 2);
                  const declined = o.myPartstat === "DECLINED";
                  const pending = o.myPartstat === "NEEDS-ACTION";
                  return (
                    <div
                      key={o.eventId + o.occurrenceStart}
                      data-occ=""
                      className={cn("absolute z-10 overflow-hidden rounded-md text-left shadow-theme-xs ring-1 ring-black/10", g && "z-30 opacity-90 shadow-theme-lg", declined && "opacity-50")}
                      style={{
                        top,
                        height,
                        left: `calc(${(col / n) * 100}% + 2px)`,
                        width: `calc(${100 / n}% - 4px)`,
                        background: pending ? "var(--surface)" : o.color,
                        color: pending ? "var(--foreground)" : inkOn(o.color),
                        border: pending ? `2px dashed ${o.color}` : undefined,
                      }}
                    >
                      <button
                        type="button"
                        className="block h-full w-full cursor-default px-1.5 py-0.5 text-left focus-visible:outline-2"
                        aria-label={`${o.title}, ${fmtTime(o.start)} to ${fmtTime(o.end)}${o.recurring ? ", repeats" : ""}${pending ? ", awaiting your reply" : ""}`}
                        onPointerDown={(e) => {
                          if (e.button !== 0 || o.readOnly) return;
                          e.stopPropagation();
                          (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
                          setDrag({ kind: "move", occ: o, dayShift: 0, minShift: 0, startY: e.clientY, startX: e.clientX, moved: false });
                        }}
                        onClick={(e) => {
                          // Pointer path opens on pointer-up; keyboard (Enter/Space) arrives here with detail 0.
                          if (e.detail === 0 || o.readOnly) setPim({ editor: { kind: "edit", eventId: o.eventId, occurrenceStart: o.occurrenceStart, recurring: o.recurring } });
                        }}
                      >
                        <div className={cn("flex items-center gap-1 text-[11.5px] font-semibold leading-tight", (o.status === "CANCELLED" || declined) && "line-through")}>
                          <span className="truncate">{o.title}</span>
                          {o.recurring && <Repeat className="size-3 shrink-0 opacity-80" aria-hidden />}
                          {o.hasAttendees && <Users className="size-3 shrink-0 opacity-80" aria-hidden />}
                          {o.readOnly && <Lock className="size-3 shrink-0 opacity-80" aria-hidden />}
                        </div>
                        {height > 30 && (
                          <div className="truncate text-[10.5px] opacity-90">
                            {fmtTime(g?.start ?? o.start)} – {fmtTime(g?.end ?? o.end)}{o.location ? ` · ${o.location}` : ""}
                          </div>
                        )}
                      </button>
                      {!o.readOnly && (
                        <div
                          aria-hidden
                          className="absolute inset-x-0 bottom-0 h-1.5 cursor-ns-resize"
                          onPointerDown={(e) => {
                            if (e.button !== 0) return;
                            e.stopPropagation();
                            (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
                            setDrag({ kind: "resize", occ: o, minShift: 0, startY: e.clientY, moved: false });
                          }}
                        />
                      )}
                    </div>
                  );
                })}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
