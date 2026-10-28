import * as React from "react";
import { KeyRound, LockKeyhole, Eye, EyeOff } from "lucide-react";
import { api } from "../../lib/api";
import { setState, useStore } from "../../lib/store";
import { Button } from "../../components/ui/button";
import { Field, Input } from "../../components/ui/form";
import { Spinner } from "../../components/ui/misc";
import type { ApiError } from "../../lib/types";

/** Shown only on computers with no usable system keychain (secrets.rs AutoStore → "file"). First run asks for a
 *  primary password; later runs ask to unlock. Errors are inline by the field (GOV.UK pattern), never a toast. */
export function VaultGate() {
  const secrets = useStore((s) => s.secrets)!;
  const creating = !secrets.initialized;
  const [pw, setPw] = React.useState("");
  const [pw2, setPw2] = React.useState("");
  const [show, setShow] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [err, setErr] = React.useState<{ field: "pw" | "pw2"; text: string } | null>(null);
  const summaryRef = React.useRef<HTMLDivElement>(null);

  React.useEffect(() => { if (err) summaryRef.current?.focus(); }, [err]);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (creating) {
      if (pw.length < 8) { setErr({ field: "pw", text: "Use at least 8 characters" }); return; }
      if (pw !== pw2) { setErr({ field: "pw2", text: "The two passwords don't match" }); return; }
    } else if (!pw) { setErr({ field: "pw", text: "Enter your primary password" }); return; }
    setBusy(true);
    setErr(null);
    try {
      const st = creating ? await api.secretsCreate(pw) : await api.secretsUnlock(pw);
      setState({ secrets: st });
    } catch (ex) {
      const a = ex as ApiError;
      setErr({ field: "pw", text: a.message ?? String(ex) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="flex h-full items-center justify-center bg-surface-inset p-6">
      <form onSubmit={submit} noValidate className="w-full max-w-[440px] rounded-card bg-surface p-7 shadow-[var(--shadow-card)] ring-1 ring-border">
        <div className="mb-5 flex size-12 items-center justify-center rounded-2xl bg-[var(--iw-red)] text-white">
          {creating ? <KeyRound className="size-6" /> : <LockKeyhole className="size-6" />}
        </div>
        <h1 className="font-title text-[24px] leading-tight">{creating ? "Choose a primary password" : "Unlock WolfMail"}</h1>
        <p className="mt-2 text-[13px] leading-relaxed text-muted-foreground">
          {creating
            ? "WolfMail keeps your account passwords in an encrypted vault on this computer. This password unlocks it each time WolfMail starts. It can't be recovered — if you forget it, you'll need to sign in to your accounts again."
            : "Your account passwords are in WolfMail's encrypted vault. Enter your primary password to start syncing."}
        </p>
        {secrets.reason && creating && <p className="mt-2 text-xs text-muted-foreground">{secrets.reason}</p>}

        {err && (
          <div ref={summaryRef} tabIndex={-1} role="alert" className="mt-4 rounded-lg bg-[var(--iw-red-wash)] px-3 py-2 text-[13px] font-medium text-[var(--iw-red-text)]">
            {err.text}
          </div>
        )}

        <div className="mt-5 space-y-4">
          <Field id="vault-pw" label={creating ? "Primary password" : "Password"} error={err?.field === "pw" ? err.text : undefined}
            hint={creating ? "At least 8 characters. A short sentence is easy to remember and hard to guess." : undefined}>
            <Input autoFocus type={show ? "text" : "password"} autoComplete={creating ? "new-password" : "current-password"} value={pw} onChange={(e) => setPw(e.target.value)} />
          </Field>
          {creating && (
            <Field id="vault-pw2" label="Type it again" error={err?.field === "pw2" ? err.text : undefined}>
              <Input type={show ? "text" : "password"} autoComplete="new-password" value={pw2} onChange={(e) => setPw2(e.target.value)} />
            </Field>
          )}
          <button type="button" onClick={() => setShow((v) => !v)} className="inline-flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground" aria-pressed={show}>
            {show ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />} {show ? "Hide password" : "Show password"}
          </button>
        </div>

        <Button type="submit" className="mt-6 h-10 w-full" disabled={busy}>
          {busy && <Spinner />} {creating ? "Create vault" : "Unlock"}
        </Button>
      </form>
    </main>
  );
}
