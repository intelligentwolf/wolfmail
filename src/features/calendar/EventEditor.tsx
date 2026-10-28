import * as React from "react";
import { Lock, Trash2, MapPin, AlignLeft, Users, Bell, Repeat, Globe, CheckCircle2, XCircle, HelpCircle, Clock3 } from "lucide-react";
import { pimApi } from "../../lib/pimApi";
import { notify } from "../../lib/notify";
import { setMode, setPim, usePim } from "../../lib/pim";
import type { Attendee, CalEvent } from "../../lib/pimTypes";
import { cn } from "../../lib/utils";
import { Dialog } from "../../components/ui/dialog";
import { Button } from "../../components/ui/button";
import { Field, Input, NativeSelect, Switch, Textarea } from "../../components/ui/form";
import { Spinner } from "../../components/ui/misc";
import { AddressField } from "../compose/AddressField";
import { askScope, deleteOccurrence } from "./actions";
import { DAY, dateStringToUtc, localZone, msToZoned, utcDateString, zoneList, zonedToMs } from "./time";

const WD = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];
const WD_NAME = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const ORD = ["first", "second", "third", "fourth", "last"];
const REMINDERS: [number, string][] = [[0, "At start"], [5, "5 min"], [10, "10 min"], [15, "15 min"], [30, "30 min"], [60, "1 hour"], [1440, "1 day"]];

type Repeat = "none" | "daily" | "weekdays" | "weekly" | "monthlyDay" | "monthlyNth" | "yearly" | "custom";
type Ends = "never" | "until" | "count";

/** Weekday index (0 = Monday), ordinal ("2", "-1") and day of month of a date string "YYYY-MM-DD". */
function dateFacts(date: string) {
  const d = new Date(`${date}T12:00:00Z`);
  const wd = (d.getUTCDay() + 6) % 7;
  const dom = d.getUTCDate();
  const dim = new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 0)).getUTCDate();
  const nth = dom + 7 > dim ? -1 : Math.floor((dom - 1) / 7) + 1;
  return { wd, dom, nth };
}

function buildRule(rep: Repeat, date: string, custom: string, ends: Ends, until: string, count: number): string | undefined {
  if (rep === "none") return undefined;
  if (rep === "custom") return custom.trim().replace(/^RRULE:/i, "") || undefined;
  const f = dateFacts(date);
  const base = {
    daily: "FREQ=DAILY",
    weekdays: "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
    weekly: `FREQ=WEEKLY;BYDAY=${WD[f.wd]}`,
    monthlyDay: `FREQ=MONTHLY;BYMONTHDAY=${f.dom}`,
    monthlyNth: `FREQ=MONTHLY;BYDAY=${f.nth}${WD[f.wd]}`,
    yearly: "FREQ=YEARLY",
  }[rep];
  if (ends === "until" && until) return `${base};UNTIL=${until.replace(/-/g, "")}T235959Z`;
  if (ends === "count" && count > 0) return `${base};COUNT=${count}`;
  return base;
}

/** Recognise a stored rule as one of the presets (so editing it shows the same choice), else "custom". */
function parseRule(rule: string | undefined, date: string): { rep: Repeat; ends: Ends; until: string; count: number; custom: string } {
  const empty = { rep: "none" as Repeat, ends: "never" as Ends, until: "", count: 10, custom: "" };
  if (!rule) return empty;
  const parts = Object.fromEntries(rule.split(";").map((p) => p.split("=") as [string, string]));
  const ends: Ends = parts.UNTIL ? "until" : parts.COUNT ? "count" : "never";
  const until = parts.UNTIL ? `${parts.UNTIL.slice(0, 4)}-${parts.UNTIL.slice(4, 6)}-${parts.UNTIL.slice(6, 8)}` : "";
  const count = parts.COUNT ? Number(parts.COUNT) : 10;
  const core = rule.split(";").filter((p) => !p.startsWith("UNTIL=") && !p.startsWith("COUNT=")).join(";");
  for (const rep of ["daily", "weekdays", "weekly", "monthlyDay", "monthlyNth", "yearly"] as Repeat[]) {
    if (buildRule(rep, date, "", "never", "", 0) === core) return { rep, ends, until, count, custom: "" };
  }
  return { rep: "custom", ends: "never", until: "", count: 10, custom: rule };
}

const partIcon = (p: string) =>
  p === "ACCEPTED" ? <CheckCircle2 className="size-4 text-[var(--iw-go-text)]" aria-label="Going" />
    : p === "DECLINED" ? <XCircle className="size-4 text-[var(--iw-red-text)]" aria-label="Not going" />
    : p === "TENTATIVE" ? <HelpCircle className="size-4 text-[var(--iw-warn-text)]" aria-label="Maybe" />
    : <Clock3 className="size-4 text-muted-foreground" aria-label="No reply yet" />;

export function EventEditor() {
  const editor = usePim((s) => s.editor);
  const calendars = usePim((s) => s.calendars);
  const accounts = usePim((s) => s.accounts);
  const open = !!editor;
  const [ev, setEv] = React.useState<CalEvent | null>(null);
  const [loadError, setLoadError] = React.useState<string>();
  const [busy, setBusy] = React.useState(false);
  const [errors, setErrors] = React.useState<Record<string, string>>({});
  // form fields
  const [title, setTitle] = React.useState("");
  const [calendarId, setCalendarId] = React.useState("");
  const [allDay, setAllDay] = React.useState(false);
  const [tz, setTz] = React.useState(localZone());
  const [sDate, setSDate] = React.useState("");
  const [sTime, setSTime] = React.useState("");
  const [eDate, setEDate] = React.useState("");
  const [eTime, setETime] = React.useState("");
  const [location, setLocation] = React.useState("");
  const [description, setDescription] = React.useState("");
  const [guests, setGuests] = React.useState<{ email: string; name?: string }[]>([]);
  const [reminders, setReminders] = React.useState<number[]>([15]);
  const [rep, setRep] = React.useState<Repeat>("none");
  const [ends, setEnds] = React.useState<Ends>("never");
  const [until, setUntil] = React.useState("");
  const [count, setCount] = React.useState(10);
  const [custom, setCustom] = React.useState("");
  const summaryRef = React.useRef<HTMLDivElement>(null);

  const writable = calendars.filter((c) => !c.readOnly);
  const isNew = editor?.kind === "new";

  // Load the event (or the new-event defaults) into the form.
  React.useEffect(() => {
    if (!editor) return;
    setErrors({});
    setLoadError(undefined);
    const fill = (e: CalEvent, occStart: number) => {
      const zone = e.timezone || localZone();
      const dur = e.end - e.start;
      // Show the clicked occurrence's times, not the series' first one.
      const start = editor.kind === "edit" ? occStart : e.start;
      setEv(e);
      setTitle(e.title);
      setCalendarId(e.calendarId);
      setAllDay(e.allDay);
      setTz(zone);
      if (e.allDay) {
        setSDate(utcDateString(start));
        setEDate(utcDateString(start + dur - DAY));
        setSTime("09:00");
        setETime("10:00");
      } else {
        const s = msToZoned(start, zone);
        const en = msToZoned(start + dur, zone);
        setSDate(s.date); setSTime(s.time); setEDate(en.date); setETime(en.time);
      }
      setLocation(e.location);
      setDescription(e.description);
      setGuests(e.attendees.map((a) => ({ email: a.email, name: a.name })));
      setReminders(e.reminders);
      const r = parseRule(e.recurrenceId ? undefined : e.rrule, e.allDay ? utcDateString(start) : msToZoned(start, zone).date);
      setRep(r.rep); setEnds(r.ends); setUntil(r.until); setCount(r.count); setCustom(r.custom);
    };
    if (editor.kind === "new") {
      const d = editor.draft;
      const now = Date.now();
      const start = d.start ?? Math.ceil(now / 1_800_000) * 1_800_000;
      const def = writable.find((c) => c.isDefault) ?? writable[0];
      fill({
        id: "", calendarId: d.calendarId ?? def?.id ?? "", uid: "", title: d.title ?? "", location: d.location ?? "", description: d.description ?? "",
        start, end: d.end ?? start + (d.allDay ? DAY : 3_600_000), allDay: d.allDay ?? false, timezone: d.allDay ? "" : localZone(),
        exdates: [], status: "CONFIRMED", attendees: d.attendees ?? [], reminders: d.allDay ? [] : [15], sequence: 0, readOnly: false,
      }, start);
    } else {
      setEv(null);
      pimApi.eventGet(editor.eventId).then((e) => fill(e, editor.occurrenceStart)).catch((e) => setLoadError((e as { message?: string }).message ?? String(e)));
    }
  }, [editor]); // eslint-disable-line react-hooks/exhaustive-deps

  const close = () => setPim({ editor: null });
  const readOnly = !!ev?.readOnly;
  // Microsoft occurrences carry no rule (Graph expands series server-side), so the clicked occurrence's flag counts.
  const recurringExisting = !isNew && !!ev && (!!ev.rrule || ev.recurrenceId !== undefined || (editor?.kind === "edit" && !!editor.recurring));
  const occStart = editor?.kind === "edit" ? editor.occurrenceStart : undefined;

  const submit = async () => {
    if (!ev) return;
    const errs: Record<string, string> = {};
    if (!calendarId) errs.calendarId = "Choose a calendar";
    if (!sDate) errs.start = "Enter a start date";
    if (!eDate) errs.end = "Enter an end date";
    let start = 0;
    let end = 0;
    if (sDate && eDate) {
      if (allDay) {
        start = dateStringToUtc(sDate);
        end = dateStringToUtc(eDate) + DAY;
      } else {
        start = zonedToMs(sDate, sTime, tz);
        end = zonedToMs(eDate, eTime, tz);
      }
      if (end < start || (allDay && end <= start)) errs.end = "The event must end after it starts";
    }
    if (rep === "custom" && !custom.trim()) errs.rrule = "Enter a repeat rule, or choose another option";
    if (rep !== "none" && ends === "until" && !until) errs.until = "Choose the last date";
    if (rep !== "none" && ends === "count" && (!count || count < 1)) errs.count = "Enter how many times";
    setErrors(errs);
    if (Object.keys(errs).length) {
      requestAnimationFrame(() => summaryRef.current?.focus());
      return;
    }
    const rrule = buildRule(rep, sDate, custom, ends, until, count);
    const keep = new Map(ev.attendees.map((a) => [a.email.toLowerCase(), a]));
    const attendees: Attendee[] = guests.map((g) => keep.get(g.email.toLowerCase()) ?? { email: g.email.toLowerCase(), name: g.name, partstat: "NEEDS-ACTION", optional: false });
    const next: CalEvent = {
      ...ev,
      calendarId, title: title.trim(), location: location.trim(), description, start, end, allDay,
      timezone: allDay ? "" : tz, rrule: ev.recurrenceId !== undefined ? undefined : rrule, attendees, reminders,
    };
    let scope: "this" | "all" | undefined;
    if (recurringExisting) {
      const s = await askScope("change", title || ev.title);
      if (!s) return;
      scope = s;
      if (s === "this") next.rrule = undefined;
    }
    setBusy(true);
    try {
      await pimApi.eventSave(next, scope, occStart);
      notify.ok(isNew ? `“${next.title || "(no title)"}” added` : "Event saved");
      close();
    } catch (e) {
      const err = e as { field?: string; message?: string };
      if (err.field) {
        setErrors({ [err.field === "event" ? "start" : err.field]: err.message ?? "Check this" });
        requestAnimationFrame(() => summaryRef.current?.focus());
      } else notify.error(e, "Couldn't save the event");
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!ev || editor?.kind !== "edit") return;
    const ok = await deleteOccurrence({ eventId: ev.id, title: ev.title, recurring: recurringExisting, occurrenceStart: editor.occurrenceStart });
    if (ok) close();
  };

  const accountName = (id: string) => accounts.find((a) => a.id === id)?.name ?? "";
  const sF = dateFacts(sDate || utcDateString(Date.now()));
  const errorList = Object.entries(errors);

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => !o && close()}
      title={isNew ? "New event" : readOnly ? "Event" : "Edit event"}
      className="w-[min(680px,calc(100vw-32px))]"
      footer={
        !readOnly && ev ? (
          <>
            {!isNew && <Button variant="ghost" className="mr-auto text-[var(--iw-red-text)]" onClick={remove}><Trash2 /> Delete</Button>}
            <Button variant="outline" onClick={close}>Cancel</Button>
            <Button onClick={submit} disabled={busy}>{busy && <Spinner />}{isNew ? "Add event" : "Save"}</Button>
          </>
        ) : (
          <Button variant="outline" onClick={close}>Close</Button>
        )
      }
    >
      {loadError ? (
        <p role="alert" className="rounded-lg bg-[var(--iw-red-wash)] p-3 text-sm text-[var(--iw-red-text)]">Couldn't open the event: {loadError}</p>
      ) : !ev ? (
        <div className="flex justify-center py-10"><Spinner /></div>
      ) : (
        <form className="space-y-4" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
          {errorList.length > 0 && (
            <div ref={summaryRef} tabIndex={-1} role="alert" className="rounded-lg bg-[var(--iw-red-wash)] p-3 text-[13px] ring-1 ring-[var(--iw-red)]/40">
              <p className="font-semibold">Check {errorList.length === 1 ? "this" : "these"} before saving:</p>
              <ul className="mt-1 list-disc pl-5">
                {errorList.map(([k, v]) => <li key={k}><a className="underline" href={`#ev-${k}`}>{v}</a></li>)}
              </ul>
            </div>
          )}
          {readOnly && (
            <p className="flex items-center gap-2 rounded-lg bg-surface-raised px-3 py-2 text-[13px] ring-1 ring-border">
              <Lock className="size-4" /> This calendar is read-only, so the event can't be changed here.
            </p>
          )}
          <fieldset disabled={readOnly} className="space-y-4">
            <Input id="ev-title" autoFocus={isNew} placeholder="Add a title" value={title} onChange={(e) => setTitle(e.target.value)}
              className="h-11 font-title text-[18px]" aria-label="Title" />

            <div className="grid gap-3 sm:grid-cols-[1fr_auto]">
              <Field id="ev-calendarId" label="Calendar" error={errors.calendarId}>
                <NativeSelect value={calendarId} onChange={(e) => setCalendarId(e.target.value)}>
                  {[...new Set(writable.map((c) => c.accountId))].map((aid) => (
                    <optgroup key={aid} label={accountName(aid)}>
                      {writable.filter((c) => c.accountId === aid).map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}
                    </optgroup>
                  ))}
                  {readOnly && <option value={calendarId}>{calendars.find((c) => c.id === calendarId)?.name}</option>}
                </NativeSelect>
              </Field>
              <div className="self-end"><Switch id="ev-allday" label="All day" checked={allDay} onCheckedChange={setAllDay} /></div>
            </div>

            <div className="grid gap-3 sm:grid-cols-2">
              <Field id="ev-start" label="Starts" error={errors.start}>
                <div className="flex gap-2">
                  <Input type="date" value={sDate} onChange={(e) => setSDate(e.target.value)} aria-label="Start date" />
                  {!allDay && <Input type="time" value={sTime} onChange={(e) => setSTime(e.target.value)} aria-label="Start time" className="w-32" />}
                </div>
              </Field>
              <Field id="ev-end" label="Ends" error={errors.end}>
                <div className="flex gap-2">
                  <Input type="date" value={eDate} onChange={(e) => setEDate(e.target.value)} aria-label="End date" />
                  {!allDay && <Input type="time" value={eTime} onChange={(e) => setETime(e.target.value)} aria-label="End time" className="w-32" />}
                </div>
              </Field>
            </div>
            {!allDay && (
              <Field id="ev-timezone" label="Time zone" error={errors.timezone} hint={<span className="inline-flex items-center gap-1"><Globe className="size-3" /> Times above are in this zone.</span>}>
                <NativeSelect value={tz} onChange={(e) => setTz(e.target.value)}>
                  {[...new Set([tz, ...zoneList()])].map((z) => <option key={z} value={z}>{z.replace(/_/g, " ")}</option>)}
                </NativeSelect>
              </Field>
            )}

            {ev.recurrenceId === undefined && (
              <div className="space-y-2 rounded-lg bg-surface-raised p-3 ring-1 ring-border">
                <Field id="ev-rrule" label="Repeat" error={errors.rrule}>
                  <NativeSelect value={rep} onChange={(e) => setRep(e.target.value as Repeat)}>
                    <option value="none">Does not repeat</option>
                    <option value="daily">Every day</option>
                    <option value="weekdays">Every weekday (Mon–Fri)</option>
                    <option value="weekly">Every week on {WD_NAME[sF.wd]}</option>
                    <option value="monthlyDay">Every month on day {sF.dom}</option>
                    <option value="monthlyNth">Every month on the {ORD[sF.nth === -1 ? 4 : sF.nth - 1]} {WD_NAME[sF.wd]}</option>
                    <option value="yearly">Every year</option>
                    <option value="custom">Custom rule…</option>
                  </NativeSelect>
                </Field>
                {rep === "custom" && (
                  <Input value={custom} onChange={(e) => setCustom(e.target.value)} placeholder="FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE" aria-label="Repeat rule (RFC 5545 RRULE)" className="font-mono text-[12px]" />
                )}
                {rep !== "none" && rep !== "custom" && (
                  <div className="flex flex-wrap items-center gap-2 text-[13px]">
                    <Repeat className="size-4 text-muted-foreground" />
                    <label htmlFor="ev-ends" className="text-muted-foreground">Ends</label>
                    <NativeSelect id="ev-ends" value={ends} onChange={(e) => setEnds(e.target.value as Ends)} className="w-36">
                      <option value="never">Never</option>
                      <option value="until">On date</option>
                      <option value="count">After</option>
                    </NativeSelect>
                    {ends === "until" && <Input id="ev-until" type="date" value={until} onChange={(e) => setUntil(e.target.value)} aria-label="Last date" aria-invalid={!!errors.until} className="w-44" />}
                    {ends === "count" && <><Input id="ev-count" type="number" min={1} max={999} value={count} onChange={(e) => setCount(Number(e.target.value))} aria-label="Number of times" aria-invalid={!!errors.count} className="w-24" /><span>times</span></>}
                  </div>
                )}
              </div>
            )}

            <div className="flex items-center gap-2"><MapPin className="size-4 shrink-0 text-muted-foreground" />
              <Input value={location} onChange={(e) => setLocation(e.target.value)} placeholder="Add a location or meeting link" aria-label="Location" />
            </div>

            <div className="overflow-visible rounded-lg ring-1 ring-border">
              <div className="flex items-center gap-2 px-3 pt-2 text-[13px] font-medium"><Users className="size-4 text-muted-foreground" /> Guests</div>
              <AddressField id="ev-guests" label="Invite" value={guests} onChange={setGuests} />
              {ev.attendees.length > 0 && (
                <ul className="space-y-1 px-4 py-2">
                  {ev.organizer && <li className="text-xs text-muted-foreground">Organised by {ev.organizer.name ?? ev.organizer.email}</li>}
                  {ev.attendees.map((a) => (
                    <li key={a.email} className="flex items-center gap-2 text-[13px]">{partIcon(a.partstat)}<span className="truncate">{a.name ?? a.email}</span>{a.optional && <span className="text-xs text-muted-foreground">optional</span>}</li>
                  ))}
                </ul>
              )}
              {guests.length > 0 && <p className="px-4 pb-2 text-[11.5px] text-muted-foreground">Guests get an invitation by email when you save.</p>}
            </div>

            <div>
              <div className="mb-1.5 flex items-center gap-2 text-[13px] font-medium"><Bell className="size-4 text-muted-foreground" /> Reminders</div>
              <div className="flex flex-wrap gap-1.5" role="group" aria-label="Reminders">
                {REMINDERS.map(([m, label]) => {
                  const on = reminders.includes(m);
                  return (
                    <button key={m} type="button" aria-pressed={on}
                      onClick={() => setReminders(on ? reminders.filter((x) => x !== m) : [...reminders, m].sort((a, b) => a - b))}
                      className={cn("h-7 rounded-full px-2.5 text-xs font-medium ring-1 ring-inset", on ? "bg-select-strong ring-[var(--iw-red)]/40" : "text-muted-foreground ring-border hover:bg-accent")}>
                      {label}
                    </button>
                  );
                })}
              </div>
            </div>

            <div className="flex gap-2"><AlignLeft className="mt-2 size-4 shrink-0 text-muted-foreground" />
              <Textarea rows={4} value={description} onChange={(e) => setDescription(e.target.value)} placeholder="Add a description" aria-label="Description" />
            </div>
          </fieldset>
        </form>
      )}
    </Dialog>
  );
}

/** Open the editor for a new event with someone as a guest (used from Contacts). */
export function newEventWith(email: string, name?: string) {
  setMode("calendar");
  setPim({ editor: { kind: "new", draft: { attendees: [{ email, name, partstat: "NEEDS-ACTION", optional: false }] } } });
}
