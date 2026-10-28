import * as React from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import {
  Search, X, UserPlus, Mail, CalendarPlus, Pencil, Trash2, Phone, MapPin, Building2, Cake, Lock, Upload, Download, Users, StickyNote, Plus, Tag,
} from "lucide-react";
import { pimApi } from "../../lib/pimApi";
import { notify } from "../../lib/notify";
import { setPim, usePim } from "../../lib/pim";
import type { Contact, Labeled } from "../../lib/pimTypes";
import { openComposer } from "../../lib/store";
import { cn } from "../../lib/utils";
import { Avatar, EmptyState, Skeleton, Spinner } from "../../components/ui/misc";
import { Button, IconButton } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/dialog";
import { Field, Input, NativeSelect, Textarea } from "../../components/ui/form";
import { newDraftFor } from "../compose/drafts";
import { newEventWith } from "../calendar/EventEditor";
import { PimHeader } from "../pim/ModeSwitch";

const LABELS = ["home", "work", "mobile", "other"];

function blank(bookId: string): Contact {
  return { id: "", bookId, uid: "", fullName: "", givenName: "", familyName: "", org: "", title: "", emails: [{ label: "work", value: "" }], phones: [], addresses: [], notes: "", categories: [], readOnly: false };
}

export function ContactsView() {
  const books = usePim((s) => s.books);
  const accounts = usePim((s) => s.accounts);
  const version = usePim((s) => s.version);
  const [bookId, setBookId] = React.useState<string | undefined>(undefined);
  const [q, setQ] = React.useState("");
  const [list, setList] = React.useState<Contact[] | null>(null);
  const [error, setError] = React.useState<string>();
  const [selected, setSelected] = React.useState<string | null>(null);
  const [editing, setEditing] = React.useState<Contact | null>(null);
  const searchRef = React.useRef<HTMLInputElement>(null);

  React.useEffect(() => {
    let live = true;
    const t = setTimeout(() => {
      pimApi.contactsList(bookId, q.trim() || undefined, 20000)
        .then((c) => { if (live) { setList(c); setError(undefined); } })
        .catch((e) => live && setError((e as { message?: string }).message ?? String(e)));
    }, q ? 120 : 0);
    return () => { live = false; clearTimeout(t); };
  }, [bookId, q, version]);

  const current = list?.find((c) => c.id === selected) ?? null;
  const writableBooks = books.filter((b) => !b.readOnly);
  const groups = React.useMemo(() => [...new Set((list ?? []).flatMap((c) => c.categories))].sort(), [list]);

  const parent = React.useRef<HTMLDivElement>(null);
  const virt = useVirtualizer({ count: list?.length ?? 0, getScrollElement: () => parent.current, estimateSize: () => 56, overscan: 12 });

  const exportAll = async () => {
    if (!list?.length) return;
    const path = await saveDialog({ defaultPath: "contacts.vcf", filters: [{ name: "vCard", extensions: ["vcf"] }] });
    if (!path) return;
    try { const n = await pimApi.contactsExportVcf(list.map((c) => c.id), path); notify.ok(`Exported ${n} contacts`); } catch (e) { notify.error(e, "Export failed"); }
  };
  const importVcf = async () => {
    const target = bookId && writableBooks.some((b) => b.id === bookId) ? bookId : writableBooks[0]?.id;
    if (!target) { notify.error({ kind: "invalid", message: "There's no address book you can add contacts to." }, "Can't import"); return; }
    const path = await openDialog({ filters: [{ name: "vCard", extensions: ["vcf", "vcard"] }], title: "Import contacts" });
    if (!path) return;
    try { const n = await pimApi.contactsImportVcf(target, String(path)); notify.ok(`Imported ${n} contacts`); } catch (e) { notify.error(e, "Import failed"); }
  };

  return (
    <section aria-label="Contacts" className="flex h-full min-w-0 bg-surface">
      <aside aria-label="Address books" className="flex w-64 shrink-0 flex-col gap-1 overflow-y-auto px-2 py-3">
        <div className="-mx-2"><PimHeader /></div>
        <h2 className="px-2 pb-1 font-title text-[18px]">Contacts</h2>
        <BookRow active={!bookId} label="All contacts" onClick={() => setBookId(undefined)} />
        {accounts.map((a) => {
          const bs = books.filter((b) => b.accountId === a.id);
          if (!bs.length) return null;
          return (
            <div key={a.id} className="pt-2">
              <div className="px-2 pb-1 text-[11px] font-semibold uppercase tracking-[0.06em] text-muted-foreground">{a.name}</div>
              {bs.map((b) => <BookRow key={b.id} active={bookId === b.id} label={b.name} readOnly={b.readOnly} onClick={() => setBookId(b.id)} />)}
            </div>
          );
        })}
        {groups.length > 0 && (
          <div className="pt-2">
            <div className="px-2 pb-1 text-[11px] font-semibold uppercase tracking-[0.06em] text-muted-foreground">Groups</div>
            {groups.map((g) => <BookRow key={g} active={q === g} label={g} icon={<Tag className="size-3.5" />} onClick={() => setQ(q === g ? "" : g)} />)}
          </div>
        )}
        <div className="mt-auto space-y-0.5 border-t border-border pt-2">
          <Button variant="ghost" size="sm" className="w-full justify-start text-muted-foreground" onClick={importVcf}><Upload /> Import vCard file…</Button>
          <Button variant="ghost" size="sm" className="w-full justify-start text-muted-foreground" onClick={exportAll} disabled={!list?.length}><Download /> Export shown contacts…</Button>
          <Button variant="ghost" size="sm" className="w-full justify-start text-muted-foreground" onClick={() => setPim({ accountDialog: true })}><Plus /> Add contacts account</Button>
        </div>
      </aside>

      <div className="flex w-[340px] shrink-0 flex-col border-l border-border">
        <div className="flex items-center gap-2 border-b border-border p-3">
          <div className="relative flex-1">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
            <input ref={searchRef} value={q} onChange={(e) => setQ(e.target.value)} aria-label="Search contacts" placeholder="Search name, email, company…"
              className="h-9 w-full rounded-control bg-input-background pl-8 pr-8 text-[13px] focus:outline-none focus:ring-1 focus:ring-[var(--iw-red)]" />
            {q && <button type="button" aria-label="Clear search" onClick={() => setQ("")} className="absolute right-1.5 top-1/2 flex size-6 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:bg-accent"><X className="size-3.5" /></button>}
          </div>
          <IconButton label="New contact" disabled={!writableBooks.length} onClick={() => { const b = bookId && writableBooks.some((x) => x.id === bookId) ? bookId : writableBooks[0].id; setEditing(blank(b)); }}><UserPlus /></IconButton>
        </div>
        <div className="px-3 py-1.5 text-xs tabular-nums text-muted-foreground" aria-live="polite">{list ? `${list.length.toLocaleString()} ${list.length === 1 ? "contact" : "contacts"}` : ""}</div>
        <div ref={parent} className="min-h-0 flex-1 overflow-y-auto" role="listbox" aria-label="Contacts">
          {error ? (
            <p role="alert" className="m-3 rounded-lg bg-[var(--iw-red-wash)] p-3 text-[13px] text-[var(--iw-red-text)]">Couldn't load contacts: {error}</p>
          ) : list === null ? (
            <div aria-hidden className="space-y-3 p-3">{Array.from({ length: 10 }).map((_, i) => <Skeleton key={i} className="h-10 w-full" />)}</div>
          ) : list.length === 0 ? (
            q ? <EmptyState tone="filtered" icon={<Search className="size-6" />} title="No matches" body={`No contact matches “${q}”.`} action={<Button variant="outline" onClick={() => setQ("")}>Clear search</Button>} />
              : <EmptyState icon={<Users className="size-6" />} title="No contacts yet" body="Add one, import a vCard file, or connect Google, Microsoft or a CardDAV server." action={writableBooks.length ? <Button onClick={() => setEditing(blank(writableBooks[0].id))}><UserPlus /> New contact</Button> : undefined} />
          ) : (
            <div style={{ height: virt.getTotalSize(), position: "relative" }}>
              {virt.getVirtualItems().map((vi) => {
                const c = list[vi.index];
                const primary = c.emails[0]?.value ?? c.phones[0]?.value ?? "";
                return (
                  <button key={c.id} type="button" role="option" aria-selected={selected === c.id} onClick={() => { setSelected(c.id); setEditing(null); }}
                    className={cn("absolute inset-x-0 flex items-center gap-3 px-3 text-left", selected === c.id ? "bg-select-strong" : "hover:bg-accent/70")}
                    style={{ top: vi.start, height: vi.size }}>
                    <Avatar address={{ email: primary || c.id, name: c.fullName || c.org }} size={34} />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[13.5px] font-medium">{c.fullName || c.org || primary || "(no name)"}</span>
                      <span className="block truncate text-xs text-muted-foreground">{c.org && c.fullName ? `${c.org} · ` : ""}{primary}</span>
                    </span>
                  </button>
                );
              })}
            </div>
          )}
        </div>
      </div>

      <div className="min-w-0 flex-1 overflow-y-auto border-l border-border">
        {editing ? (
          <ContactEditor initial={editing} onDone={(saved) => { setEditing(null); if (saved) setSelected(saved.id); }} />
        ) : current ? (
          <ContactDetail c={current} onEdit={() => setEditing(current)} onDeleted={() => setSelected(null)} />
        ) : (
          <EmptyState icon={<Users className="size-6" />} title="Pick a contact" body="Their details show here." />
        )}
      </div>
    </section>
  );
}

function BookRow({ active, label, onClick, readOnly, icon }: { active: boolean; label: string; onClick: () => void; readOnly?: boolean; icon?: React.ReactNode }) {
  return (
    <button type="button" onClick={onClick} aria-current={active ? "page" : undefined}
      className={cn("flex h-8 w-full items-center gap-2 rounded-lg px-2 text-left text-[13px]", active ? "bg-select-strong font-semibold" : "hover:bg-accent")}>
      {icon ?? <Users className="size-4 text-muted-foreground" />}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {readOnly && <span className="flex items-center gap-1 rounded-full px-1.5 text-[10.5px] text-muted-foreground ring-1 ring-border"><Lock className="size-2.5" /> read-only</span>}
    </button>
  );
}

function Row({ icon, children, label }: { icon: React.ReactNode; children: React.ReactNode; label?: string }) {
  return (
    <div className="flex gap-3 py-1.5 [&_svg]:size-4 [&_svg]:text-muted-foreground">
      <span className="mt-0.5 shrink-0">{icon}</span>
      <div className="min-w-0 flex-1 selectable text-[13.5px]">{children}{label && <span className="ml-2 text-xs capitalize text-muted-foreground">{label}</span>}</div>
    </div>
  );
}

function ContactDetail({ c, onEdit, onDeleted }: { c: Contact; onEdit: () => void; onDeleted: () => void }) {
  const [deleting, setDeleting] = React.useState(false);
  const email = c.emails[0]?.value;
  return (
    <article className="mx-auto max-w-xl px-8 py-8">
      <div className="flex items-center gap-4">
        {c.photo ? <img src={c.photo} alt="" className="size-20 rounded-full object-cover" /> : <Avatar address={{ email: email ?? c.id, name: c.fullName || c.org }} size={80} />}
        <div className="min-w-0">
          <h2 className="selectable truncate font-title text-[24px] leading-tight">{c.fullName || c.org || "(no name)"}</h2>
          {(c.title || c.org) && <p className="truncate text-[14px] text-muted-foreground">{[c.title, c.fullName ? c.org : ""].filter(Boolean).join(" · ")}</p>}
          {c.readOnly && <p className="mt-1 flex items-center gap-1 text-xs text-muted-foreground"><Lock className="size-3" /> Read-only here — edit it with your provider</p>}
        </div>
      </div>
      <div className="mt-5 flex flex-wrap gap-2">
        {email && <Button size="sm" onClick={() => openComposer({ mode: "new", draft: newDraftFor([{ email, name: c.fullName || undefined }]), quoteOnTop: true })}><Mail /> Email</Button>}
        {email && <Button size="sm" variant="outline" onClick={() => newEventWith(email, c.fullName || undefined)}><CalendarPlus /> New event with</Button>}
        {!c.readOnly && <Button size="sm" variant="outline" onClick={onEdit}><Pencil /> Edit</Button>}
        {!c.readOnly && <Button size="sm" variant="ghost" className="ml-auto text-[var(--iw-red-text)]" onClick={() => setDeleting(true)}><Trash2 /> Delete</Button>}
      </div>
      <div className="mt-6 divide-y divide-border rounded-card bg-surface-raised px-4 py-2 ring-1 ring-border">
        {c.emails.map((e, i) => <Row key={`e${i}`} icon={<Mail />} label={e.label}>{e.value}</Row>)}
        {c.phones.map((p, i) => <Row key={`p${i}`} icon={<Phone />} label={p.label}><a href={`tel:${p.value}`} className="hover:underline">{p.value}</a></Row>)}
        {c.addresses.map((a, i) => <Row key={`a${i}`} icon={<MapPin />} label={a.label}>{a.value}</Row>)}
        {c.org && <Row icon={<Building2 />}>{c.org}</Row>}
        {c.birthday && <Row icon={<Cake />}>{c.birthday.startsWith("--") ? new Date(`2000-${c.birthday.slice(2)}T12:00:00`).toLocaleDateString(undefined, { day: "numeric", month: "long" }) : new Date(`${c.birthday}T12:00:00`).toLocaleDateString(undefined, { day: "numeric", month: "long", year: "numeric" })}</Row>}
        {c.categories.length > 0 && <Row icon={<Tag />}>{c.categories.join(", ")}</Row>}
        {c.notes && <Row icon={<StickyNote />}><span className="whitespace-pre-wrap">{c.notes}</span></Row>}
      </div>
      <ConfirmDialog open={deleting} onOpenChange={setDeleting} title={`Delete ${c.fullName || "this contact"}?`} body="It's removed from this address book everywhere it syncs." confirmLabel="Delete contact" destructive
        onConfirm={async () => {
          try { await pimApi.contactDelete(c.id); notify.ok("Contact deleted"); onDeleted(); } catch (e) { notify.error(e, "Couldn't delete the contact"); }
        }} />
    </article>
  );
}

function LabeledList({ id, label, items, onChange, placeholder, type }: {
  id: string; label: string; items: Labeled[]; onChange: (v: Labeled[]) => void; placeholder: string; type?: string;
}) {
  return (
    <fieldset className="space-y-1.5">
      <legend className="mb-1 text-[13px] font-medium">{label}</legend>
      {items.map((it, i) => (
        <div key={i} className="flex gap-2">
          <NativeSelect value={LABELS.includes(it.label) ? it.label : "other"} onChange={(e) => onChange(items.map((x, j) => (j === i ? { ...x, label: e.target.value } : x)))} className="w-28 shrink-0" aria-label={`${label} ${i + 1} type`}>
            {LABELS.map((l) => <option key={l} value={l}>{l}</option>)}
          </NativeSelect>
          <Input id={i === 0 ? id : undefined} type={type} value={it.value} placeholder={placeholder} aria-label={`${label} ${i + 1}`}
            onChange={(e) => onChange(items.map((x, j) => (j === i ? { ...x, value: e.target.value } : x)))} />
          <IconButton label={`Remove ${label.toLowerCase()} ${i + 1}`} onClick={() => onChange(items.filter((_, j) => j !== i))}><X /></IconButton>
        </div>
      ))}
      <Button variant="ghost" size="sm" onClick={() => onChange([...items, { label: "work", value: "" }])}><Plus /> Add {({ Emails: "email", Phones: "phone", Addresses: "address" } as Record<string, string>)[label] ?? label.toLowerCase()}</Button>
    </fieldset>
  );
}

function ContactEditor({ initial, onDone }: { initial: Contact; onDone: (saved?: Contact) => void }) {
  const books = usePim((s) => s.books);
  const [c, setC] = React.useState<Contact>(initial);
  const [errors, setErrors] = React.useState<Record<string, string>>({});
  const [busy, setBusy] = React.useState(false);
  const summary = React.useRef<HTMLDivElement>(null);
  const set = (p: Partial<Contact>) => setC((x) => ({ ...x, ...p }));
  const isNew = !initial.id;

  const save = async () => {
    const errs: Record<string, string> = {};
    if (!c.fullName.trim() && !c.givenName.trim() && !c.familyName.trim() && !c.org.trim() && !c.emails.some((e) => e.value.trim())) errs.fullName = "Enter a name, a company or an email address";
    c.emails.forEach((e) => { if (e.value.trim() && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(e.value.trim())) errs.emails = `“${e.value}” isn't an email address`; });
    setErrors(errs);
    if (Object.keys(errs).length) { requestAnimationFrame(() => summary.current?.focus()); return; }
    setBusy(true);
    try {
      const saved = await pimApi.contactSave({ ...c, fullName: c.fullName.trim() || `${c.givenName} ${c.familyName}`.trim() });
      notify.ok(isNew ? "Contact added" : "Contact saved");
      onDone(saved);
    } catch (e) {
      const err = e as { field?: string; message?: string };
      if (err.field) { setErrors({ [err.field]: err.message ?? "Check this" }); requestAnimationFrame(() => summary.current?.focus()); }
      else notify.error(e, "Couldn't save the contact");
    } finally {
      setBusy(false);
    }
  };

  const errs = Object.entries(errors);
  return (
    <form className="mx-auto max-w-xl space-y-4 px-8 py-8" onSubmit={(e) => { e.preventDefault(); void save(); }}>
      <h2 className="font-title text-[22px]">{isNew ? "New contact" : `Edit ${initial.fullName || "contact"}`}</h2>
      {errs.length > 0 && (
        <div ref={summary} tabIndex={-1} role="alert" className="rounded-lg bg-[var(--iw-red-wash)] p-3 text-[13px] ring-1 ring-[var(--iw-red)]/40">
          {errs.map(([k, v]) => <p key={k}><a href={`#ct-${k}`} className="underline">{v}</a></p>)}
        </div>
      )}
      {isNew && (
        <Field id="ct-bookId" label="Address book" error={errors.bookId}>
          <NativeSelect value={c.bookId} onChange={(e) => set({ bookId: e.target.value })}>
            {books.filter((b) => !b.readOnly).map((b) => <option key={b.id} value={b.id}>{b.name}</option>)}
          </NativeSelect>
        </Field>
      )}
      <div className="grid gap-3 sm:grid-cols-2">
        <Field id="ct-givenName" label="First name"><Input value={c.givenName} onChange={(e) => set({ givenName: e.target.value })} autoFocus /></Field>
        <Field id="ct-familyName" label="Last name"><Input value={c.familyName} onChange={(e) => set({ familyName: e.target.value })} /></Field>
      </div>
      <Field id="ct-fullName" label="Display name" hint="Leave blank to use first and last name." error={errors.fullName}><Input value={c.fullName} onChange={(e) => set({ fullName: e.target.value })} /></Field>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field id="ct-org" label="Company"><Input value={c.org} onChange={(e) => set({ org: e.target.value })} /></Field>
        <Field id="ct-title" label="Job title"><Input value={c.title} onChange={(e) => set({ title: e.target.value })} /></Field>
      </div>
      {errors.emails && <p id="ct-emails-error" className="text-xs font-medium text-[var(--iw-red-text)]">{errors.emails}</p>}
      <LabeledList id="ct-emails" label="Emails" items={c.emails} onChange={(emails) => set({ emails })} placeholder="name@example.com" type="email" />
      <LabeledList id="ct-phones" label="Phones" items={c.phones} onChange={(phones) => set({ phones })} placeholder="+44 7700 900123" type="tel" />
      <LabeledList id="ct-addresses" label="Addresses" items={c.addresses} onChange={(addresses) => set({ addresses })} placeholder="Street, town, postcode, country" />
      <div className="grid gap-3 sm:grid-cols-2">
        <Field id="ct-birthday" label="Birthday" error={errors.birthday}><Input type="date" value={c.birthday && !c.birthday.startsWith("--") ? c.birthday : ""} onChange={(e) => set({ birthday: e.target.value || undefined })} /></Field>
        <Field id="ct-categories" label="Groups" hint="Separate with commas."><Input value={c.categories.join(", ")} onChange={(e) => set({ categories: e.target.value.split(",").map((s) => s.trim()).filter(Boolean) })} /></Field>
      </div>
      <Field id="ct-notes" label="Notes"><Textarea rows={4} value={c.notes} onChange={(e) => set({ notes: e.target.value })} /></Field>
      <div className="flex justify-end gap-2 border-t border-border pt-4">
        <Button variant="outline" onClick={() => onDone()}>Cancel</Button>
        <Button type="submit" disabled={busy}>{busy && <Spinner />}{isNew ? "Add contact" : "Save"}</Button>
      </div>
    </form>
  );
}
