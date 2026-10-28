import * as React from "react";
import { checkForUpdate, installUpdate, restartApp, type UpdateState } from "../../lib/updates";
import { IW_ENABLED } from "../../lib/features";
import * as D from "@radix-ui/react-dialog";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  X, SlidersHorizontal, BookOpen, PenLine, Users, Tag as TagIcon, Keyboard, Info, Plus, Trash2, Pencil, KeyRound, Download,
  CheckCircle2, AlertTriangle, Star, Link2Off, ChevronRight,
} from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { refreshAccounts, refreshFolders, setState, setUi, setView, updateSettings, useStore, getState } from "../../lib/store";
import type { Account, ApiError, Identity, Settings, Tag } from "../../lib/types";
import { cn } from "../../lib/utils";
import { Button } from "../../components/ui/button";
import { ConfirmDialog, Dialog } from "../../components/ui/dialog";
import { Field, Input, Kbd, NativeSelect, Switch, Textarea } from "../../components/ui/form";
import { Spinner } from "../../components/ui/misc";
import { ColorPicker } from "../accounts/ColorPicker";
import { ServerFields, validateServer, type ServerErrors } from "../accounts/ServerFields";
import { MessageBody } from "../reader/MessageBody";
import { SHORTCUTS } from "../shortcuts";

type NavItem = { id: string; label: string; icon: React.ComponentType<{ className?: string }>; color?: string; sub?: boolean };

export function SettingsDialog() {
  const section = useStore((s) => s.ui.settings);
  const accounts = useStore((s) => s.accounts);
  const open = typeof section === "string";
  const current = open ? section : "general";

  const nav: { group: string; items: NavItem[] }[] = [
    { group: "App", items: [
      { id: "general", label: "General", icon: SlidersHorizontal },
      { id: "reading", label: "Reading", icon: BookOpen },
      { id: "compose", label: "Writing", icon: PenLine },
      { id: "tags", label: "Tags", icon: TagIcon },
      { id: "shortcuts", label: "Keyboard shortcuts", icon: Keyboard },
    ] },
    { group: "Accounts", items: [
      { id: "accounts", label: "All accounts", icon: Users },
      ...accounts.map((a) => ({ id: `account:${a.id}`, label: a.name, icon: Users, color: a.color, sub: true })),
    ] },
    { group: IW_ENABLED ? "Connections" : "About", items: [
      ...(IW_ENABLED ? [{ id: "iw", label: "IntelligentWolf", icon: Link2Off }] : []),
      { id: "about", label: "About WolfMail", icon: Info },
    ] },
  ];
  const go = (id: string) => setUi({ settings: id });

  return (
    <D.Root open={open} onOpenChange={(o) => !o && setUi({ settings: false })}>
      <D.Portal>
        <D.Overlay className="fixed inset-0 z-50 bg-black/40 backdrop-blur-[2px]" />
        <D.Content aria-modal="true" className="fixed left-1/2 top-1/2 z-50 flex h-[min(720px,calc(100vh-48px))] w-[min(980px,calc(100vw-32px))] -translate-x-1/2 -translate-y-1/2 overflow-hidden rounded-card bg-popover text-foreground shadow-[var(--shadow-pop)] ring-1 ring-border focus:outline-none">
          <nav aria-label="Settings sections" className="flex w-60 shrink-0 flex-col border-r border-border bg-surface-inset">
            <D.Title className="px-5 pb-2 pt-5 font-title text-xl">Settings</D.Title>
            <D.Description className="sr-only">WolfMail settings</D.Description>
            <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-4">
              {nav.map((g) => (
                <div key={g.group} className="mt-3">
                  <div className="px-3 pb-1 text-[11px] font-semibold uppercase tracking-[0.08em] text-muted-foreground">{g.group}</div>
                  <ul role="list">
                    {g.items.map((it) => {
                      const active = current === it.id;
                      return (
                        <li key={it.id}>
                          <button type="button" onClick={() => go(it.id)} aria-current={active ? "page" : undefined}
                            className={cn("flex h-8 w-full items-center gap-2.5 rounded-lg px-3 text-left text-[13px] transition-colors",
                              it.sub && "pl-6", active ? "bg-select-strong font-semibold" : "hover:bg-accent")}>
                            {it.color ? <span aria-hidden className="size-2.5 shrink-0 rounded-full" style={{ background: it.color }} />
                              : <it.icon className={cn("size-4 shrink-0", active ? "text-[var(--iw-red-text)]" : "text-muted-foreground")} />}
                            <span className="truncate">{it.label}</span>
                          </button>
                        </li>
                      );
                    })}
                  </ul>
                </div>
              ))}
            </div>
          </nav>
          <div className="relative min-w-0 flex-1 overflow-y-auto">
            <D.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close settings" className="absolute right-3 top-3 z-10"><X /></Button>
            </D.Close>
            <div className="mx-auto max-w-[640px] px-8 pb-12 pt-6">
              <Section id={current} />
            </div>
          </div>
        </D.Content>
      </D.Portal>
    </D.Root>
  );
}

function Section({ id }: { id: string }) {
  if (id.startsWith("account:")) return <AccountPage accountId={id.slice(8)} />;
  switch (id) {
    case "reading": return <ReadingPage />;
    case "compose": return <ComposePage />;
    case "accounts": return <AccountsPage />;
    case "tags": return <TagsPage />;
    case "iw": return IW_ENABLED ? <IwPage /> : null;
    case "shortcuts": return <ShortcutsPage />;
    case "about": return <AboutPage />;
    default: return <GeneralPage />;
  }
}

function PageTitle({ children, sub }: { children: React.ReactNode; sub?: React.ReactNode }) {
  return (
    <header className="mb-6 pr-10">
      <h2 className="font-title text-[22px] leading-tight">{children}</h2>
      {sub && <p className="mt-1 text-[13px] text-muted-foreground">{sub}</p>}
    </header>
  );
}

function Card({ title, children, footer }: { title?: string; children: React.ReactNode; footer?: React.ReactNode }) {
  return (
    <section className="mb-5 rounded-card bg-surface ring-1 ring-border">
      {title && <h3 className="border-b border-border px-5 py-3 text-[13px] font-semibold">{title}</h3>}
      <div className="divide-y divide-border px-5 py-1.5">{children}</div>
      {footer && <div className="flex items-center justify-end gap-2 border-t border-border px-5 py-3">{footer}</div>}
    </section>
  );
}

function SelectRow<K extends keyof Settings>({ k, label, description, options }: { k: K; label: string; description?: string; options: { value: Settings[K]; label: string }[] }) {
  const settings = useStore((s) => s.settings);
  if (!settings) return null;
  const id = `set-${String(k)}`;
  return (
    <div className="flex items-center justify-between gap-6 py-2.5">
      <div className="min-w-0">
        <label htmlFor={id} className="block text-[13px] font-medium">{label}</label>
        {description && <p className="mt-0.5 text-xs text-muted-foreground">{description}</p>}
      </div>
      <NativeSelect id={id} className="w-52 shrink-0" value={String(settings[k])}
        onChange={(e) => {
          const opt = options.find((o) => String(o.value) === e.target.value);
          if (opt) void updateSettings({ [k]: opt.value } as Partial<Settings>);
        }}>
        {options.map((o) => <option key={String(o.value)} value={String(o.value)}>{o.label}</option>)}
      </NativeSelect>
    </div>
  );
}

function SwitchRow({ k, label, description }: { k: { [P in keyof Settings]: Settings[P] extends boolean ? P : never }[keyof Settings]; label: string; description?: string }) {
  const settings = useStore((s) => s.settings);
  if (!settings) return null;
  return <Switch id={`set-${k}`} checked={settings[k]} onCheckedChange={(v) => void updateSettings({ [k]: v } as Partial<Settings>)} label={label} description={description} />;
}

function GeneralPage() {
  return (
    <>
      <PageTitle sub="How WolfMail looks and behaves on this computer.">General</PageTitle>
      <Card title="Appearance">
        <SelectRow k="theme" label="Theme" options={[{ value: "system", label: "Match my system" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" }]} />
        <SelectRow k="density" label="Density" description="Compact fits more messages on screen." options={[{ value: "comfortable", label: "Comfortable" }, { value: "compact", label: "Compact" }]} />
      </Card>
      <Card title="Window and notifications">
        <SwitchRow k="closeToTray" label="Keep running in the tray when closed" description="New mail keeps arriving and scheduled messages still send." />
        <SwitchRow k="startInTray" label="Start in the tray" description="WolfMail opens hidden when your computer starts it." />
        <SwitchRow k="notifications" label="Desktop notifications for new mail" />
      </Card>
    </>
  );
}

function ReadingPage() {
  return (
    <>
      <PageTitle sub="The message list and reading pane.">Reading</PageTitle>
      <Card title="Message list">
        <SelectRow k="listLayout" label="List style" options={[{ value: "multiline", label: "Multi-line (with preview)" }, { value: "table", label: "Table (one line)" }]} />
        <SelectRow k="readingPane" label="Reading pane" options={[{ value: "right", label: "On the right" }, { value: "bottom", label: "Below the list" }, { value: "off", label: "Off" }]} />
        <SwitchRow k="threaded" label="Group messages into conversations" />
        <SelectRow k="markReadAfterMs" label="Mark as read" options={[
          { value: 0, label: "Straight away" }, { value: 2000, label: "After 2 seconds" }, { value: 5000, label: "After 5 seconds" }, { value: -1, label: "Only when I say so" },
        ]} />
      </Card>
      <Card title="Privacy">
        <SelectRow k="loadRemoteContent" label="Load remote images" description="Remote images tell senders when and where you opened their email. Tracking pixels are always removed."
          options={[{ value: "never", label: "Never automatically" }, { value: "contacts", label: "From people I've written to" }, { value: "always", label: "Always" }]} />
      </Card>
    </>
  );
}

function ComposePage() {
  const settings = useStore((s) => s.settings);
  return (
    <>
      <PageTitle sub="Signatures and reply style are set per identity — open an account under Accounts.">Writing</PageTitle>
      <Card title="Sending">
        <SelectRow k="undoSendSeconds" label="Undo send" description="How long you have to take a message back after pressing Send."
          options={[{ value: 0, label: "Off" }, { value: 5, label: "5 seconds" }, { value: 10, label: "10 seconds" }, { value: 20, label: "20 seconds" }, { value: 30, label: "30 seconds" }]} />
        {settings && settings.undoSendSeconds === 0 && <p className="py-2.5 text-xs text-muted-foreground">Messages go the moment you press Send.</p>}
      </Card>
    </>
  );
}

function AccountsPage() {
  const accounts = useStore((s) => s.accounts);
  const sync = useStore((s) => s.sync);
  return (
    <>
      <PageTitle sub="Every mailbox WolfMail checks. Drag order follows the sidebar.">Accounts</PageTitle>
      <div className="mb-5 flex gap-2">
        <Button onClick={() => setUi({ settings: false, addAccount: true })}><Plus /> Add account</Button>
        <Button variant="outline" onClick={() => setUi({ settings: false, importer: true })}><Download /> Import from Betterbird or Thunderbird</Button>
      </div>
      <ul role="list" className="space-y-2">
        {accounts.map((a, i) => {
          const st = sync[a.id]?.status ?? a.status;
          return (
            <li key={a.id} className="flex items-center gap-3 rounded-card bg-surface px-4 py-3 ring-1 ring-border">
              <span aria-hidden className="size-3 shrink-0 rounded-full" style={{ background: a.color }} />
              <button type="button" onClick={() => setUi({ settings: `account:${a.id}` })} className="min-w-0 flex-1 text-left">
                <span className="block truncate text-[13.5px] font-semibold">{a.name}</span>
                <span className="block truncate text-xs text-muted-foreground">{a.email} · {a.imap.host}</span>
              </button>
              <StatusPill status={st} enabled={a.enabled} />
              <div className="flex">
                <Button variant="ghost" size="iconSm" aria-label={`Move ${a.name} up`} disabled={i === 0} onClick={() => reorder(accounts, i, -1)}>↑</Button>
                <Button variant="ghost" size="iconSm" aria-label={`Move ${a.name} down`} disabled={i === accounts.length - 1} onClick={() => reorder(accounts, i, 1)}>↓</Button>
              </div>
              <ChevronRight className="size-4 text-muted-foreground" />
            </li>
          );
        })}
      </ul>
    </>
  );
}

async function reorder(accounts: Account[], i: number, d: number) {
  const ids = accounts.map((a) => a.id);
  const [x] = ids.splice(i, 1);
  ids.splice(i + d, 0, x);
  try { await api.accountsReorder(ids); await refreshAccounts(); }
  catch (e) { notify.error(e, "Couldn't reorder the accounts"); }
}

function StatusPill({ status, enabled }: { status: Account["status"]; enabled: boolean }) {
  if (!enabled || status === "disabled") return <span className="rounded-full px-2 py-0.5 text-[11px] font-semibold text-muted-foreground ring-1 ring-border">Off</span>;
  if (status === "authError") return <span className="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-semibold text-foreground ring-1 ring-[var(--iw-warn)]"><AlertTriangle className="size-3 text-[var(--iw-warn-text)]" /> Sign-in needed</span>;
  if (status === "error") return <span className="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-semibold text-[var(--iw-red-text)] ring-1 ring-[var(--iw-red)]/40"><AlertTriangle className="size-3" /> Error</span>;
  if (status === "offline") return <span className="rounded-full px-2 py-0.5 text-[11px] font-semibold text-muted-foreground ring-1 ring-border">Offline</span>;
  if (status === "syncing") return <span className="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-semibold ring-1 ring-border"><Spinner className="size-3 border-[1.5px]" /> Syncing</span>;
  return <span className="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-semibold text-[var(--iw-go-text)] ring-1 ring-[var(--iw-go-text)]/40"><CheckCircle2 className="size-3" /> Connected</span>;
}

const BODY_DAYS: { value: number; label: string }[] = [
  { value: 7, label: "Last 7 days" }, { value: 30, label: "Last 30 days" }, { value: 90, label: "Last 90 days" },
  { value: 365, label: "Last year" }, { value: -1, label: "Everything" }, { value: 0, label: "Headers only" },
];

function AccountPage({ accountId }: { accountId: string }) {
  const account = useStore((s) => s.accounts.find((a) => a.id === accountId));
  const status = useStore((s) => s.sync[accountId]?.status);
  const [name, setName] = React.useState("");
  const [color, setColor] = React.useState("#c41e1e");
  const [bodyDays, setBodyDays] = React.useState(30);
  const [nameErr, setNameErr] = React.useState<string>();
  const [saving, setSaving] = React.useState(false);
  const [removeOpen, setRemoveOpen] = React.useState(false);
  const [editing, setEditing] = React.useState<Partial<Identity> | null>(null);
  const [removingIdentity, setRemovingIdentity] = React.useState<Identity | null>(null);

  React.useEffect(() => {
    if (!account) return;
    setName(account.name); setColor(account.color); setBodyDays(account.bodyDays); setNameErr(undefined);
  }, [account?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!account) return <p className="text-[13px] text-muted-foreground">This account no longer exists.</p>;

  const dirty = name !== account.name || color !== account.color || bodyDays !== account.bodyDays;
  const save = async (e?: React.FormEvent) => {
    e?.preventDefault();
    if (!name.trim()) { setNameErr("Enter a name for this account"); document.getElementById("acc-name")?.focus(); return; }
    setNameErr(undefined);
    setSaving(true);
    try { await api.accountUpdate(account.id, { name: name.trim(), color, bodyDays }); await refreshAccounts(); notify.ok("Account saved"); }
    catch (ex) { notify.error(ex, "Couldn't save the account"); }
    finally { setSaving(false); }
  };

  const toggleEnabled = async (v: boolean) => {
    try { await api.accountUpdate(account.id, { enabled: v }); await Promise.all([refreshAccounts(), refreshFolders()]); notify.ok(v ? `${account.name} turned on` : `${account.name} turned off — its mail is kept but not checked`); }
    catch (ex) { notify.error(ex, "Couldn't change the account"); }
  };

  const remove = async () => {
    try {
      await api.accountRemove(account.id);
      const s = getState();
      if (s.view.kind === "folder" && s.folders.find((f) => f.id === (s.view as { folderId: string }).folderId)?.accountId === account.id) setView({ kind: "unified", unified: "inbox" });
      await Promise.all([refreshAccounts(), refreshFolders()]);
      setUi({ settings: "accounts" });
      notify.ok(`${account.name} removed from WolfMail`);
    } catch (ex) { notify.error(ex, "Couldn't remove the account"); }
  };

  return (
    <>
      <PageTitle sub={account.email}>
        <span className="inline-flex items-center gap-2.5"><span aria-hidden className="size-3 rounded-full" style={{ background: account.color }} />{account.name}</span>
      </PageTitle>

      <Card title="Account">
        <div className="py-2"><Switch id="acc-enabled" checked={account.enabled} onCheckedChange={toggleEnabled} label="Check this account for mail" description="Turning it off keeps everything but stops syncing and hides it from All inboxes." /></div>
        <form onSubmit={save} className="space-y-4 py-4" noValidate>
          <Field id="acc-name" label="Account name" error={nameErr}><Input value={name} onChange={(e) => setName(e.target.value)} /></Field>
          <ColorPicker name="acc-color" value={color} onChange={setColor} label="Account colour" />
          <Field id="acc-bodydays" label="Keep full messages on this computer" hint="Older messages download when you open them. Fewer days means less disk space.">
            <NativeSelect value={String(bodyDays)} onChange={(e) => setBodyDays(Number(e.target.value))}>
              {BODY_DAYS.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
            </NativeSelect>
          </Field>
          <div className="flex justify-end"><Button type="submit" disabled={!dirty || saving}>{saving && <Spinner />} Save changes</Button></div>
        </form>
      </Card>

      <Card title="Identities" footer={<Button variant="outline" size="sm" onClick={() => setEditing({ accountId: account.id, name: account.identities[0]?.name ?? "", email: "", signatureHtml: "", replyOnTop: true, isDefault: account.identities.length === 0 })}><Plus /> Add identity</Button>}>
        {account.identities.length === 0 && <p className="py-3 text-[13px] text-muted-foreground">No identities — add one to send from this account.</p>}
        {account.identities.map((i) => (
          <div key={i.id} className="flex items-center gap-3 py-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 text-[13px] font-medium">
                <span className="truncate">{i.name ? `${i.name} <${i.email}>` : i.email}</span>
                {i.isDefault && <span className="inline-flex items-center gap-1 rounded-full bg-surface-raised px-1.5 text-[10.5px] font-semibold ring-1 ring-border"><Star className="size-3" /> Default</span>}
              </div>
              <div className="text-xs text-muted-foreground">{i.replyOnTop ? "Replies above the quote" : "Replies below the quote"}{i.signatureHtml.trim() ? " · has a signature" : ""}{i.replyTo ? ` · replies go to ${i.replyTo}` : ""}</div>
            </div>
            <Button variant="ghost" size="iconSm" aria-label={`Edit ${i.email}`} onClick={() => setEditing(i)}><Pencil /></Button>
            <Button variant="ghost" size="iconSm" aria-label={`Remove ${i.email}`} disabled={account.identities.length <= 1} onClick={() => setRemovingIdentity(i)}><Trash2 /></Button>
          </div>
        ))}
      </Card>

      <ServersCard account={account} status={status ?? account.status} />

      <section className="rounded-card p-5 ring-1 ring-[var(--iw-red)]/40">
        <h3 className="text-[13px] font-semibold text-[var(--iw-red-text)]">Remove this account</h3>
        <p className="mt-1 text-xs text-muted-foreground">Deletes WolfMail's copy of this account's mail and its saved password from this computer. Nothing on the mail server is touched.</p>
        <Button variant="destructive" size="sm" className="mt-3" onClick={() => setRemoveOpen(true)}><Trash2 /> Remove {account.name}…</Button>
      </section>

      <ConfirmDialog open={removeOpen} onOpenChange={setRemoveOpen} title={`Remove ${account.name}?`} destructive confirmLabel="Remove account"
        body={<>WolfMail deletes its local copy of <strong>{account.email}</strong> and the saved password. Your mail stays on the server — add the account again to get it back.</>}
        onConfirm={remove} />
      <ConfirmDialog open={!!removingIdentity} onOpenChange={(o) => !o && setRemovingIdentity(null)} title={`Remove ${removingIdentity?.email ?? ""}?`} destructive confirmLabel="Remove identity"
        body="You won't be able to send as this address from WolfMail until you add it again."
        onConfirm={async () => {
          if (!removingIdentity) return;
          try { await api.identityRemove(removingIdentity.id); await refreshAccounts(); notify.ok("Identity removed"); }
          catch (e) { notify.error(e, "Couldn't remove the identity"); }
        }} />
      <IdentityDialog value={editing} onClose={() => setEditing(null)} />
    </>
  );
}

function IdentityDialog({ value, onClose }: { value: Partial<Identity> | null; onClose: () => void }) {
  const [v, setV] = React.useState<Partial<Identity>>({});
  const [errs, setErrs] = React.useState<{ email?: string; replyTo?: string }>({});
  const [busy, setBusy] = React.useState(false);
  React.useEffect(() => { if (value) { setV(value); setErrs({}); } }, [value]);
  const save = async (e?: React.FormEvent) => {
    e?.preventDefault();
    const er: typeof errs = {};
    const emailRe = /^[^\s@<>]+@[^\s@<>]+\.[^\s@<>]+$/;
    if (!v.email?.trim()) er.email = "Enter the email address to send from";
    else if (!emailRe.test(v.email.trim())) er.email = "Enter an email address in the right format, like name@example.com";
    if (v.replyTo?.trim() && !emailRe.test(v.replyTo.trim())) er.replyTo = "Enter a reply-to address in the right format, or leave it empty";
    setErrs(er);
    if (Object.keys(er).length) { document.getElementById(er.email ? "id-email" : "id-replyto")?.focus(); return; }
    setBusy(true);
    try {
      await api.identityUpsert({ ...v, accountId: v.accountId!, email: v.email!.trim(), replyTo: v.replyTo?.trim() || undefined, name: v.name?.trim() ?? "", signatureHtml: v.signatureHtml ?? "", replyOnTop: v.replyOnTop ?? true, isDefault: v.isDefault ?? false });
      await refreshAccounts();
      notify.ok("Identity saved");
      onClose();
    } catch (ex) {
      const err = ex as ApiError;
      if (err.kind === "invalid" && err.field === "email") setErrs({ email: err.message });
      else notify.error(ex, "Couldn't save the identity");
    } finally { setBusy(false); }
  };
  return (
    <Dialog open={!!value} onOpenChange={(o) => !o && onClose()} title={value?.id ? "Edit identity" : "Add identity"}
      description="An identity is a name and address you send as. Mail addressed to it is answered from it automatically."
      className="w-[min(680px,calc(100vw-32px))]"
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button onClick={() => save()} disabled={busy}>{busy && <Spinner />} Save identity</Button></>}>
      <form onSubmit={save} className="space-y-4" noValidate>
        <div className="grid grid-cols-2 gap-3">
          <Field id="id-name" label="Your name"><Input value={v.name ?? ""} onChange={(e) => setV({ ...v, name: e.target.value })} /></Field>
          <Field id="id-email" label="Email address" error={errs.email}><Input type="email" value={v.email ?? ""} onChange={(e) => setV({ ...v, email: e.target.value })} spellCheck={false} /></Field>
        </div>
        <Field id="id-replyto" label="Reply-to (optional)" hint="Replies go here instead of the address above." error={errs.replyTo}>
          <Input type="email" value={v.replyTo ?? ""} onChange={(e) => setV({ ...v, replyTo: e.target.value })} spellCheck={false} />
        </Field>
        <div className="rounded-table px-4 ring-1 ring-border">
          <Switch id="id-ontop" checked={v.replyOnTop ?? true} onCheckedChange={(x) => setV({ ...v, replyOnTop: x })} label="Write replies above the quoted message" />
          <Switch id="id-default" checked={v.isDefault ?? false} onCheckedChange={(x) => setV({ ...v, isDefault: x })} label="Default identity for this account" />
        </div>
        <div className="grid grid-cols-2 gap-3">
          <Field id="id-sig" label="Signature (HTML)" hint="Plain text works too.">
            <Textarea rows={8} value={v.signatureHtml ?? ""} onChange={(e) => setV({ ...v, signatureHtml: e.target.value })} spellCheck={false} className="font-mono text-xs" />
          </Field>
          <div>
            <div className="mb-1.5 text-[13px] font-medium">Preview</div>
            <div className="min-h-[164px] rounded-control bg-white p-2 ring-1 ring-border">
              {(v.signatureHtml ?? "").trim() ? <MessageBody html={v.signatureHtml} remote={false} dark={false} /> : <p className="p-2 text-xs text-[#6b6470]">No signature</p>}
            </div>
          </div>
        </div>
        <button type="submit" className="sr-only">Save identity</button>
      </form>
    </Dialog>
  );
}

function ServersCard({ account, status }: { account: Account; status: Account["status"] }) {
  const [imap, setImap] = React.useState(account.imap);
  const [smtp, setSmtp] = React.useState(account.smtp);
  const [password, setPassword] = React.useState("");
  const [errs, setErrs] = React.useState<{ imap: ServerErrors; smtp: ServerErrors; password?: string; test?: string }>({ imap: {}, smtp: {} });
  const [busy, setBusy] = React.useState<null | "save" | "oauth">(null);
  React.useEffect(() => { setImap(account.imap); setSmtp(account.smtp); setPassword(""); setErrs({ imap: {}, smtp: {} }); }, [account.id]); // eslint-disable-line react-hooks/exhaustive-deps
  const oauth = imap.auth !== "password";

  const save = async (e?: React.FormEvent) => {
    e?.preventDefault();
    const er = { imap: validateServer(imap, !oauth), smtp: validateServer(smtp, !oauth) };
    setErrs(er);
    if (Object.values(er.imap).some(Boolean) || Object.values(er.smtp).some(Boolean)) return;
    setBusy("save");
    try {
      const changedServers = JSON.stringify(imap) !== JSON.stringify(account.imap) || JSON.stringify(smtp) !== JSON.stringify(account.smtp);
      if (!oauth && (password || changedServers)) {
        // A changed server needs the password to test against; the stored one is in the keychain, not here.
        if (!password) { setErrs({ ...er, password: "Enter the password to check the new server settings" }); return; }
        const t = await api.accountTest(imap, smtp, password);
        if (!t.imapOk || !t.smtpOk) {
          setErrs({ ...er, test: [t.imapOk ? "" : `Incoming: ${t.imapError?.message ?? "failed"}`, t.smtpOk ? "" : `Outgoing: ${t.smtpError?.message ?? "failed"}`].filter(Boolean).join(" · ") });
          return;
        }
      }
      await api.accountUpdate(account.id, { imap, smtp }, password || undefined);
      await refreshAccounts();
      setPassword("");
      notify.ok("Server settings saved");
    } catch (ex) { notify.error(ex, "Couldn't save the server settings"); }
    finally { setBusy(null); }
  };

  const reauth = async () => {
    setBusy("oauth");
    try {
      await api.oauthAuthorize(imap.auth === "oauthGoogle" ? "google" : "microsoft", account.email);
      await api.syncNow(account.id);
      await refreshAccounts();
      notify.ok("Signed in again");
    } catch (ex) { notify.error(ex, "Couldn't sign in"); }
    finally { setBusy(null); }
  };

  return (
    <section className="mb-5 rounded-card bg-surface ring-1 ring-border">
      <h3 className="flex items-center gap-2 border-b border-border px-5 py-3 text-[13px] font-semibold">
        Servers and sign-in
        {status === "authError" && <span className="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-semibold ring-1 ring-[var(--iw-warn)]"><AlertTriangle className="size-3 text-[var(--iw-warn-text)]" /> Sign-in needed</span>}
      </h3>
      <form onSubmit={save} className="space-y-4 p-5" noValidate>
        {errs.test && <p role="alert" className="selectable rounded-lg bg-[var(--iw-red-wash)] px-3 py-2 text-[13px] text-[var(--iw-red-text)]">{errs.test}</p>}
        {oauth ? (
          <div className="flex items-center gap-3 rounded-table bg-surface-raised p-3 ring-1 ring-border">
            <KeyRound className="size-5 text-muted-foreground" />
            <p className="min-w-0 flex-1 text-[13px]">Signs in with {imap.auth === "oauthGoogle" ? "Google" : "Microsoft"} — WolfMail never sees the password.</p>
            <Button size="sm" variant="outline" onClick={reauth} disabled={!!busy}>{busy === "oauth" && <Spinner />} Sign in again</Button>
          </div>
        ) : (
          <Field id="srv-password" label={status === "authError" ? "Password (needed)" : "Change password"} hint="Leave empty to keep the saved one." error={errs.password}>
            <Input type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="current-password" />
          </Field>
        )}
        <ServerFields kind="imap" idPrefix="srv" value={imap} onChange={setImap} errors={errs.imap} />
        <ServerFields kind="smtp" idPrefix="srv" value={smtp} onChange={setSmtp} errors={errs.smtp} />
        <div className="flex justify-end"><Button type="submit" disabled={!!busy}>{busy === "save" && <Spinner />} Save server settings</Button></div>
      </form>
    </section>
  );
}

function TagsPage() {
  const tags = useStore((s) => s.tags);
  const [editing, setEditing] = React.useState<Partial<Tag> | null>(null);
  return (
    <>
      <PageTitle sub="Tags are shared with Thunderbird and Betterbird on the same mailbox. Press 1–9 on a message to toggle the first nine.">Tags</PageTitle>
      <Card footer={<Button variant="outline" size="sm" onClick={() => setEditing({ name: "", color: "#c41e1e" })}><Plus /> New tag</Button>}>
        {tags.length === 0 && <p className="py-3 text-[13px] text-muted-foreground">No tags yet.</p>}
        {tags.map((t, i) => (
          <div key={t.key} className="flex items-center gap-3 py-2.5">
            <span className="w-4 text-xs tabular-nums text-muted-foreground">{i < 9 ? i + 1 : ""}</span>
            <span className="rounded-full px-2.5 py-0.5 text-xs font-semibold" style={{ background: `${t.color}22`, color: t.color }}>{t.name}</span>
            <div className="flex-1" />
            <Button variant="ghost" size="iconSm" aria-label={`Edit tag ${t.name}`} onClick={() => setEditing(t)}><Pencil /></Button>
          </div>
        ))}
      </Card>
      <TagDialog value={editing} onClose={() => setEditing(null)} />
    </>
  );
}

function TagDialog({ value, onClose }: { value: Partial<Tag> | null; onClose: () => void }) {
  const [name, setName] = React.useState("");
  const [color, setColor] = React.useState("#c41e1e");
  const [err, setErr] = React.useState<string>();
  const [busy, setBusy] = React.useState(false);
  React.useEffect(() => { if (value) { setName(value.name ?? ""); setColor(value.color ?? "#c41e1e"); setErr(undefined); } }, [value]);
  const save = async (e?: React.FormEvent) => {
    e?.preventDefault();
    if (!name.trim()) { setErr("Enter a tag name"); return; }
    setBusy(true);
    try {
      await api.tagUpsert({ key: value?.key, name: name.trim(), color });
      setState({ tags: await api.tagsList() });
      notify.ok("Tag saved");
      onClose();
    } catch (ex) { notify.error(ex, "Couldn't save the tag"); }
    finally { setBusy(false); }
  };
  return (
    <Dialog open={!!value} onOpenChange={(o) => !o && onClose()} title={value?.key ? "Edit tag" : "New tag"}
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button onClick={() => save()} disabled={busy}>{busy && <Spinner />} Save tag</Button></>}>
      <form onSubmit={save} className="space-y-4" noValidate>
        <Field id="tag-name" label="Name" error={err}><Input autoFocus value={name} onChange={(e) => setName(e.target.value)} /></Field>
        <ColorPicker name="tag-color" value={color} onChange={setColor} label="Colour" />
      </form>
    </Dialog>
  );
}

function IwPage() {
  const iw = useStore((s) => s.iw);
  const settings = useStore((s) => s.settings);
  const [confirm, setConfirm] = React.useState(false);
  const [base, setBase] = React.useState("");
  const [baseErr, setBaseErr] = React.useState<string>();
  React.useEffect(() => { setBase(settings?.iwBaseUrl ?? ""); }, [settings?.iwBaseUrl]);
  const saveBase = (e?: React.FormEvent) => {
    e?.preventDefault();
    try { const u = new URL(base.trim()); if (u.protocol !== "https:" && u.hostname !== "localhost" && u.hostname !== "127.0.0.1") throw new Error(); }
    catch { setBaseErr("Enter an https:// address, like https://intelligentwolf.com/api"); return; }
    setBaseErr(undefined);
    void updateSettings({ iwBaseUrl: base.trim().replace(/\/+$/, "") }).then(() => notify.ok("Address saved"));
  };
  return (
    <>
      <PageTitle sub="Your CRM, Alex and booking links inside WolfMail. Mail only goes to IntelligentWolf when you press one of its buttons.">IntelligentWolf</PageTitle>
      {iw?.connected ? (
        <Card title="Connected">
          <div className="flex items-center gap-3 py-3">
            <CheckCircle2 className="size-5 text-[var(--iw-go-text)]" />
            <div className="min-w-0 flex-1">
              <div className="text-[13.5px] font-semibold">{iw.org?.name ?? "Workspace"}</div>
              <div className="text-xs text-muted-foreground">{iw.user ? `${iw.user.name} · ${iw.user.email}` : ""}</div>
            </div>
            <Button variant="outline" size="sm" onClick={() => setConfirm(true)}>Disconnect…</Button>
          </div>
        </Card>
      ) : (
        <Card>
          <div className="flex items-center gap-3 py-4">
            <img src="/icon.png" alt="" className="size-9 rounded-lg" />
            <p className="min-w-0 flex-1 text-[13px]">Not connected. WolfMail works fully without it.</p>
            <Button onClick={() => setUi({ settings: false, iwConnect: true })}>Connect</Button>
          </div>
        </Card>
      )}
      <details className="rounded-card ring-1 ring-border">
        <summary className="cursor-pointer select-none px-5 py-3 text-[13px] font-semibold">Advanced</summary>
        <form onSubmit={saveBase} className="space-y-3 px-5 pb-5" noValidate>
          <Field id="iw-base" label="IntelligentWolf API address" hint="Only change this if IntelligentWolf support asks you to." error={baseErr}>
            <Input value={base} onChange={(e) => setBase(e.target.value)} spellCheck={false} />
          </Field>
          <div className="flex justify-end"><Button type="submit" size="sm" variant="outline">Save address</Button></div>
        </form>
      </details>
      <ConfirmDialog open={confirm} onOpenChange={setConfirm} title="Disconnect IntelligentWolf?" confirmLabel="Disconnect" destructive
        body="WolfMail forgets its IntelligentWolf connection on this computer and revokes it on the server. Your mail isn't affected."
        onConfirm={async () => {
          try { await api.iwDisconnect(); setState({ iw: await api.iwStatus() }); notify.ok("Disconnected from IntelligentWolf"); }
          catch (e) { notify.error(e, "Couldn't disconnect"); }
        }} />
    </>
  );
}

function ShortcutsPage() {
  const groups = React.useMemo(() => {
    const m = new Map<string, typeof SHORTCUTS>();
    for (const s of SHORTCUTS) m.set(s.group, [...(m.get(s.group) ?? []), s]);
    return [...m.entries()];
  }, []);
  return (
    <>
      <PageTitle sub="WolfMail is built to be used from the keyboard. Press ? anywhere to see these.">Keyboard shortcuts</PageTitle>
      {groups.map(([g, list]) => (
        <Card key={g} title={g}>
          {list.map((s) => (
            <div key={s.keys + s.label} className="flex items-center justify-between gap-4 py-2">
              <span className="text-[13px]">{s.label}</span>
              <span className="flex gap-1">{s.keys.split(" ").map((k, i) => <Kbd key={i}>{k}</Kbd>)}</span>
            </div>
          ))}
        </Card>
      ))}
    </>
  );
}

function AboutPage() {
  const [version, setVersion] = React.useState<string>();
  const [upd, setUpd] = React.useState<UpdateState>({ kind: "idle" });
  React.useEffect(() => { getVersion().then(setVersion).catch(() => setVersion(undefined)); }, []);
  const runCheck = async () => { setUpd({ kind: "checking" }); setUpd(await checkForUpdate()); };
  const runInstall = async () => {
    if (upd.kind !== "available") return;
    try { await installUpdate(upd.update, setUpd); } catch (e) { setUpd({ kind: "error", message: e instanceof Error ? e.message : String(e) }); }
  };
  return (
    <>
      <PageTitle>About WolfMail</PageTitle>
      <div className="flex items-center gap-4 rounded-card bg-surface p-6 ring-1 ring-border">
        <img src="/icon.png" alt="" className="size-16 rounded-2xl" />
        <div>
          <div className="font-title text-2xl">WolfMail</div>
          <div className="text-[13px] text-muted-foreground">Version {version ?? "—"} · by IntelligentWolf Ltd</div>
          <Button variant="link" className="mt-1 h-auto px-0" onClick={() => openUrl("https://intelligentwolf.com").catch((e) => notify.error(e, "Couldn't open your browser"))}>intelligentwolf.com</Button>
        </div>
      </div>
      <section aria-label="Updates" className="mt-5 rounded-card bg-surface p-5 ring-1 ring-border">
        <div className="flex flex-wrap items-center gap-3">
          <div className="min-w-0 flex-1">
            <h3 className="text-[13px] font-semibold">Updates</h3>
            <p className="mt-0.5 text-xs text-muted-foreground" role="status" aria-live="polite">
              {upd.kind === "idle" && "WolfMail checks once a day and asks before installing anything."}
              {upd.kind === "checking" && "Checking…"}
              {upd.kind === "none" && "You're on the latest version."}
              {upd.kind === "available" && `Version ${upd.version} is available.`}
              {upd.kind === "downloading" && `Downloading ${upd.version}… ${upd.total ? Math.round((upd.done / upd.total) * 100) + "%" : ""}`}
              {upd.kind === "ready" && `Version ${upd.version} is installed — restart to use it.`}
            </p>
            {upd.kind === "error" && <p role="alert" className="mt-1 text-xs font-medium text-[var(--iw-red-text)]">Couldn't check for updates: {upd.message}</p>}
            {upd.kind === "available" && upd.notes && <p className="mt-2 whitespace-pre-wrap text-xs text-muted-foreground">{upd.notes}</p>}
          </div>
          {upd.kind === "available" ? <Button onClick={runInstall}>Download and install</Button>
            : upd.kind === "ready" ? <Button onClick={restartApp}>Restart now</Button>
            : <Button variant="outline" disabled={upd.kind === "checking" || upd.kind === "downloading"} onClick={runCheck}>Check for updates</Button>}
        </div>
      </section>
    </>
  );
}
