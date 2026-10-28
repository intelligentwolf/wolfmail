import * as React from "react";
import { secretsPlace } from "../../lib/features";
import { ArrowLeft, Building2, CheckCircle2, ChevronRight, KeyRound, Lock, Mail, Server, ShieldCheck } from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { refreshAccounts, refreshFolders, setUi, useStore } from "../../lib/store";
import type { ApiError, Autodetected, IwBusinessMailbox, Provider, ServerConfig } from "../../lib/types";
import { cn, isEmail } from "../../lib/utils";
import { Button } from "../../components/ui/button";
import { Dialog } from "../../components/ui/dialog";
import { Field, Input } from "../../components/ui/form";
import { Spinner } from "../../components/ui/misc";
import { ColorPicker, ACCOUNT_COLORS } from "./ColorPicker";
import { ServerFields, validateServer, type ServerErrors } from "./ServerFields";

type Step = "identify" | "configure";
type Errors = {
  name?: string; email?: string; password?: string; oauth?: string;
  imap: ServerErrors; smtp: ServerErrors;
  imapTest?: string; smtpTest?: string;
};
const noErrors: Errors = { imap: {}, smtp: {} };

const providerLabel: Record<Provider, string> = { imap: "IMAP mail server", gmail: "Google (Gmail / Workspace)", outlook: "Microsoft (Outlook / Microsoft 365)" };

export function AddAccountDialog() {
  const secretsBackend = useStore((s) => s.secrets?.backend);
  const open = useStore((s) => s.ui.addAccount);
  const accounts = useStore((s) => s.accounts);
  const iwConnected = useStore((s) => !!s.iw?.connected);

  const [step, setStep] = React.useState<Step>("identify");
  const [name, setName] = React.useState("");
  const [email, setEmail] = React.useState("");
  const [password, setPassword] = React.useState("");
  const [yourName, setYourName] = React.useState("");
  const [color, setColor] = React.useState(ACCOUNT_COLORS[0].hex);
  const [provider, setProvider] = React.useState<Provider>("imap");
  const [imap, setImap] = React.useState<ServerConfig>({ host: "", port: 993, security: "tls", username: "", auth: "password" });
  const [smtp, setSmtp] = React.useState<ServerConfig>({ host: "", port: 465, security: "tls", username: "", auth: "password" });
  const [source, setSource] = React.useState<Autodetected["source"] | null>(null);
  const [showServers, setShowServers] = React.useState(false);
  const [busy, setBusy] = React.useState<null | "detect" | "oauth" | "test">(null);
  const [errors, setErrors] = React.useState<Errors>(noErrors);
  const [oauthDone, setOauthDone] = React.useState(false);
  const [oauthUnavailable, setOauthUnavailable] = React.useState(false);
  const [business, setBusiness] = React.useState<IwBusinessMailbox[] | null>(null);
  const summaryRef = React.useRef<HTMLDivElement>(null);

  // Reset whenever the dialog opens; pick the first colour no account uses yet.
  React.useEffect(() => {
    if (!open) return;
    const used = new Set(accounts.map((a) => a.color.toLowerCase()));
    setStep("identify"); setName(""); setEmail(""); setPassword(""); setYourName(accounts[0]?.identities[0]?.name ?? ""); setProvider("imap");
    setColor((ACCOUNT_COLORS.find((c) => !used.has(c.hex)) ?? ACCOUNT_COLORS[0]).hex);
    setSource(null); setShowServers(false); setBusy(null); setErrors(noErrors); setOauthDone(false); setOauthUnavailable(false);
    setBusiness(null);
    if (iwConnected) api.iwBusinessMailboxes().then(setBusiness).catch(() => setBusiness([]));
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  const close = () => setUi({ addAccount: false });
  const isOauth = imap.auth !== "password";

  const summaryItems = (e: Errors): { id: string; text: string }[] => {
    const out: { id: string; text: string }[] = [];
    if (e.name) out.push({ id: "acct-name", text: e.name });
    if (e.email) out.push({ id: "acct-email", text: e.email });
    if (e.password) out.push({ id: "acct-password", text: e.password });
    if (e.oauth) out.push({ id: "acct-oauth", text: e.oauth });
    if (e.imapTest) out.push({ id: "acct-imap-host", text: `Incoming server: ${e.imapTest}` });
    if (e.smtpTest) out.push({ id: "acct-smtp-host", text: `Outgoing server: ${e.smtpTest}` });
    for (const [k, v] of Object.entries(e.imap)) if (v) out.push({ id: `acct-imap-${k === "username" ? "user" : k}`, text: `Incoming ${v.charAt(0).toLowerCase()}${v.slice(1)}` });
    for (const [k, v] of Object.entries(e.smtp)) if (v) out.push({ id: `acct-smtp-${k === "username" ? "user" : k}`, text: `Outgoing ${v.charAt(0).toLowerCase()}${v.slice(1)}` });
    return out;
  };
  const showErrors = (e: Errors) => {
    setErrors(e);
    if (summaryItems(e).length) {
      // Error summary receives focus (GOV.UK validation pattern).
      setTimeout(() => summaryRef.current?.focus(), 0);
      if (Object.values(e.imap).some(Boolean) || Object.values(e.smtp).some(Boolean) || e.imapTest || e.smtpTest) setShowServers(true);
    }
  };

  const applyDetected = (d: Autodetected, user: string) => {
    setProvider(d.provider);
    setImap({ ...d.imap, username: d.imap.username || user });
    setSmtp({ ...d.smtp, username: d.smtp.username || user });
    setSource(d.source);
  };

  const continueIdentify = async (e?: React.FormEvent, forced?: Provider) => {
    e?.preventDefault();
    const err: Errors = { imap: {}, smtp: {} };
    if (!email.trim()) err.email = "Enter your email address";
    else if (!isEmail(email)) err.email = "Enter an email address in the right format, like name@example.com";
    else if (accounts.some((a) => a.email.toLowerCase() === email.trim().toLowerCase())) err.email = "This address is already set up in WolfMail";
    if (summaryItems(err).length) { showErrors(err); return; }
    setErrors(noErrors);
    setBusy("detect");
    try {
      const d = await api.accountAutodetect(email.trim());
      applyDetected(d, email.trim());
      if (forced && forced !== d.provider) {
        // The person pressed "Google" or "Microsoft" but the domain didn't resolve to it (e.g. a custom domain on
        // Workspace without Google MX visible). Trust the button for the auth method; keep the detected servers.
        setProvider(forced);
        const auth = forced === "gmail" ? "oauthGoogle" : "oauthMicrosoft";
        setImap((x) => ({ ...x, auth }));
        setSmtp((x) => ({ ...x, auth }));
      }
      setStep("configure");
    } catch (ex) {
      // Autodetect failing is not fatal: let the person type the servers in.
      const domain = email.trim().split("@")[1] ?? "";
      setImap({ host: "", port: 993, security: "tls", username: email.trim(), auth: "password" });
      setSmtp({ host: "", port: 465, security: "tls", username: email.trim(), auth: "password" });
      setProvider("imap");
      setSource(null);
      setShowServers(true);
      setStep("configure");
      notify.info(`Couldn't find settings for ${domain} automatically — enter the server details from your email provider below. (${(ex as ApiError).message ?? ""})`);
    } finally {
      setBusy(null);
    }
  };

  const signIn = async () => {
    const prov = imap.auth === "oauthGoogle" ? "google" : "microsoft";
    setBusy("oauth");
    setErrors((x) => ({ ...x, oauth: undefined }));
    try {
      await api.oauthAuthorize(prov, email.trim());
      setOauthDone(true);
      notify.ok(`Signed in to ${prov === "google" ? "Google" : "Microsoft"}`);
    } catch (ex) {
      const err = ex as ApiError;
      if (err.kind === "oauthConfig") {
        setOauthUnavailable(true);
      } else if (err.kind !== "cancelled") {
        showErrors({ ...noErrors, oauth: err.message });
      }
    } finally {
      setBusy(null);
    }
  };

  const useAppPassword = () => {
    setImap((s) => ({ ...s, auth: "password" }));
    setSmtp((s) => ({ ...s, auth: "password" }));
    setOauthUnavailable(false);
    setErrors(noErrors);
  };

  const testAndAdd = async (e?: React.FormEvent) => {
    e?.preventDefault();
    const err: Errors = { imap: validateServer(imap, !isOauth), smtp: validateServer(smtp, !isOauth) };
    if (!name.trim()) err.name = "Enter a name for this account, like Work or Garage Master";
    if (!isOauth && !password) err.password = "Enter the password for this mailbox";
    if (isOauth && !oauthDone) err.oauth = `Sign in with ${imap.auth === "oauthGoogle" ? "Google" : "Microsoft"} first`;
    if (summaryItems(err).length) { showErrors(err); return; }
    setErrors(noErrors);
    setBusy("test");
    try {
      const t = await api.accountTest(imap, smtp, isOauth ? undefined : password);
      if (!t.imapOk || !t.smtpOk) {
        showErrors({
          ...noErrors,
          imapTest: t.imapOk ? undefined : t.imapError?.message ?? "WolfMail couldn't sign in.",
          smtpTest: t.smtpOk ? undefined : t.smtpError?.message ?? "WolfMail couldn't sign in.",
          password: !isOauth && (t.imapError?.kind === "auth" || t.smtpError?.kind === "auth") ? "The server didn't accept this password" : undefined,
        });
        return;
      }
      await api.accountAdd({
        name: name.trim(), email: email.trim(), displayName: yourName.trim(), provider, imap, smtp,
        password: isOauth ? undefined : password, color,
      });
      await Promise.all([refreshAccounts(), refreshFolders()]);
      notify.ok(`${name.trim()} added — your mail is syncing`);
      close();
    } catch (ex) {
      const err2 = ex as ApiError;
      if (err2.kind === "invalid" && err2.field) {
        const f = err2.field;
        showErrors({ ...noErrors, ...(f === "name" ? { name: err2.message } : f === "email" ? { email: err2.message } : { imapTest: err2.message }) });
      } else {
        notify.error(ex, "Couldn't add the account");
      }
    } finally {
      setBusy(null);
    }
  };

  const prefillBusiness = (b: IwBusinessMailbox) => {
    setEmail(b.email);
    if (!name) setName(b.email.split("@")[1] ?? b.email);
    // Hosts/ports come from IntelligentWolf (GET /wolfmail/business-mailboxes); security follows the standard port
    // meaning (RFC 8314: 993/465 implicit TLS, 143/587 STARTTLS).
    setImap({ host: b.imapHost, port: b.imapPort, security: b.imapPort === 993 ? "tls" : "starttls", username: b.email, auth: "password" });
    setSmtp({ host: b.smtpHost, port: b.smtpPort, security: b.smtpPort === 465 ? "tls" : "starttls", username: b.email, auth: "password" });
    setProvider("imap");
    setSource(null);
    setErrors(noErrors);
    setStep("configure");
  };

  const items = summaryItems(errors);

  const summary = items.length > 0 && (
    <div ref={summaryRef} tabIndex={-1} role="alert" aria-labelledby="acct-err-title" className="mb-4 rounded-table bg-[var(--iw-red-wash)] p-4 ring-1 ring-[var(--iw-red)]/40 focus:outline-none">
      <h3 id="acct-err-title" className="text-[13px] font-semibold text-[var(--iw-red-text)]">There is a problem</h3>
      <ul className="mt-1.5 list-disc space-y-0.5 pl-5 text-[13px]">
        {items.map((it) => (
          <li key={it.id + it.text}>
            <a href={`#${it.id}`} className="text-[var(--iw-red-text)] underline underline-offset-2" onClick={(ev) => { ev.preventDefault(); document.getElementById(it.id)?.focus(); }}>{it.text}</a>
          </li>
        ))}
      </ul>
    </div>
  );

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => !o && close()}
      title={step === "identify" ? "Add an email account" : "Connect your mailbox"}
      description={step === "identify" ? "WolfMail works with Gmail, Microsoft 365 and any IMAP mailbox. Your mail stays on this computer." : email}
      className="w-[min(600px,calc(100vw-32px))]"
      footer={
        step === "identify" ? (
          <>
            <Button variant="outline" onClick={close}>Cancel</Button>
            <Button onClick={() => continueIdentify()} disabled={busy === "detect"}>{busy === "detect" ? <Spinner /> : null} Continue <ChevronRight /></Button>
          </>
        ) : (
          <>
            <Button variant="ghost" onClick={() => { setStep("identify"); setErrors(noErrors); }}><ArrowLeft /> Back</Button>
            <div className="flex-1" />
            <Button variant="outline" onClick={close}>Cancel</Button>
            <Button onClick={() => testAndAdd()} disabled={busy === "test" || busy === "oauth"}>{busy === "test" ? <Spinner /> : <ShieldCheck />} {busy === "test" ? "Checking…" : "Test & add"}</Button>
          </>
        )
      }
    >
      {summary}
      {step === "identify" ? (
        <form onSubmit={(e) => continueIdentify(e)} className="space-y-5" noValidate>
          <div className="grid grid-cols-2 gap-2.5">
            <ProviderButton label="Google" sub="Gmail & Workspace" onClick={() => { if (email) void continueIdentify(undefined, "gmail"); else document.getElementById("acct-email")?.focus(); }} letter="G" />
            <ProviderButton label="Microsoft" sub="Outlook & 365" onClick={() => { if (email) void continueIdentify(undefined, "outlook"); else document.getElementById("acct-email")?.focus(); }} letter="M" />
          </div>
          <Field id="acct-email" label="Email address" error={errors.email}>
            <Input type="email" autoFocus value={email} onChange={(e) => setEmail(e.target.value)} placeholder="you@yourbusiness.com" autoComplete="email" spellCheck={false} />
          </Field>
          <Field id="acct-yourname" label="Your name" hint="What people see in their inbox when you send from this address.">
            <Input value={yourName} onChange={(e) => setYourName(e.target.value)} autoComplete="name" />
          </Field>
          <Field id="acct-name" label="Account name" hint="Shown in the sidebar. You can change it later." error={errors.name}>
            <Input value={name} onChange={(e) => setName(e.target.value)} placeholder={email.split("@")[1] || "Work"} />
          </Field>
          <ColorPicker name="acct-color" value={color} onChange={setColor} label="Account colour" />

          {iwConnected && business && business.length > 0 && (
            <div className="rounded-card bg-surface-raised p-4 ring-1 ring-border">
              <div className="mb-2 flex items-center gap-2 text-[13px] font-semibold"><Building2 className="size-4 text-[var(--iw-red-text)]" /> Add my IntelligentWolf business mailboxes</div>
              <ul role="list" className="space-y-1">
                {business.filter((b) => !accounts.some((a) => a.email.toLowerCase() === b.email.toLowerCase())).map((b) => (
                  <li key={b.email}>
                    <button type="button" onClick={() => prefillBusiness(b)} className="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-[13px] hover:bg-accent">
                      <Mail className="size-4 text-muted-foreground" /><span className="min-w-0 flex-1 truncate">{b.email}</span><ChevronRight className="size-4 text-muted-foreground" />
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          )}
          <button type="submit" className="sr-only">Continue</button>
        </form>
      ) : (
        <form onSubmit={testAndAdd} className="space-y-5" noValidate>
          <div className="flex items-center gap-3 rounded-card bg-surface-raised p-4 ring-1 ring-border">
            <div className="flex size-10 items-center justify-center rounded-xl bg-[var(--iw-red-wash)] text-[var(--iw-red-text)]"><Server className="size-5" /></div>
            <div className="min-w-0 flex-1">
              <div className="text-[13.5px] font-semibold">{providerLabel[provider]}</div>
              <div className="truncate text-xs text-muted-foreground">
                {imap.host || "No server yet"}{source ? ` · settings from ${sourceLabel(source)}` : " · entered by hand"}
              </div>
            </div>
            <Button variant="ghost" size="sm" aria-expanded={showServers} aria-controls="acct-servers" onClick={() => setShowServers((v) => !v)}>
              {showServers ? "Hide" : "Edit"} server settings
            </Button>
          </div>

          <Field id="acct-name" label="Account name" error={errors.name}>
            <Input value={name} onChange={(e) => setName(e.target.value)} placeholder={email.split("@")[1] || "Work"} />
          </Field>

          {isOauth ? (
            <div id="acct-oauth" tabIndex={-1} className="space-y-3 focus:outline-none">
              {oauthDone ? (
                <div role="status" className="flex items-center gap-2.5 rounded-table bg-surface-raised p-3 text-[13px] ring-1 ring-[var(--iw-go-text)]/40">
                  <CheckCircle2 className="size-5 text-[var(--iw-go-text)]" /> Signed in as {email}. WolfMail never sees your password.
                </div>
              ) : oauthUnavailable ? (
                <div role="alert" className="space-y-2 rounded-table bg-surface-raised p-4 text-[13px] ring-1 ring-[var(--iw-warn)]/60">
                  <p className="font-semibold">“Sign in with {imap.auth === "oauthGoogle" ? "Google" : "Microsoft"}” isn't available in this build yet.</p>
                  <p className="text-muted-foreground">
                    This copy of WolfMail was built without a {imap.auth === "oauthGoogle" ? "Google" : "Microsoft"} app ID, so it can't open the sign-in page.
                    You can still connect today with an <strong>app password</strong> — a separate password your {imap.auth === "oauthGoogle" ? "Google" : "Microsoft"} account creates just for mail apps.
                  </p>
                  <Button size="sm" variant="outline" onClick={useAppPassword}><KeyRound /> Use an app password instead</Button>
                </div>
              ) : (
                <>
                  <Button size="lg" className="w-full" onClick={signIn} disabled={busy === "oauth"}>
                    {busy === "oauth" ? <Spinner /> : <Lock />} Sign in with {imap.auth === "oauthGoogle" ? "Google" : "Microsoft"}
                  </Button>
                  <p className="text-center text-xs text-muted-foreground">Your browser opens. Approve WolfMail there, then come back here.</p>
                  {errors.oauth && <p className="text-xs font-medium text-[var(--iw-red-text)]">{errors.oauth}</p>}
                </>
              )}
            </div>
          ) : (
            <Field id="acct-password" label={provider === "imap" ? "Password" : "App password"} error={errors.password}
              hint={provider === "imap" ? `Stored in ${secretsPlace(secretsBackend)}, never in WolfMail's settings files.` : "Create one in your account's security settings. It is stored in your system keychain."}>
              <Input type="password" autoFocus value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="current-password" />
            </Field>
          )}

          {(errors.imapTest || errors.smtpTest) && (
            <div className="grid gap-2 text-[13px]">
              {errors.imapTest && <TestResult label="Incoming server" error={errors.imapTest} />}
              {errors.smtpTest && <TestResult label="Outgoing server" error={errors.smtpTest} />}
            </div>
          )}

          {showServers && (
            <div id="acct-servers" className="space-y-3">
              <ServerFields kind="imap" idPrefix="acct" value={imap} onChange={setImap} errors={errors.imap} />
              <ServerFields kind="smtp" idPrefix="acct" value={smtp} onChange={setSmtp} errors={errors.smtp} />
            </div>
          )}
          <button type="submit" className="sr-only">Test and add</button>
        </form>
      )}
    </Dialog>
  );
}

function sourceLabel(s: Autodetected["source"]) {
  return { builtin: "WolfMail's provider list", ispdb: "Mozilla's provider database", autoconfig: "your provider", srv: "your domain's DNS", guess: "a best guess — please check" }[s];
}

function TestResult({ label, error }: { label: string; error: string }) {
  return (
    <div className="flex items-start gap-2 rounded-lg bg-[var(--iw-red-wash)] px-3 py-2">
      <span className="font-semibold text-[var(--iw-red-text)]">✕ {label}:</span>
      <span className="selectable min-w-0 flex-1">{error}</span>
    </div>
  );
}

function ProviderButton({ label, sub, onClick, letter }: { label: string; sub: string; onClick: () => void; letter: string }) {
  return (
    <button type="button" onClick={onClick}
      className={cn("flex items-center gap-3 rounded-card bg-surface p-3.5 text-left ring-1 ring-border transition-shadow hover:shadow-theme-md hover:ring-[var(--iw-red)]/40")}>
      <span aria-hidden className="flex size-9 items-center justify-center rounded-xl bg-surface-raised font-title text-lg">{letter}</span>
      <span>
        <span className="block text-[13.5px] font-semibold">{label}</span>
        <span className="block text-xs text-muted-foreground">{sub}</span>
      </span>
    </button>
  );
}
