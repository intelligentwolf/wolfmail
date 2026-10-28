import * as React from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { CheckCircle2, FolderOpen, HardDrive, KeyRound, Lock, PackageOpen, RefreshCw, ShieldCheck } from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { refreshAccounts, refreshFolders, setUi, useStore } from "../../lib/store";
import type { Account, ApiError, ImportCandidate, Provider } from "../../lib/types";
import { bytes, cn } from "../../lib/utils";
import { Button } from "../../components/ui/button";
import { Dialog } from "../../components/ui/dialog";
import { Field, Input } from "../../components/ui/form";
import { EmptyState, Skeleton, Spinner } from "../../components/ui/misc";

const providerBadge: Record<Provider, string> = { imap: "IMAP", gmail: "Google", outlook: "Microsoft" };

export function ImportDialog() {
  const open = useStore((s) => s.ui.importer);
  const existing = useStore((s) => s.accounts);
  const [step, setStep] = React.useState<"pick" | "finish">("pick");
  const [cands, setCands] = React.useState<ImportCandidate[] | null>(null);
  const [scanError, setScanError] = React.useState<string>();
  const [checked, setChecked] = React.useState<Record<string, boolean>>({});
  const [busy, setBusy] = React.useState(false);
  const [imported, setImported] = React.useState<Account[]>([]);
  const [pickError, setPickError] = React.useState<string>();

  const scan = React.useCallback(async (profilePath?: string) => {
    setCands(null);
    setScanError(undefined);
    try {
      const r = await api.importScan(profilePath);
      setCands(r);
      const c: Record<string, boolean> = {};
      const have = new Set(existing.map((a) => a.email.toLowerCase()));
      for (const p of r) for (const a of p.accounts) c[`${p.profilePath}|${a.email}`] = !have.has(a.email.toLowerCase());
      setChecked(c);
    } catch (e) {
      setScanError((e as ApiError).message ?? String(e));
      setCands([]);
    }
  }, [existing]);

  React.useEffect(() => {
    if (!open) return;
    setStep("pick");
    setImported([]);
    setPickError(undefined);
    void scan();
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  const close = () => setUi({ importer: false });

  const chooseFolder = async () => {
    const dir = await openDialog({ directory: true, title: "Choose a Betterbird or Thunderbird profile folder" });
    if (typeof dir === "string") void scan(dir);
  };

  const doImport = async () => {
    if (!cands) return;
    const byProfile = new Map<string, string[]>();
    for (const [k, on] of Object.entries(checked)) {
      if (!on) continue;
      const i = k.lastIndexOf("|");
      const p = k.slice(0, i);
      byProfile.set(p, [...(byProfile.get(p) ?? []), k.slice(i + 1)]);
    }
    if (byProfile.size === 0) { setPickError("Choose at least one account to import"); return; }
    setPickError(undefined);
    setBusy(true);
    try {
      const all: Account[] = [];
      for (const [profilePath, emails] of byProfile) all.push(...(await api.importApply(profilePath, emails)));
      await Promise.all([refreshAccounts(), refreshFolders()]);
      setImported(all);
      setStep("finish");
      notify.ok(`Imported ${all.length} ${all.length === 1 ? "account" : "accounts"}`);
    } catch (e) {
      notify.error(e, "Import failed");
    } finally {
      setBusy(false);
    }
  };

  const total = Object.values(checked).filter(Boolean).length;

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => !o && close()}
      title={step === "pick" ? "Import from Betterbird or Thunderbird" : "Sign in to your imported accounts"}
      description={step === "pick"
        ? "WolfMail copies your accounts, identities, signatures, sending servers and tags. Your mail stays on the server and syncs fresh."
        : "Passwords are never copied from Betterbird — WolfMail doesn't read its password store. Enter each one once; it goes into your system keychain."}
      className="w-[min(720px,calc(100vw-32px))]"
      footer={step === "pick" ? (
        <>
          <Button variant="ghost" onClick={chooseFolder}><FolderOpen /> Choose another profile folder</Button>
          <div className="flex-1" />
          <Button variant="outline" onClick={close}>Cancel</Button>
          <Button onClick={doImport} disabled={busy || !cands || cands.length === 0}>{busy && <Spinner />} Import {total > 0 ? total : ""} {total === 1 ? "account" : "accounts"}</Button>
        </>
      ) : (
        <Button onClick={close}>Done</Button>
      )}
    >
      {step === "pick" ? (
        cands === null ? (
          <div aria-hidden className="space-y-3">{[0, 1, 2].map((i) => <Skeleton key={i} className="h-14 rounded-table" />)}<span className="sr-only">Looking for profiles</span></div>
        ) : cands.length === 0 ? (
          <EmptyState
            tone={scanError ? "error" : "default"}
            icon={<PackageOpen className="size-6" />}
            title={scanError ? "Couldn't read that profile" : "No Betterbird or Thunderbird profile found"}
            body={scanError ?? "WolfMail looked in the usual places, including Flatpak installs. If yours is somewhere else, choose its folder."}
            action={<div className="flex gap-2"><Button variant="outline" onClick={() => scan()}><RefreshCw /> Look again</Button><Button onClick={chooseFolder}><FolderOpen /> Choose folder</Button></div>}
          />
        ) : (
          <div className="space-y-5">
            {pickError && <p role="alert" className="rounded-lg bg-[var(--iw-red-wash)] px-3 py-2 text-[13px] font-medium text-[var(--iw-red-text)]">{pickError}</p>}
            {cands.map((p) => {
              const keys = p.accounts.map((a) => `${p.profilePath}|${a.email}`);
              const allOn = keys.every((k) => checked[k]);
              const size = p.accounts.reduce((n, a) => n + a.localSizeBytes, 0);
              return (
                <section key={p.profilePath} aria-label={`${p.source} profile`} className="rounded-card ring-1 ring-border">
                  <header className="flex items-center gap-3 border-b border-border px-4 py-3">
                    <HardDrive className="size-5 text-[var(--iw-red-text)]" />
                    <div className="min-w-0 flex-1">
                      <div className="text-[13.5px] font-semibold capitalize">{p.source}</div>
                      <div className="selectable truncate text-xs text-muted-foreground" title={p.profilePath}>{p.profilePath}</div>
                    </div>
                    <span className="text-xs tabular-nums text-muted-foreground">{p.accounts.length} accounts · {bytes(size)}</span>
                    <Button size="sm" variant="ghost" onClick={() => setChecked((c) => { const n = { ...c }; for (const k of keys) n[k] = !allOn; return n; })}>
                      {allOn ? "Select none" : "Select all"}
                    </Button>
                  </header>
                  <ul role="list" className="divide-y divide-border">
                    {p.accounts.map((a) => {
                      const k = `${p.profilePath}|${a.email}`;
                      const dup = existing.some((x) => x.email.toLowerCase() === a.email.toLowerCase());
                      const id = `imp-${k.replace(/[^a-z0-9]/gi, "-")}`;
                      return (
                        <li key={k} className={cn("flex items-center gap-3 px-4 py-2.5", dup && "opacity-60")}>
                          <input id={id} type="checkbox" disabled={dup} checked={!!checked[k]} onChange={(e) => setChecked((c) => ({ ...c, [k]: e.target.checked }))}
                            className="size-4 accent-[var(--iw-red)]" />
                          <label htmlFor={id} className="min-w-0 flex-1 cursor-pointer">
                            <span className="block truncate text-[13px] font-medium">{a.name}</span>
                            <span className="block truncate text-xs text-muted-foreground">
                              {a.email} · {a.imap.host}{a.identities.length > 1 ? ` · ${a.identities.length} identities` : ""}{dup ? " · already in WolfMail" : ""}
                            </span>
                          </label>
                          <span className="rounded-full bg-surface-raised px-2 py-0.5 text-[11px] font-semibold ring-1 ring-border">{providerBadge[a.provider]}</span>
                          <span className="w-24 text-right text-xs tabular-nums text-muted-foreground">{a.folderCount} folders<br />{bytes(a.localSizeBytes)}</span>
                        </li>
                      );
                    })}
                  </ul>
                  {p.tags.length > 0 && (
                    <div className="flex flex-wrap items-center gap-1.5 border-t border-border px-4 py-2.5 text-xs text-muted-foreground">
                      Tags:
                      {p.tags.map((t) => <span key={t.key} className="rounded-full px-2 py-0.5 font-semibold" style={{ background: `${t.color}22`, color: t.color }}>{t.name}</span>)}
                    </div>
                  )}
                </section>
              );
            })}
          </div>
        )
      ) : (
        <FinishList accounts={imported} />
      )}
    </Dialog>
  );
}

function FinishList({ accounts }: { accounts: Account[] }) {
  const live = useStore((s) => s.accounts);
  if (!accounts.length) return <p className="text-[13px] text-muted-foreground">No accounts were imported.</p>;
  return (
    <ul role="list" className="space-y-2.5">
      {accounts.map((a) => <FinishRow key={a.id} account={live.find((x) => x.id === a.id) ?? a} />)}
    </ul>
  );
}

function FinishRow({ account }: { account: Account }) {
  const [pw, setPw] = React.useState("");
  const [err, setErr] = React.useState<string>();
  const [busy, setBusy] = React.useState(false);
  const [done, setDone] = React.useState(account.status !== "authError");
  const oauth = account.imap.auth !== "password";
  const id = `fin-${account.id}`;

  const save = async (e?: React.FormEvent) => {
    e?.preventDefault();
    if (!pw) { setErr("Enter the password for this mailbox"); return; }
    setBusy(true);
    setErr(undefined);
    try {
      const t = await api.accountTest(account.imap, account.smtp, pw);
      if (!t.imapOk) { setErr(t.imapError?.message ?? "The server didn't accept this password"); return; }
      await api.accountUpdate(account.id, {}, pw);
      await refreshAccounts();
      setDone(true);
      setPw("");
      notify.ok(`${account.name} is connected`);
    } catch (ex) {
      setErr((ex as ApiError).message ?? String(ex));
    } finally {
      setBusy(false);
    }
  };

  const signIn = async () => {
    setBusy(true);
    setErr(undefined);
    try {
      await api.oauthAuthorize(account.imap.auth === "oauthGoogle" ? "google" : "microsoft", account.email);
      await api.syncNow(account.id);
      await refreshAccounts();
      setDone(true);
      notify.ok(`${account.name} is connected`);
    } catch (ex) {
      const e2 = ex as ApiError;
      if (e2.kind === "oauthConfig") setErr("Sign-in with this provider isn't available in this build yet. Open the account in Settings → Accounts and switch it to an app password.");
      else if (e2.kind !== "cancelled") setErr(e2.message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <li className="rounded-card p-4 ring-1 ring-border">
      <div className="flex items-center gap-3">
        <span aria-hidden className="size-3 rounded-full" style={{ background: account.color }} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-[13.5px] font-semibold">{account.name}</div>
          <div className="truncate text-xs text-muted-foreground">{account.email}</div>
        </div>
        {done && <span role="status" className="flex items-center gap-1.5 text-xs font-semibold text-[var(--iw-go-text)]"><CheckCircle2 className="size-4" /> Connected</span>}
        {!done && oauth && (
          <Button size="sm" onClick={signIn} disabled={busy}>{busy ? <Spinner /> : <Lock />} Sign in with {account.imap.auth === "oauthGoogle" ? "Google" : "Microsoft"}</Button>
        )}
      </div>
      {!done && !oauth && (
        <form onSubmit={save} className="mt-3 flex items-end gap-2" noValidate>
          <Field id={id} label={`Password for ${account.email}`} error={err} className="flex-1">
            <Input type="password" value={pw} onChange={(e) => setPw(e.target.value)} autoComplete="current-password" />
          </Field>
          <Button type="submit" disabled={busy}>{busy ? <Spinner /> : <KeyRound />} Save</Button>
        </form>
      )}
      {!done && oauth && err && <p role="alert" className="mt-2 text-xs font-medium text-[var(--iw-red-text)]">{err}</p>}
      {done && !oauth && <p className="mt-1.5 flex items-center gap-1.5 text-xs text-muted-foreground"><ShieldCheck className="size-3.5" /> Password stored in your system keychain.</p>}
    </li>
  );
}
