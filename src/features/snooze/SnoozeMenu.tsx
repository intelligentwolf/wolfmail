import * as React from "react";
import { addDays, addHours, format, nextMonday, nextSaturday, setHours, setMinutes, startOfHour } from "date-fns";
import { AlarmClock, CalendarClock, Sun, Sofa, Briefcase, Clock } from "lucide-react";
import { Ctx, Menu } from "../../components/ui/misc";
import { Dialog } from "../../components/ui/dialog";
import { Button } from "../../components/ui/button";
import { Field, Input } from "../../components/ui/form";
import * as A from "../actions";

/** The usual snooze choices (Gmail/Spark convention). "Later today" is only offered while there is a later today. */
export function snoozePresets(now = new Date()): { label: string; at: Date; icon: React.ReactNode }[] {
  const at = (d: Date, h: number) => setMinutes(setHours(d, h), 0);
  const list: { label: string; at: Date; icon: React.ReactNode }[] = [];
  const later = addHours(startOfHour(now), 3);
  if (later.getDate() === now.getDate() && later.getHours() <= 20) list.push({ label: "Later today", at: later, icon: <Clock /> });
  list.push({ label: "Tomorrow morning", at: at(addDays(now, 1), 8), icon: <Sun /> });
  const day = now.getDay(); // 0 Sun … 6 Sat
  if (day >= 1 && day <= 4) list.push({ label: "This weekend", at: at(nextSaturday(now), 9), icon: <Sofa /> });
  list.push({ label: "Next week", at: at(nextMonday(now), 8), icon: <Briefcase /> });
  return list;
}

function when(d: Date) {
  return format(d, "EEE HH:mm");
}

/** Snooze choices as dropdown items (toolbar, bulk bar) or context-menu items (row right-click). */
export function SnoozeItems({ kind, ids, onPick }: { kind: "menu" | "ctx"; ids?: string[]; onPick: () => void }) {
  const Item = kind === "menu" ? Menu.Item : Ctx.Item;
  return (
    <>
      {snoozePresets().map((p) => (
        <Item key={p.label} onSelect={() => A.snooze(p.at.getTime(), ids)}>
          {p.icon} {p.label}
          <span className="ml-auto pl-4 text-[11px] text-muted-foreground">{when(p.at)}</span>
        </Item>
      ))}
      <Item onSelect={onPick}><CalendarClock /> Pick a date and time…</Item>
    </>
  );
}

/** Custom snooze time. Validation on submit (GOV.UK): must be at least a minute ahead. */
export function SnoozeDialog({ open, onClose, ids }: { open: boolean; onClose: () => void; ids?: string[] }) {
  const [val, setVal] = React.useState("");
  const [err, setErr] = React.useState<string>();
  React.useEffect(() => {
    if (open) { setVal(format(addDays(setMinutes(setHours(new Date(), 8), 0), 1), "yyyy-MM-dd'T'HH:mm")); setErr(undefined); }
  }, [open]);
  const submit = () => {
    const t = new Date(val).getTime();
    if (!val || Number.isNaN(t)) { setErr("Enter a date and time"); return; }
    if (t < Date.now() + 60_000) { setErr("Pick a time at least a minute from now"); return; }
    onClose();
    void A.snooze(t, ids);
  };
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()} title="Snooze until…"
      description="The conversation disappears from your inboxes and comes back unread at this time. Snoozes are kept on this computer."
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button onClick={submit}><AlarmClock /> Snooze</Button></>}>
      <form onSubmit={(e) => { e.preventDefault(); submit(); }}>
        <Field id="snooze-at" label="Bring it back at" error={err}>
          <Input type="datetime-local" value={val} onChange={(e) => setVal(e.target.value)} />
        </Field>
      </form>
    </Dialog>
  );
}
