import * as React from "react";
import * as Popover from "@radix-ui/react-popover";
import {
  ChevronLeft, ChevronRight, Plus, RefreshCw, AlertTriangle, WifiOff, Lock, MoreHorizontal, Download, UserPlus, Trash2, KeyRound,
  CalendarPlus, Eye, EyeOff,
} from "lucide-react";
import { pimApi } from "../../lib/pimApi";
import { notify } from "../../lib/notify";
import { newEvent, setCalView, setPim, step, usePim } from "../../lib/pim";
import type { Calendar, PimAccount } from "../../lib/pimTypes";
import { cn } from "../../lib/utils";
import { Button, IconButton } from "../../components/ui/button";
import { ConfirmDialog, Dialog } from "../../components/ui/dialog";
import { Field, Input } from "../../components/ui/form";
import { Menu, Skeleton, Spinner } from "../../components/ui/misc";
import { TimeGrid } from "./TimeGrid";
import { MonthGrid } from "./MonthGrid";
import { Agenda } from "./Agenda";
import { useOccurrences } from "./useOccurrences";
import { PimHeader } from "../pim/ModeSwitch";
import { addDays, fmtMonth, sameDay, startOfDay, startOfMonth, startOfWeek } from "./time";

const SWATCHES = ["#c41e1e", "#2563eb", "#15803d", "#b7791f", "#7c3aed", "#0e7490", "#be185d", "#4b4c53"];

function range(view: string, anchor: number): { from: number; to: number; days: number[]; title: string } {
  if (view === "month") {
    const m = startOfMonth(anchor);
    const first = startOfWeek(m);
    return { from: first, to: addDays(first, 42), days: [], title: fmtMonth(anchor) };
  }
  if (view === "week") {
    const s = startOfWeek(anchor);
    const days = Array.from({ length: 7 }, (_, i) => addDays(s, i));
    const e = days[6];
    const sameMonth = new Date(s).getMonth() === new Date(e).getMonth();
    const title = sameMonth
      ? fmtMonth(s)
      : `${new Date(s).toLocaleDateString(undefined, { month: "short" })} – ${new Date(e).toLocaleDateString(undefined, { month: "short", year: "numeric" })}`;
    return { from: s, to: addDays(s, 7), days, title };
  }
  if (view === "agenda") {
    const s = startOfDay(anchor);
    return { from: s, to: addDays(s, 30), days: [], title: `From ${new Date(s).toLocaleDateString(undefined, { day: "numeric", month: "long" })}` };
  }
  const s = startOfDay(anchor);
  return { from: s, to: addDays(s, 1), days: [s], title: new Date(s).toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long", year: "numeric" }) };
}

export function CalendarView() {
  const view = usePim((s) => s.view);
  const anchor = usePim((s) => s.anchor);
  const loaded = usePim((s) => s.loaded);
  const r = React.useMemo(() => range(view, anchor), [view, anchor]);
  const { items, loading, error } = useOccurrences(r.from, r.to);

  return (
    <section aria-label="Calendar" className="flex h-full min-w-0 bg-surface">
      <CalendarRail />
      <div className="flex min-w-0 flex-1 flex-col border-l border-border">
        <header className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-3">
          <Button variant="outline" size="sm" onClick={() => setPim({ anchor: Date.now() })} aria-keyshortcuts="T">Today</Button>
          <IconButton label="Previous" shortcut="K" size="iconSm" onClick={() => step(-1)}><ChevronLeft /></IconButton>
          <IconButton label="Next" shortcut="J" size="iconSm" onClick={() => step(1)}><ChevronRight /></IconButton>
          <h1 className="ml-1 min-w-0 truncate font-title text-[19px]" aria-live="polite">{r.title}</h1>
          {loading && <Spinner className="ml-1 size-3.5 border-[1.5px] text-muted-foreground" />}
          <div className="flex-1" />
          <div role="tablist" aria-label="Calendar view" className="flex rounded-control bg-surface-inset p-0.5 ring-1 ring-border">
            {(["day", "week", "month", "agenda"] as const).map((v) => (
              <button key={v} role="tab" type="button" aria-selected={view === v} onClick={() => setCalView(v)}
                aria-keyshortcuts={v[0].toUpperCase()}
                className={cn("h-7 rounded-[6px] px-3 text-[12.5px] font-medium capitalize", view === v ? "bg-surface shadow-theme-xs" : "text-muted-foreground hover:text-foreground")}>
                {v}
              </button>
            ))}
          </div>
          <Button size="sm" onClick={() => newEvent()} aria-keyshortcuts="C"><Plus /> New event</Button>
        </header>
        {error && <p role="alert" className="border-b border-border bg-[var(--iw-red-wash)] px-4 py-2 text-[13px] text-[var(--iw-red-text)]">Couldn't load events: {error}</p>}
        <div className="min-h-0 flex-1">
          {!loaded ? (
            <div aria-hidden className="space-y-3 p-6">{Array.from({ length: 8 }).map((_, i) => <Skeleton key={i} className="h-10 w-full" />)}</div>
          ) : view === "month" ? (
            <MonthGrid first={r.from} month={startOfMonth(anchor)} items={items} />
          ) : view === "agenda" ? (
            <Agenda from={r.from} items={items} />
          ) : (
            <TimeGrid days={r.days} items={items} />
          )}
        </div>
      </div>
    </section>
  );
}

function MiniMonth() {
  const anchor = usePim((s) => s.anchor);
  const [shown, setShown] = React.useState(startOfMonth(anchor));
  React.useEffect(() => setShown(startOfMonth(anchor)), [anchor]);
  const first = startOfWeek(shown);
  const days = Array.from({ length: 42 }, (_, i) => addDays(first, i));
  const now = Date.now();
  return (
    <div className="px-3">
      <div className="mb-1 flex items-center">
        <span className="flex-1 pl-1 text-[13px] font-semibold">{fmtMonth(shown)}</span>
        <IconButton label="Previous month" size="iconSm" onClick={() => { const d = new Date(shown); d.setMonth(d.getMonth() - 1); setShown(d.getTime()); }}><ChevronLeft /></IconButton>
        <IconButton label="Next month" size="iconSm" onClick={() => { const d = new Date(shown); d.setMonth(d.getMonth() + 1); setShown(d.getTime()); }}><ChevronRight /></IconButton>
      </div>
      <div className="grid grid-cols-7 text-center text-[10.5px] text-muted-foreground" aria-hidden>
        {days.slice(0, 7).map((d) => <span key={d}>{new Date(d).toLocaleDateString(undefined, { weekday: "narrow" })}</span>)}
      </div>
      <div className="grid grid-cols-7" role="grid" aria-label="Pick a date">
        {days.map((d) => {
          const inMonth = new Date(d).getMonth() === new Date(shown).getMonth();
          const selected = sameDay(d, anchor);
          return (
            <button key={d} type="button" onClick={() => setPim({ anchor: d })} aria-label={new Date(d).toDateString()} aria-current={sameDay(d, now) ? "date" : undefined}
              className={cn("m-px flex h-7 items-center justify-center rounded-full text-[12px] tabular-nums hover:bg-accent",
                !inMonth && "text-muted-foreground/70", selected && "bg-select-strong font-semibold", sameDay(d, now) && "bg-primary font-semibold text-white hover:bg-primary")}>
              {new Date(d).getDate()}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function StatusIcon({ a }: { a: PimAccount }) {
  if (a.status === "syncing") return <Spinner className="size-3 border-[1.5px] text-muted-foreground" />;
  if (a.status === "authError") return <span title={a.statusMessage} className="text-[var(--iw-warn-text)]"><AlertTriangle className="size-3.5" /><span className="sr-only">Needs sign-in: {a.statusMessage}</span></span>;
  if (a.status === "offline") return <span title={a.statusMessage} className="text-muted-foreground"><WifiOff className="size-3.5" /><span className="sr-only">Offline</span></span>;
  if (a.status === "error") return <span title={a.statusMessage} className="text-[var(--iw-red-text)]"><AlertTriangle className="size-3.5" /><span className="sr-only">Error: {a.statusMessage}</span></span>;
  return null;
}

function CalendarRail() {
  const accounts = usePim((s) => s.accounts);
  const calendars = usePim((s) => s.calendars);
  const [fix, setFix] = React.useState<PimAccount | null>(null);
  const [removing, setRemoving] = React.useState<PimAccount | null>(null);
  const [creating, setCreating] = React.useState<PimAccount | null>(null);

  return (
    <aside aria-label="Calendars" className="flex w-64 shrink-0 flex-col overflow-y-auto py-3">
      <PimHeader />
      <MiniMonth />
      <div className="mt-4 flex-1 space-y-4 px-2">
        {accounts.filter((a) => a.enabled).map((a) => {
          const cals = calendars.filter((c) => c.accountId === a.id);
          return (
            <section key={a.id} aria-label={a.name}>
              <div className="group flex items-center gap-1.5 px-2 pb-1">
                <span className="min-w-0 flex-1 truncate text-[11px] font-semibold uppercase tracking-[0.06em] text-muted-foreground">{a.name}</span>
                <StatusIcon a={a} />
                <Menu.Root>
                  <Menu.Trigger asChild><IconButton label={`${a.name} options`} size="iconSm"><MoreHorizontal /></IconButton></Menu.Trigger>
                  <Menu.Content align="end">
                    {a.kind !== "local" && <Menu.Item onSelect={() => pimApi.syncNow(a.id).catch((e) => notify.error(e, "Sync failed"))}><RefreshCw /> Sync now</Menu.Item>}
                    {a.status === "authError" && <Menu.Item onSelect={() => setFix(a)}><KeyRound /> {a.kind === "dav" ? "Enter password…" : "Sign in again…"}</Menu.Item>}
                    <Menu.Item onSelect={() => setCreating(a)}><CalendarPlus /> New calendar…</Menu.Item>
                    {a.kind !== "local" && <><Menu.Separator /><Menu.Item danger onSelect={() => setRemoving(a)}><Trash2 /> Remove account…</Menu.Item></>}
                  </Menu.Content>
                </Menu.Root>
              </div>
              {a.status === "authError" && (
                <button type="button" onClick={() => setFix(a)} className="mx-2 mb-1 block w-[calc(100%-16px)] rounded-lg bg-surface-raised px-2.5 py-1.5 text-left text-[12px] ring-1 ring-[var(--iw-warn)]/60 hover:bg-accent">
                  {a.statusMessage ?? "Sign-in needed"} <span className="font-semibold text-[var(--iw-red-text)]">Fix</span>
                </button>
              )}
              <ul role="list">
                {cals.map((c) => <CalendarRow key={c.id} c={c} />)}
                {!cals.length && a.status === "ok" && <li className="px-3 py-1 text-[12px] text-muted-foreground">No calendars</li>}
              </ul>
            </section>
          );
        })}
      </div>
      <div className="mt-3 space-y-0.5 border-t border-border px-2 pt-2">
        <Button variant="ghost" size="sm" className="w-full justify-start text-muted-foreground" onClick={() => setPim({ accountDialog: true })}><UserPlus /> Add calendar account</Button>
        <Button variant="ghost" size="sm" className="w-full justify-start text-muted-foreground" onClick={() => setPim({ importDialog: true })}><Download /> Import from Betterbird</Button>
      </div>
      <FixAccountDialog account={fix} onClose={() => setFix(null)} />
      <NewCalendarDialog account={creating} onClose={() => setCreating(null)} />
      <ConfirmDialog open={!!removing} onOpenChange={(o) => !o && setRemoving(null)} title={`Remove “${removing?.name ?? ""}”?`}
        body="Its calendars and contacts disappear from WolfMail. Nothing is deleted on the server." confirmLabel="Remove" destructive
        onConfirm={async () => {
          if (!removing) return;
          try { await pimApi.accountRemove(removing.id); notify.ok(`Removed ${removing.name}`); } catch (e) { notify.error(e, "Couldn't remove the account"); }
        }} />
    </aside>
  );
}

function CalendarRow({ c }: { c: Calendar }) {
  const [deleting, setDeleting] = React.useState(false);
  const accounts = usePim((s) => s.accounts);
  const local = accounts.find((a) => a.id === c.accountId)?.kind === "local";
  const update = (patch: { color?: string; visible?: boolean }) => pimApi.calendarUpdate(c.id, patch).catch((e) => notify.error(e, "Couldn't change the calendar"));
  return (
    <li className="group flex h-8 items-center gap-2 rounded-lg px-2 hover:bg-accent">
      <button type="button" role="checkbox" aria-checked={c.visible} aria-label={`Show ${c.name}`} onClick={() => update({ visible: !c.visible })}
        className="flex size-4 shrink-0 items-center justify-center rounded ring-2 ring-inset" style={{ background: c.visible ? c.color : "transparent", boxShadow: `inset 0 0 0 2px ${c.color}` }}>
        {c.visible ? <Eye className="size-2.5 text-white" aria-hidden /> : <EyeOff className="size-2.5" style={{ color: c.color }} aria-hidden />}
      </button>
      <span className={cn("min-w-0 flex-1 truncate text-[13px]", !c.visible && "text-muted-foreground")}>{c.name}</span>
      {c.readOnly && <Lock className="size-3 text-muted-foreground" aria-label="Read-only" />}
      <Popover.Root>
        <Popover.Trigger asChild>
          <button type="button" aria-label={`Colour of ${c.name}`} className="hidden size-6 items-center justify-center rounded hover:bg-surface group-hover:flex focus-visible:flex">
            <span className="size-3 rounded-full" style={{ background: c.color }} />
          </button>
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content sideOffset={6} className="z-[80] rounded-table bg-popover p-2 shadow-[var(--shadow-pop)] ring-1 ring-border">
            <div role="radiogroup" aria-label="Calendar colour" className="grid grid-cols-4 gap-1.5">
              {SWATCHES.map((s) => (
                <button key={s} type="button" role="radio" aria-checked={c.color.toLowerCase() === s} aria-label={s} onClick={() => update({ color: s })}
                  className={cn("size-7 rounded-full ring-2 ring-offset-2 ring-offset-popover", c.color.toLowerCase() === s ? "ring-foreground" : "ring-transparent")} style={{ background: s }} />
              ))}
            </div>
            {local && <Button variant="ghost" size="sm" className="mt-2 w-full text-[var(--iw-red-text)]" onClick={() => setDeleting(true)}><Trash2 /> Delete calendar</Button>}
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>
      <ConfirmDialog open={deleting} onOpenChange={setDeleting} title={`Delete “${c.name}”?`} body="All its events are deleted from this computer. This can't be undone." confirmLabel="Delete calendar" destructive
        onConfirm={async () => { try { await pimApi.calendarDelete(c.id); notify.ok(`Deleted “${c.name}”`); } catch (e) { notify.error(e, "Couldn't delete the calendar"); } }} />
    </li>
  );
}

function NewCalendarDialog({ account, onClose }: { account: PimAccount | null; onClose: () => void }) {
  const [name, setName] = React.useState("");
  const [err, setErr] = React.useState<string>();
  const [busy, setBusy] = React.useState(false);
  React.useEffect(() => { if (account) { setName(""); setErr(undefined); } }, [account]);
  const submit = async () => {
    if (!account) return;
    if (!name.trim()) { setErr("Give the calendar a name"); return; }
    setBusy(true);
    try { await pimApi.calendarCreate(account.id, name.trim(), SWATCHES[Math.floor(Math.random() * SWATCHES.length)]); notify.ok(`Calendar “${name.trim()}” created`); onClose(); }
    catch (e) { setErr((e as { message?: string }).message ?? String(e)); }
    finally { setBusy(false); }
  };
  return (
    <Dialog open={!!account} onOpenChange={(o) => !o && onClose()} title={`New calendar in ${account?.name ?? ""}`}
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button onClick={submit} disabled={busy}>{busy && <Spinner />}Create</Button></>}>
      <form onSubmit={(e) => { e.preventDefault(); void submit(); }}>
        <Field id="newcal-name" label="Name" error={err}><Input autoFocus value={name} onChange={(e) => setName(e.target.value)} /></Field>
      </form>
    </Dialog>
  );
}

/** Enter a DAV password again, or redo a Google/Microsoft sign-in. */
function FixAccountDialog({ account, onClose }: { account: PimAccount | null; onClose: () => void }) {
  const [pass, setPass] = React.useState("");
  const [err, setErr] = React.useState<string>();
  const [busy, setBusy] = React.useState(false);
  React.useEffect(() => { if (account) { setPass(""); setErr(undefined); } }, [account]);
  const submit = async () => {
    if (!account) return;
    setBusy(true);
    setErr(undefined);
    try {
      if (account.kind === "dav") {
        if (!pass) { setErr("Enter the password"); return; }
        await pimApi.accountUpdate(account.id, {}, pass);
      } else {
        await pimApi.oauthAuthorize(account.kind === "google" ? "google" : "microsoft", account.email ?? "");
      }
      await pimApi.syncNow(account.id);
      notify.ok(`${account.name} is syncing again`);
      onClose();
    } catch (e) {
      setErr((e as { message?: string }).message ?? String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={!!account} onOpenChange={(o) => !o && onClose()} title={account?.kind === "dav" ? "Enter password" : "Sign in again"}
      description={account?.statusMessage}
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button onClick={submit} disabled={busy}>{busy && <Spinner />}{account?.kind === "dav" ? "Save and sync" : "Open sign-in"}</Button></>}>
      {account?.kind === "dav" ? (
        <form onSubmit={(e) => { e.preventDefault(); void submit(); }} className="space-y-3">
          <p className="text-[13px] text-muted-foreground">{account.username} at {account.url}</p>
          <Field id="fix-pass" label="Password or app password" error={err}><Input autoFocus type="password" value={pass} onChange={(e) => setPass(e.target.value)} /></Field>
        </form>
      ) : (
        <div className="space-y-2 text-[13px]">
          <p>Your browser opens so you can sign in to {account?.kind === "google" ? "Google" : "Microsoft"} as <b>{account?.email}</b> and allow calendar and contacts access.</p>
          {err && <p role="alert" className="rounded-lg bg-[var(--iw-red-wash)] p-2.5 text-[var(--iw-red-text)]">{err}</p>}
        </div>
      )}
    </Dialog>
  );
}
