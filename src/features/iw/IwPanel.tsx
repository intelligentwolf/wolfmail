import * as React from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Sparkles, ClipboardCheck, BookmarkPlus, ExternalLink, Building2, Phone, Mail, Calendar, FileText, MessageSquare, Receipt,
  UserRound, ListChecks, X,
} from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { selectOpenRow, setUi, useStore } from "../../lib/store";
import type { IwPerson, MessageSummary } from "../../lib/types";
import { cn, displayName, listDate } from "../../lib/utils";
import { Avatar, EmptyState, Skeleton, Spinner } from "../../components/ui/misc";
import { Button, IconButton } from "../../components/ui/button";
import { Dialog } from "../../components/ui/dialog";
import { Field, Input } from "../../components/ui/form";

const kindIcon = { email: Mail, call: Phone, meeting: Calendar, note: MessageSquare, task: ListChecks, invoice: Receipt } as const;

/** Right-hand panel: who the sender is in IntelligentWolf, and the IW actions for this message. Only the sender's
 *  address is sent to IW to look them up; message text goes only when the user presses Summarise/Log/Task. */
export function IwPanel() {
  const row = useStore(selectOpenRow);
  const accounts = useStore((s) => s.accounts);
  const mine = React.useMemo(() => new Set(accounts.flatMap((a) => a.identities.map((i) => i.email.toLowerCase()))), [accounts]);
  if (!row) {
    return (
      <Shell>
        <EmptyState icon={<UserRound className="size-6" />} title="Open a message" body="The sender's IntelligentWolf record shows here." />
      </Shell>
    );
  }
  const person = mine.has(row.from.email.toLowerCase()) ? row.to[0] ?? row.from : row.from;
  return (
    <Shell>
      <PersonCard key={person.email + row.id} email={person.email} name={person.name} row={row} />
    </Shell>
  );
}

function Shell({ children }: { children: React.ReactNode }) {
  const org = useStore((s) => s.iw?.org?.name);
  return (
    <aside aria-label="IntelligentWolf" className="flex h-full flex-col border-l border-border bg-surface">
      <div className="flex items-center gap-2 border-b border-border px-4 py-3">
        <img src="/icon.png" alt="" className="size-5 rounded" />
        <span className="font-title text-[14px]">IntelligentWolf</span>
        <span className="truncate text-xs text-muted-foreground">{org}</span>
        <div className="flex-1" />
        <IconButton label="Hide panel" shortcut="I" size="iconSm" onClick={() => setUi({ iwPanel: false })}><X /></IconButton>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">{children}</div>
    </aside>
  );
}

function PersonCard({ email, name, row }: { email: string; name?: string; row: MessageSummary }) {
  const [person, setPerson] = React.useState<IwPerson | null | undefined>(undefined);
  const [error, setError] = React.useState<string>();
  const [summary, setSummary] = React.useState<{ text: string; credits: number } | null>(null);
  const [busy, setBusy] = React.useState<null | "sum" | "log">(null);
  const [taskOpen, setTaskOpen] = React.useState(false);

  React.useEffect(() => {
    let live = true;
    api.iwPerson(email).then((p) => live && setPerson(p)).catch((e) => { if (live) { setError((e as { message?: string }).message ?? String(e)); setPerson(null); } });
    return () => { live = false; };
  }, [email]);

  const summarise = async () => {
    setBusy("sum");
    try { const r = await api.iwAi("summarise", row.id); setSummary({ text: r.text, credits: r.creditsUsed }); }
    catch (e) { notify.error(e, "Alex couldn't summarise this"); }
    finally { setBusy(null); }
  };
  const log = async () => {
    setBusy("log");
    try { await api.iwLogEmail(row.id, person?.leadId); notify.ok(person ? `Logged to ${person.name}'s timeline` : "Logged — a new lead was created in IntelligentWolf"); }
    catch (e) { notify.error(e, "Couldn't log the email"); }
    finally { setBusy(null); }
  };

  return (
    <div className="space-y-4 p-4">
      <div className="flex items-center gap-3">
        <Avatar address={{ email, name }} size={48} />
        <div className="min-w-0">
          <div className="truncate text-[15px] font-semibold">{person?.name || displayName({ email, name })}</div>
          <div className="selectable truncate text-xs text-muted-foreground">{email}</div>
        </div>
      </div>

      {person === undefined ? (
        <div aria-hidden className="space-y-2"><Skeleton className="h-16 w-full rounded-xl" /><Skeleton className="h-3 w-2/3" /><Skeleton className="h-3 w-1/2" /></div>
      ) : error ? (
        <p role="alert" className="rounded-lg bg-[var(--iw-red-wash)] p-3 text-xs text-[var(--iw-red-text)]">Couldn't reach IntelligentWolf: {error}</p>
      ) : person ? (
        <div className="space-y-3 rounded-card bg-surface-raised p-3.5 ring-1 ring-border">
          <div className="flex items-center gap-2">
            <span className={cn("rounded-full px-2 py-0.5 text-[11px] font-semibold", person.kind === "lead" ? "bg-[var(--iw-red-wash)] text-[var(--iw-red-text)]" : "bg-plum/10 text-foreground")}>
              {person.kind === "lead" ? "Lead" : "Contact"}
            </span>
            {person.stage && <span className="text-xs font-medium">{person.stage}</span>}
            {typeof person.value === "number" && (
              <span className="ml-auto text-[13px] font-semibold tabular-nums">
                {new Intl.NumberFormat(undefined, { style: "currency", currency: person.currency || "GBP", maximumFractionDigits: 0 }).format(person.value)}
              </span>
            )}
          </div>
          <dl className="space-y-1.5 text-xs">
            {person.company && <Row icon={<Building2 />} label="Company">{person.company}</Row>}
            {person.phone && <Row icon={<Phone />} label="Phone"><span className="selectable">{person.phone}</span></Row>}
            {person.owner && <Row icon={<UserRound />} label="Owner">{person.owner}</Row>}
            {person.lastContactAt && <Row icon={<Calendar />} label="Last contact">{listDate(person.lastContactAt)}</Row>}
            {typeof person.score === "number" && <Row icon={<Sparkles />} label="Score">{person.score}</Row>}
          </dl>
          <Button variant="outline" size="sm" className="w-full" onClick={() => openUrl(person.url).catch((e) => notify.error(e, "Couldn't open IntelligentWolf"))}>
            <ExternalLink /> Open in IntelligentWolf
          </Button>
        </div>
      ) : (
        <div className="rounded-card bg-surface-raised p-3.5 text-xs text-muted-foreground ring-1 ring-border">
          Not in your IntelligentWolf CRM yet. <b className="text-foreground">Log to CRM</b> creates a lead from this email.
        </div>
      )}

      <div className="grid grid-cols-1 gap-2">
        <Button variant="secondary" className="justify-start" disabled={busy !== null} onClick={summarise}>
          {busy === "sum" ? <Spinner /> : <Sparkles className="text-[var(--iw-red-text)]" />} Summarise with Alex
        </Button>
        <Button variant="secondary" className="justify-start" disabled={busy !== null} onClick={log}>
          {busy === "log" ? <Spinner /> : <BookmarkPlus />} Log to CRM
        </Button>
        <Button variant="secondary" className="justify-start" onClick={() => setTaskOpen(true)}><ClipboardCheck /> Make a task</Button>
      </div>
      <p className="text-[11px] leading-relaxed text-muted-foreground">Summarise and Log send this one message to IntelligentWolf. Summaries use your IntelligentWolf credits.</p>

      {summary && (
        <section aria-label="Summary" className="rounded-card bg-[var(--iw-red-wash)] p-3.5 ring-1 ring-[var(--iw-red)]/20">
          <div className="mb-1.5 flex items-center gap-1.5 text-xs font-semibold"><Sparkles className="size-3.5 text-[var(--iw-red-text)]" /> Alex's summary <span className="ml-auto font-normal text-muted-foreground">{summary.credits} credits</span></div>
          <p className="selectable whitespace-pre-wrap text-[13px] leading-relaxed">{summary.text}</p>
        </section>
      )}

      {person?.timeline?.length ? (
        <section aria-label="Recent activity">
          <h3 className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Recent activity</h3>
          <ol className="space-y-2.5 border-l border-border pl-3">
            {person.timeline.map((t, i) => {
              const Icon = kindIcon[t.kind] ?? FileText;
              return (
                <li key={i} className="relative">
                  <span className="absolute -left-[19px] top-0.5 flex size-[13px] items-center justify-center rounded-full bg-surface ring-1 ring-border"><Icon className="size-2" /></span>
                  <div className="text-[12.5px] leading-snug">{t.title}</div>
                  <div className="text-[11px] text-muted-foreground">{listDate(t.at)}</div>
                </li>
              );
            })}
          </ol>
        </section>
      ) : null}

      <TaskDialog open={taskOpen} onClose={() => setTaskOpen(false)} row={row} />
    </div>
  );
}

function Row({ icon, label, children }: { icon: React.ReactNode; label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-2 [&_svg]:size-3.5 [&_svg]:text-muted-foreground">
      {icon}<dt className="sr-only">{label}</dt><dd className="min-w-0 truncate">{children}</dd>
    </div>
  );
}

function TaskDialog({ open, onClose, row }: { open: boolean; onClose: () => void; row: MessageSummary }) {
  const [title, setTitle] = React.useState("");
  const [due, setDue] = React.useState("");
  const [err, setErr] = React.useState<string>();
  const [busy, setBusy] = React.useState(false);
  React.useEffect(() => { if (open) { setTitle(`Follow up: ${row.subject}`); setDue(""); setErr(undefined); } }, [open, row.subject]);
  const submit = async () => {
    if (!title.trim()) { setErr("Give the task a title"); return; }
    setBusy(true);
    try {
      await api.iwCreateTask(row.id, title.trim(), due ? new Date(due).getTime() : undefined);
      notify.ok("Task created in IntelligentWolf");
      onClose();
    } catch (e) { notify.error(e, "Couldn't create the task"); }
    finally { setBusy(false); }
  };
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()} title="Make a task" description="The task links back to this email in IntelligentWolf."
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button onClick={submit} disabled={busy}>{busy && <Spinner />} Create task</Button></>}>
      <form className="space-y-4" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
        <Field id="task-title" label="Task" error={err}><Input autoFocus value={title} onChange={(e) => setTitle(e.target.value)} /></Field>
        <Field id="task-due" label="Due (optional)"><Input type="date" value={due} onChange={(e) => setDue(e.target.value)} /></Field>
      </form>
    </Dialog>
  );
}
