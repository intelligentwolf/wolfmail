import * as React from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Sparkles, History } from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { getState, useStore } from "../../lib/store";
import type { IwField, IwPerson, MessageSummary } from "../../lib/types";
import { displayName } from "../../lib/utils";
import { Dialog } from "../../components/ui/dialog";
import { Button } from "../../components/ui/button";
import { Field, Input, Switch, Textarea } from "../../components/ui/form";
import { Skeleton, Spinner } from "../../components/ui/misc";
import { invalidatePeople, otherParty } from "./people";

/** Who and what: someone reading an email from a customer wants that email, and any new details in it (a new phone
 *  number, a job title from the signature), on the customer's IntelligentWolf record in one step, without retyping.
 *  Flow: right-click → this dialog (the record's current details load) → optionally "Find details in this email"
 *  (the assistant reads it; costs credits, so it is a button, never automatic) → tick what to change → Save.
 *  Opened by `wm:iw-update` (detail = message id). For someone not in IntelligentWolf yet, Save adds them as a lead. */
const LABELS: Record<IwField, string> = {
  name: "Name", company: "Company", phone: "Phone", mobile: "Mobile", job_title: "Job title", website: "Website",
  address_line1: "Address", address_line2: "Address line 2", city: "Town / city", region: "County / region",
  postcode: "Postcode", country: "Country", linkedin_url: "LinkedIn",
};
const ORDER = Object.keys(LABELS) as IwField[];

export function UpdateContactHost() {
  const [row, setRow] = React.useState<MessageSummary | null>(null);
  React.useEffect(() => {
    const h = (e: Event) => {
      const id = (e as CustomEvent<string>).detail;
      const m = getState().list.items.find((x) => x.id === id);
      if (m) setRow(m);
    };
    window.addEventListener("wm:iw-update", h);
    return () => window.removeEventListener("wm:iw-update", h);
  }, []);
  return row ? <UpdateContactDialog key={row.id} row={row} onClose={() => setRow(null)} /> : null;
}

type Change = { value: string; on: boolean };

function UpdateContactDialog({ row, onClose }: { row: MessageSummary; onClose: () => void }) {
  const accounts = useStore((s) => s.accounts);
  const assistant = useStore((s) => s.settings?.assistantName || "Mike");
  const mine = React.useMemo(() => new Set(accounts.flatMap((a) => a.identities.map((i) => i.email.toLowerCase()))), [accounts]);
  const who = otherParty(row, mine) ?? row.from;
  const [person, setPerson] = React.useState<IwPerson | null | undefined>(undefined);
  const [loadErr, setLoadErr] = React.useState<string>();
  const [name, setName] = React.useState(displayName(who));
  const [nameErr, setNameErr] = React.useState<string>();
  const [logIt, setLogIt] = React.useState(true);
  const [note, setNote] = React.useState("");
  const [changes, setChanges] = React.useState<Partial<Record<IwField, Change>>>({});
  const [finding, setFinding] = React.useState(false);
  const [found, setFound] = React.useState<string | null>(null);
  const [saving, setSaving] = React.useState(false);

  React.useEffect(() => {
    let live = true;
    api.iwPerson(who.email).then((p) => { if (live) setPerson(p); })
      .catch((e) => { if (live) { setLoadErr((e as { message?: string }).message ?? String(e)); setPerson(null); } });
    return () => { live = false; };
  }, [who.email]);

  const current = person?.fields ?? {};
  // A contact (accounts directory) has fewer fields than a lead: only offer what the server will accept.
  const allowed = person ? ORDER.filter((f) => f in current) : ORDER;

  const find = async () => {
    setFinding(true);
    try {
      const r = await api.iwExtractContact(row.id);
      const next: Partial<Record<IwField, Change>> = {};
      for (const f of allowed) {
        const v = (r.fields[f] ?? "").trim();
        if (v && v !== (current[f] ?? "").trim() && !(f === "name" && !person)) next[f] = { value: v, on: true };
      }
      if (!person && r.fields.name?.trim()) setName(r.fields.name.trim());
      setChanges(next);
      if (r.note && !note.trim()) setNote(r.note);
      const n = Object.keys(next).length;
      setFound(n ? `${assistant} found ${n} detail${n === 1 ? "" : "s"} to update.` : `${assistant} found nothing new in this email.`);
    } catch (e) { notify.error(e, `${assistant} couldn't read the email`); }
    finally { setFinding(false); }
  };

  const save = async () => {
    if (!person && !name.trim()) { setNameErr("Enter their name"); return; }
    setSaving(true);
    try {
      const fields: Partial<Record<IwField, string>> = {};
      for (const [f, c] of Object.entries(changes) as [IwField, Change][]) if (c.on && c.value.trim()) fields[f] = c.value.trim();
      let p = person;
      if (!p) p = await api.iwContactCreate(who.email, { ...fields, name: name.trim() });
      const kind = p.kind;
      const id = kind === "lead" ? p.leadId : p.contactId;
      if (!id) throw { kind: "server", message: "IntelligentWolf didn't say which record this is." };
      const changed = person ? Object.keys(fields).length > 0 : false;
      if (changed || note.trim()) await api.iwContactUpdate(kind, id, person ? fields : {}, note.trim() || undefined);
      if (logIt) await api.iwLogEmail(row.id, { leadId: p.leadId, contactId: kind === "contact" ? p.contactId : undefined, includeText: true });
      invalidatePeople(who.email);
      const url = p.url;
      notify.ok(person ? `Updated ${p.name} in IntelligentWolf` : `Added ${p.name} to IntelligentWolf`, {
        action: url ? { label: "Open", onClick: () => { openUrl(url).catch((e) => notify.error(e, "Couldn't open IntelligentWolf")); } } : undefined,
      });
      onClose();
    } catch (e) { notify.error(e, person ? "Couldn't update the contact" : "Couldn't add them to IntelligentWolf"); }
    finally { setSaving(false); }
  };

  const title = person === undefined ? "IntelligentWolf" : person ? `Update ${person.name}` : `Add ${displayName(who)} to IntelligentWolf`;
  const nothing = !!person && !logIt && !note.trim() && !Object.values(changes).some((c) => c?.on);

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()} title={title}
      description={<>From “{row.subject || "(no subject)"}” · <span className="selectable">{who.email}</span></>}
      footer={<>
        <Button variant="outline" onClick={onClose}>Cancel</Button>
        <Button onClick={() => void save()} disabled={saving || person === undefined || nothing}>{saving && <Spinner />} {person ? "Save to IntelligentWolf" : "Add to IntelligentWolf"}</Button>
      </>}>
      {person === undefined ? (
        <div aria-hidden className="space-y-3"><Skeleton className="h-10 w-full" /><Skeleton className="h-24 w-full" /></div>
      ) : (
        <form className="space-y-5" onSubmit={(e) => { e.preventDefault(); void save(); }} noValidate>
          {loadErr && <p role="alert" className="rounded-lg bg-[var(--iw-red-wash)] p-3 text-xs text-[var(--iw-red-text)]">Couldn't reach IntelligentWolf: {loadErr}</p>}
          {!person && (
            <Field id="iwu-name" label="Name" error={nameErr}><Input autoFocus value={name} onChange={(e) => { setName(e.target.value); setNameErr(undefined); }} /></Field>
          )}

          <div className="rounded-card ring-1 ring-border px-4">
            <Switch id="iwu-log" checked={logIt} onCheckedChange={setLogIt} label="Add this email to their history"
              description="The whole email goes on their IntelligentWolf timeline, so the team can read it there." />
          </div>

          <section aria-labelledby="iwu-details" className="space-y-3">
            <div className="flex items-center gap-2">
              <h3 id="iwu-details" className="text-[13px] font-semibold">Details</h3>
              <div className="flex-1" />
              <Button type="button" size="sm" variant="secondary" onClick={() => void find()} disabled={finding}>
                {finding ? <Spinner /> : <Sparkles className="text-[var(--iw-red-text)]" />} Find details in this email
              </Button>
            </div>
            <p className="text-xs text-muted-foreground">{assistant} reads this one email for a new phone number, job title, address and so on. Uses your IntelligentWolf credits.</p>
            {found && <p role="status" className="text-xs font-medium">{found}</p>}
            {(Object.entries(changes) as [IwField, Change][]).map(([f, c]) => (
              <div key={f} className="flex items-start gap-3 rounded-lg bg-surface-raised p-3 ring-1 ring-border">
                <input id={`iwu-on-${f}`} type="checkbox" className="mt-2 size-4 accent-[var(--iw-red)]" checked={c.on}
                  onChange={(e) => setChanges((x) => ({ ...x, [f]: { ...c, on: e.target.checked } }))} />
                <div className="min-w-0 flex-1 space-y-1">
                  <label htmlFor={`iwu-v-${f}`} className="block text-[12.5px] font-medium">{LABELS[f]}</label>
                  <Input id={`iwu-v-${f}`} value={c.value} disabled={!c.on} aria-describedby={`iwu-was-${f}`}
                    onChange={(e) => setChanges((x) => ({ ...x, [f]: { ...c, value: e.target.value } }))} />
                  <p id={`iwu-was-${f}`} className="truncate text-[11.5px] text-muted-foreground">
                    {current[f] ? <>Now: <span className="selectable">{current[f]}</span></> : "Not set yet"}
                  </p>
                </div>
              </div>
            ))}
          </section>

          <Field id="iwu-note" label="Note for their history (optional)" hint="Something worth remembering about them from this email.">
            <Textarea rows={3} value={note} onChange={(e) => setNote(e.target.value)} />
          </Field>
          {person && (
            <p className="flex items-center gap-1.5 text-[11.5px] text-muted-foreground"><History className="size-3.5" aria-hidden /> Changes are saved by IntelligentWolf as you, and its automations run as usual.</p>
          )}
        </form>
      )}
    </Dialog>
  );
}
