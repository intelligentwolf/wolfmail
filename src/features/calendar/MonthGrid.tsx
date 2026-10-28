import * as React from "react";
import type { Occurrence } from "../../lib/pimTypes";
import { newEvent, setPim } from "../../lib/pim";
import { cn } from "../../lib/utils";
import { reschedule } from "./actions";
import { DAY, addDays, allDayToLocal, fmtTime, inkOn, localDateToAllDay, sameDay, startOfDay } from "./time";

const MAX_PER_CELL = 3;

/** Six weeks starting on the Monday on/before the 1st. Drag an event to another day to move it (time kept). */
export function MonthGrid({ first, month, items }: { first: number; month: number; items: Occurrence[] }) {
  const days = Array.from({ length: 42 }, (_, i) => addDays(first, i));
  const now = Date.now();
  const onDay = (d: number) =>
    items
      .filter((o) => (o.allDay ? allDayToLocal(o.start) <= d && allDayToLocal(o.end) > d : o.start < d + DAY && o.end > d))
      .sort((a, b) => Number(b.allDay) - Number(a.allDay) || a.start - b.start);

  const drop = async (e: React.DragEvent, d: number) => {
    const raw = e.dataTransfer.getData("application/x-wolfmail-occ");
    if (!raw) return;
    e.preventDefault();
    const o = JSON.parse(raw) as Occurrence;
    if (o.allDay) {
      const start = localDateToAllDay(d);
      await reschedule(o, start, start + (o.end - o.start), true);
    } else {
      const shift = d - startOfDay(o.start);
      await reschedule(o, o.start + shift, o.end + shift);
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="grid grid-cols-7 border-b border-border bg-surface">
        {days.slice(0, 7).map((d) => (
          <div key={d} className="py-2 text-center text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
            {new Date(d).toLocaleDateString(undefined, { weekday: "short" })}
          </div>
        ))}
      </div>
      <div className="grid min-h-0 flex-1 grid-cols-7 grid-rows-6">
        {days.map((d) => {
          const list = onDay(d);
          const inMonth = new Date(d).getMonth() === new Date(month).getMonth();
          const today = sameDay(d, now);
          return (
            <div
              key={d}
              onDragOver={(e) => e.preventDefault()}
              onDrop={(e) => drop(e, d)}
              onDoubleClick={(e) => {
                if ((e.target as HTMLElement).closest("button")) return;
                const start = d + 9 * 3_600_000;
                newEvent({ start, end: start + 3_600_000, allDay: false });
              }}
              className={cn("flex min-h-0 flex-col gap-0.5 overflow-hidden border-b border-l border-border p-1", !inMonth && "bg-surface-inset/60")}
            >
              <button
                type="button"
                onClick={() => setPim({ view: "day", anchor: d })}
                aria-label={`Show ${new Date(d).toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" })}`}
                className={cn(
                  "ml-auto flex size-6 items-center justify-center rounded-full text-[12px] tabular-nums hover:bg-accent",
                  today ? "bg-primary font-semibold text-white hover:bg-primary" : inMonth ? "text-foreground" : "text-muted-foreground",
                )}
              >
                {new Date(d).getDate()}
              </button>
              {list.slice(0, MAX_PER_CELL).map((o) => (
                <button
                  key={o.eventId + o.occurrenceStart}
                  type="button"
                  draggable={!o.readOnly}
                  onDragStart={(e) => e.dataTransfer.setData("application/x-wolfmail-occ", JSON.stringify(o))}
                  onClick={() => setPim({ editor: { kind: "edit", eventId: o.eventId, occurrenceStart: o.occurrenceStart, recurring: o.recurring } })}
                  title={o.allDay ? o.title : `${fmtTime(o.start)} ${o.title}`}
                  className={cn(
                    "flex w-full items-center gap-1 truncate rounded px-1 py-px text-left text-[11.5px]",
                    (o.status === "CANCELLED" || o.myPartstat === "DECLINED") && "line-through opacity-60",
                  )}
                  style={o.allDay ? { background: o.color, color: inkOn(o.color) } : undefined}
                >
                  {!o.allDay && <span aria-hidden className="size-2 shrink-0 rounded-full" style={{ background: o.color }} />}
                  {!o.allDay && <span className="shrink-0 tabular-nums text-muted-foreground">{fmtTime(o.start)}</span>}
                  <span className="truncate font-medium">{o.title}</span>
                </button>
              ))}
              {list.length > MAX_PER_CELL && (
                <button type="button" onClick={() => setPim({ view: "day", anchor: d })} className="px-1 text-left text-[11px] font-semibold text-muted-foreground hover:text-foreground">
                  +{list.length - MAX_PER_CELL} more
                </button>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
