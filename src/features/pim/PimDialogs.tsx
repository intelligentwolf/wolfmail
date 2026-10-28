import * as React from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Server, CheckCircle2, AlertTriangle, CalendarDays, Contact as ContactIcon, HardDrive } from "lucide-react";
import { pimApi } from "../../lib/pimApi";
import { notify } from "../../lib/notify";
import { refreshPim, setPim, usePim } from "../../lib/pim";
import type { PimAccount, PimImportCandidate, PimImportItem, PimKind } from "../../lib/pimTypes";
import { cn } from "../../lib/utils";
import { Dialog } from "../../components/ui/dialog";
import { Button } from "../../components/ui/button";
import { Field, Input } from "../../components/ui/form";
import { Spinner } from "../../components/ui/misc";

/** Add a calendar & contacts account: Google, Microsoft 365/Outlook.com, or any CalDAV/CardDAV server. */
export function PimAccountDialog() {
  const open = usePim((s) => s.accountDialog);
  const [kind, setKind] = React.useState<PimKind>("google");
  const [email, setEmail] = React.useState("");
  const [url, setUrl] = React.useState("");
  const [username, setUsername] = React.useState("");
  const [password, setPassword] = React.useState("");
  const [name, setName] = React.useState("");
  const [errors, setErrors] = React.useState<Record<string, string>>({});
  const [busy, setBusy] = React.useState(false);
  const summary = React.useRef<HTMLDivElement>(null);

  React.useEffect(() => {
    if (open) { setErrors({}); setPassword(""); }
  }, [open]);
  const close = () => setPim({ accountDialog: false });

  const fail = (field: string, msg: string) => {
    setErrors({ [field]: msg });
    requestAnimationFrame(() => summary.current?.focus());
  };

  const submit = async () => {
    setErrors({});
    if (kind === "dav") {
      if (!/^https?:\/\//.test(url.trim())) return fail("url", "Enter the server address, starting with https://");
      if (!username.trim()) return fail("username", "Enter your user name");
      if (!password) return fail("password", "Enter your password or app password");
    } else if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email.trim())) {
      return fail("email", "Enter the email address you sign in with");
    }
    setBusy(true);
    try {
      if (kind !== "dav") await pimApi.oauthAuthorize(kind === "google" ? "google" : "microsoft", email.trim());
      const a = await pimApi.accountAdd({
        kind, name: name.trim() || (kind === "dav" ? new URL(url.trim()).hostname : email.trim()),
        email: kind === "dav" ? undefined : email.trim(), url: kind === "dav" ? url.trim() : undefined,
        username: kind === "dav" ? username.trim() : undefined, password: kind === "dav" ? password : undefined,
      });
      await refreshPim();
      notify.ok(a.status === "ok" ? `${a.name} added — calendars and contacts are syncing` : `${a.name} added`);
      close();
    } catch (e) {
      const err = e as { kind?: string; field?: string; message?: string };
      if (err.kind === "oauthConfig") fail("email", err.message ?? "Sign-in isn't set up in this build.");
      else if (err.field) fail(err.field, err.message ?? "Check this");
      else notify.error(e, "Couldn't add the account");
    } finally {
      setBusy(false);
    }
  };

  const kinds: { k: PimKind; title: string; sub: string; icon: React.ReactNode }[] = [
    { k: "google", title: "Google", sub: "Calendar + contacts (contacts read-only)", icon: <span className="font-title text-[15px] text-[#4285f4]">G</span> },
    { k: "microsoft", title: "Microsoft 365 / Outlook.com", sub: "Calendar + contacts", icon: <span className="grid size-4 grid-cols-2 gap-px"><i className="bg-[#f25022]" /><i className="bg-[#7fba00]" /><i className="bg-[#00a4ef]" /><i className="bg-[#ffb900]" /></span> },
    { k: "dav", title: "CalDAV / CardDAV server", sub: "Nextcloud, Fastmail, iCloud, Radicale…", icon: <Server className="size-4" /> },
  ];
  const errs = Object.entries(errors);

  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()} title="Add a calendar & contacts account"
      description="Calendars and contacts sync both ways. Mail accounts are added separately."
      footer={<><Button variant="outline" onClick={close}>Cancel</Button><Button onClick={submit} disabled={busy}>{busy && <Spinner />}{kind === "dav" ? "Connect" : "Sign in and add"}</Button></>}>
      <form className="space-y-4" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
        {errs.length > 0 && (
          <div ref={summary} tabIndex={-1} role="alert" className="rounded-lg bg-[var(--iw-red-wash)] p-3 text-[13px] ring-1 ring-[var(--iw-red)]/40">
            {errs.map(([k, v]) => <p key={k}><a href={`#pim-${k}`} className="underline">{v}</a></p>)}
          </div>
        )}
        <div role="radiogroup" aria-label="Account type" className="grid gap-2">
          {kinds.map((x) => (
            <button key={x.k} type="button" role="radio" aria-checked={kind === x.k} onClick={() => { setKind(x.k); setErrors({}); }}
              className={cn("flex items-center gap-3 rounded-table px-3 py-2.5 text-left ring-1 ring-inset", kind === x.k ? "bg-select ring-[var(--iw-red)]/50" : "ring-border hover:bg-accent")}>
              <span className="flex size-8 items-center justify-center rounded-lg bg-surface-raised ring-1 ring-border">{x.icon}</span>
              <span><span className="block text-[13.5px] font-medium">{x.title}</span><span className="block text-xs text-muted-foreground">{x.sub}</span></span>
            </button>
          ))}
        </div>
        {kind === "dav" ? (
          <>
            <Field id="pim-url" label="Server address" hint="Your provider's CalDAV/CardDAV address, or just its website — WolfMail finds the rest." error={errors.url}>
              <Input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://cloud.example.com" autoFocus />
            </Field>
            <div className="grid gap-3 sm:grid-cols-2">
              <Field id="pim-username" label="User name" error={errors.username}><Input value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="username" /></Field>
              <Field id="pim-password" label="Password" hint="Use an app password if your provider has them." error={errors.password}><Input type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="current-password" /></Field>
            </div>
          </>
        ) : (
          <Field id="pim-email" label="Email address" hint="Your browser opens to sign in and allow calendar & contacts access." error={errors.email}>
            <Input type="email" value={email} onChange={(e) => setEmail(e.target.value)} placeholder={kind === "google" ? "you@gmail.com" : "you@outlook.com"} autoFocus />
          </Field>
        )}
        <Field id="pim-name" label="Name in WolfMail (optional)"><Input value={name} onChange={(e) => setName(e.target.value)} placeholder={kind === "dav" ? "Work calendars" : "Personal"} /></Field>
      </form>
    </Dialog>
  );
}

const kindLabel: Record<string, string> = {
  "google-calendar": "Google calendar", "google-contacts": "Google contacts", caldav: "CalDAV calendar", carddav: "CardDAV address book", "local-calendar": "On this computer",
};

/** Bring calendars and address books over from Betterbird/Thunderbird. */
export function PimImportDialog() {
  const open = usePim((s) => s.importDialog);
  const [cands, setCands] = React.useState<PimImportCandidate[] | null>(null);
  const [error, setError] = React.useState<string>();
  const [picked, setPicked] = React.useState<Set<string>>(new Set());
  const [busy, setBusy] = React.useState(false);
  const [result, setResult] = React.useState<PimAccount[] | null>(null);
  const key = (p: string, i: PimImportItem) => `${p}|${i.kind}|${i.url ?? i.name}`;

  const scan = React.useCallback(async (path?: string) => {
    setCands(null); setError(undefined); setResult(null);
    try {
      const c = await pimApi.importScan(path);
      setCands(c);
      setPicked(new Set(c.flatMap((x) => x.items.filter((i) => i.enabled).map((i) => key(x.profilePath, i)))));
    } catch (e) {
      setError((e as { message?: string }).message ?? String(e));
      setCands([]);
    }
  }, []);
  React.useEffect(() => { if (open) void scan(); }, [open, scan]);
  const close = () => setPim({ importDialog: false });

  const apply = async () => {
    if (!cands) return;
    setBusy(true);
    try {
      const out: PimAccount[] = [];
      for (const c of cands) {
        const items = c.items.filter((i) => picked.has(key(c.profilePath, i)));
        if (items.length) out.push(...(await pimApi.importApply(c.profilePath, items)));
      }
      await refreshPim();
      setResult(out.filter((a) => a.kind !== "local"));
      notify.ok("Imported calendar and contact settings");
    } catch (e) {
      notify.error(e, "Import failed");
    } finally {
      setBusy(false);
    }
  };

  const total = cands?.reduce((n, c) => n + c.items.length, 0) ?? 0;
  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()} title="Import calendars & contacts" className="w-[min(640px,calc(100vw-32px))]"
      description="From Betterbird or Thunderbird. Passwords are never copied — you sign in once per account afterwards."
      footer={result ? <Button onClick={close}>Done</Button> : (
        <>
          <Button variant="ghost" className="mr-auto" onClick={async () => { const p = await openDialog({ directory: true, title: "Choose a Thunderbird/Betterbird profile folder" }); if (p) void scan(String(p)); }}>Choose another profile…</Button>
          <Button variant="outline" onClick={close}>Cancel</Button>
          <Button onClick={apply} disabled={busy || !picked.size}>{busy && <Spinner />}Import {picked.size || ""}</Button>
        </>
      )}>
      {result ? (
        <div className="space-y-3">
          <p className="flex items-center gap-2 text-[14px] font-medium"><CheckCircle2 className="size-5 text-[var(--iw-go-text)]" /> Imported.</p>
          {result.length > 0 && <p className="text-[13px] text-muted-foreground">Finish each account below — open Calendar and choose <b>Fix</b> next to it:</p>}
          <ul className="space-y-1.5">
            {result.map((a) => (
              <li key={a.id} className="flex items-center gap-2 rounded-lg bg-surface-raised px-3 py-2 text-[13px] ring-1 ring-border">
                {a.status === "ok" ? <CheckCircle2 className="size-4 text-[var(--iw-go-text)]" /> : <AlertTriangle className="size-4 text-[var(--iw-warn-text)]" />}
                <span className="font-medium">{a.name}</span>
                <span className="truncate text-muted-foreground">{a.status === "ok" ? "syncing" : a.statusMessage}</span>
              </li>
            ))}
          </ul>
        </div>
      ) : cands === null ? (
        <div className="flex justify-center py-10"><Spinner /></div>
      ) : error ? (
        <p role="alert" className="rounded-lg bg-[var(--iw-red-wash)] p-3 text-[13px] text-[var(--iw-red-text)]">{error}</p>
      ) : total === 0 ? (
        <p className="py-6 text-center text-[13px] text-muted-foreground">No Betterbird or Thunderbird calendars were found. Choose the profile folder if it's somewhere unusual.</p>
      ) : (
        <div className="space-y-4">
          {cands.map((c) => (
            <section key={c.profilePath}>
              <h3 className="mb-1.5 truncate text-[11px] font-semibold uppercase tracking-wide text-muted-foreground" title={c.profilePath}>{c.profilePath}</h3>
              <ul className="divide-y divide-border overflow-hidden rounded-table ring-1 ring-border">
                {c.items.map((i) => {
                  const k = key(c.profilePath, i);
                  const on = picked.has(k);
                  const Icon = i.kind.includes("contacts") || i.kind === "carddav" ? ContactIcon : i.kind === "local-calendar" ? HardDrive : CalendarDays;
                  return (
                    <li key={k}>
                      <label className="flex cursor-pointer items-center gap-3 px-3 py-2.5 hover:bg-accent/60">
                        <input type="checkbox" checked={on} onChange={() => setPicked((s) => { const n = new Set(s); if (on) n.delete(k); else n.add(k); return n; })} className="size-4 accent-[var(--iw-red)]" />
                        <span className="size-3 shrink-0 rounded-full" style={{ background: i.color ?? "var(--muted-foreground)" }} />
                        <Icon className="size-4 shrink-0 text-muted-foreground" />
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-[13.5px] font-medium">{i.name}</span>
                          <span className="block truncate text-xs text-muted-foreground">
                            {kindLabel[i.kind] ?? i.kind}{i.username ? ` · ${i.username}` : ""}{i.kind === "local-calendar" ? ` · ${i.localEvents} events` : ""}{!i.enabled ? " · hidden in Betterbird" : ""}
                          </span>
                        </span>
                      </label>
                    </li>
                  );
                })}
              </ul>
            </section>
          ))}
        </div>
      )}
    </Dialog>
  );
}
