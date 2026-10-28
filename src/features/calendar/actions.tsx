import * as React from "react";
import * as AD from "@radix-ui/react-alert-dialog";
import { pimApi } from "../../lib/pimApi";
import { notify } from "../../lib/notify";
import type { CalEvent, EditScope, Occurrence } from "../../lib/pimTypes";
import { Button } from "../../components/ui/button";
import { DAY } from "./time";

// "This event or the whole series?" — asked whenever a repeating event is changed or deleted (the convention of
// Google Calendar, Outlook and Thunderbird). Promise-based so drag handlers and the editor share one dialog.
type Ask = { verb: "change" | "delete"; title: string; resolve: (s: EditScope | null) => void };
let setAsk: ((a: Ask | null) => void) | null = null;

export function askScope(verb: "change" | "delete", title: string): Promise<EditScope | null> {
  return new Promise((resolve) => {
    if (!setAsk) return resolve("all");
    setAsk({ verb, title, resolve });
  });
}

export function ScopePrompt() {
  const [ask, set] = React.useState<Ask | null>(null);
  React.useEffect(() => {
    setAsk = set;
    return () => { setAsk = null; };
  }, []);
  const done = (s: EditScope | null) => {
    ask?.resolve(s);
    set(null);
  };
  return (
    <AD.Root open={!!ask} onOpenChange={(o) => { if (!o) done(null); }}>
      <AD.Portal>
        <AD.Overlay className="fixed inset-0 z-[60] bg-black/40" />
        <AD.Content aria-modal="true" className="fixed left-1/2 top-1/2 z-[60] w-[min(440px,calc(100vw-32px))] -translate-x-1/2 -translate-y-1/2 rounded-card bg-popover p-6 shadow-[var(--shadow-pop)] ring-1 ring-border">
          <AD.Title className="font-title text-lg">{ask?.verb === "delete" ? "Delete a repeating event" : "Change a repeating event"}</AD.Title>
          <AD.Description className="mt-2 text-sm text-muted-foreground">
            “{ask?.title}” repeats. {ask?.verb === "delete" ? "Delete" : "Change"} just this one, or every event in the series?
          </AD.Description>
          <div className="mt-6 flex flex-wrap items-center gap-2">
            <AD.Cancel asChild><Button variant="outline">Cancel</Button></AD.Cancel>
            <div className="flex-1" />
            <Button variant="secondary" onClick={() => done("this")}>This event</Button>
            <Button variant={ask?.verb === "delete" ? "destructive" : "default"} onClick={() => done("all")}>All events</Button>
          </div>
        </AD.Content>
      </AD.Portal>
    </AD.Root>
  );
}

/** Move/resize an occurrence (drag & drop). Returns false when the user cancelled or saving failed. */
export async function reschedule(o: Occurrence, start: number, end: number, allDay = o.allDay): Promise<boolean> {
  if (o.readOnly) {
    notify.error({ kind: "invalid", message: "This calendar is read-only." }, "Can't move that event");
    return false;
  }
  let scope: EditScope | undefined;
  if (o.recurring) {
    const s = await askScope("change", o.title);
    if (!s) return false;
    scope = s;
  }
  try {
    const ev = await pimApi.eventGet(o.eventId);
    const next: CalEvent = { ...ev, start, end: Math.max(end, start + (allDay ? DAY : 0)), allDay };
    await pimApi.eventSave(next, scope, o.occurrenceStart);
    return true;
  } catch (e) {
    notify.error(e, "Couldn't move the event");
    return false;
  }
}

export async function deleteOccurrence(o: { eventId: string; title: string; recurring: boolean; occurrenceStart: number }): Promise<boolean> {
  let scope: EditScope = "all";
  if (o.recurring) {
    const s = await askScope("delete", o.title);
    if (!s) return false;
    scope = s;
  }
  try {
    await pimApi.eventDelete(o.eventId, scope, o.occurrenceStart);
    notify.ok(scope === "this" ? "That one event was deleted" : `“${o.title}” deleted`);
    return true;
  } catch (e) {
    notify.error(e, "Couldn't delete the event");
    return false;
  }
}
