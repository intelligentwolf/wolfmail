import { CalendarX2, MapPin, Repeat, Users } from "lucide-react";
import type { Occurrence } from "../../lib/pimTypes";
import { newEvent, setPim } from "../../lib/pim";
import { cn } from "../../lib/utils";
import { EmptyState } from "../../components/ui/misc";
import { Button } from "../../components/ui/button";
import { DAY, allDayToLocal, fmtLongDay, fmtTime, sameDay, startOfDay } from "./time";

/** The next 30 days as a list, grouped by day. */
export function Agenda({ from, items }: { from: number; items: Occurrence[] }) {
  const groups = new Map<number, Occurrence[]>();
  for (const o of items) {
    const first = o.allDay ? allDayToLocal(o.start) : startOfDay(o.start);
    const day = Math.max(first, from);
    groups.set(day, [...(groups.get(day) ?? []), o]);
  }
  const days = [...groups.keys()].sort((a, b) => a - b);
  if (!days.length) {
    return (
      <EmptyState icon={<CalendarX2 className="size-6" />} title="Nothing planned" body="No events in the next 30 days in the calendars you're showing."
        action={<Button onClick={() => newEvent()}>New event</Button>} />
    );
  }
  return (
    <div className="h-full overflow-y-auto px-6 py-4">
      <ol className="mx-auto max-w-3xl space-y-5">
        {days.map((d) => (
          <li key={d}>
            <h3 className={cn("mb-2 text-[12px] font-semibold uppercase tracking-wide", sameDay(d, Date.now()) ? "text-[var(--iw-red-text)]" : "text-muted-foreground")}>
              {sameDay(d, Date.now()) ? "Today · " : sameDay(d, Date.now() + DAY) ? "Tomorrow · " : ""}{fmtLongDay(d)}
            </h3>
            <ul className="overflow-hidden rounded-card bg-surface ring-1 ring-border">
              {(groups.get(d) ?? []).map((o) => (
                <li key={o.eventId + o.occurrenceStart} className="border-b border-border last:border-b-0">
                  <button
                    type="button"
                    onClick={() => setPim({ editor: { kind: "edit", eventId: o.eventId, occurrenceStart: o.occurrenceStart, recurring: o.recurring } })}
                    className="flex w-full items-center gap-4 px-4 py-3 text-left hover:bg-accent/60"
                  >
                    <span aria-hidden className="h-9 w-1 shrink-0 rounded-full" style={{ background: o.color }} />
                    <span className="w-28 shrink-0 text-[13px] tabular-nums text-muted-foreground">
                      {o.allDay ? "All day" : `${fmtTime(o.start)} – ${fmtTime(o.end)}`}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className={cn("flex items-center gap-1.5 text-[14px] font-medium", (o.status === "CANCELLED" || o.myPartstat === "DECLINED") && "line-through opacity-60")}>
                        <span className="truncate">{o.title}</span>
                        {o.recurring && <Repeat className="size-3.5 text-muted-foreground" aria-label="Repeats" />}
                        {o.hasAttendees && <Users className="size-3.5 text-muted-foreground" aria-label="Has guests" />}
                        {o.myPartstat === "NEEDS-ACTION" && <span className="rounded-full bg-[var(--iw-red-wash)] px-2 text-[11px] font-semibold text-[var(--iw-red-text)]">Reply needed</span>}
                      </span>
                      {o.location && <span className="mt-0.5 flex items-center gap-1 truncate text-[12px] text-muted-foreground"><MapPin className="size-3" />{o.location}</span>}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </li>
        ))}
      </ol>
    </div>
  );
}
