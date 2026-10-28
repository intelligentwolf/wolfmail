import type { Security, ServerConfig } from "../../lib/types";
import { Field, Input, NativeSelect } from "../../components/ui/form";

export type ServerErrors = Partial<Record<"host" | "port" | "username", string>>;

// Standard ports: IMAP 993 implicit TLS / 143 STARTTLS (RFC 8314 §3.2, RFC 9051 §2.1); SMTP submission 465 implicit
// TLS / 587 STARTTLS (RFC 8314 §3.3, RFC 6409 §3).
const DEFAULT_PORT: Record<"imap" | "smtp", Record<Security, number>> = {
  imap: { tls: 993, starttls: 143, none: 143 },
  smtp: { tls: 465, starttls: 587, none: 587 },
};

export function validateServer(s: ServerConfig, needUser: boolean): ServerErrors {
  const e: ServerErrors = {};
  if (!s.host.trim()) e.host = "Enter the server name, like imap.example.com";
  else if (/\s|\/|:/.test(s.host.trim())) e.host = "Enter just the server name — no spaces, slashes or port";
  if (!Number.isInteger(s.port) || s.port < 1 || s.port > 65535) e.port = "Enter a port between 1 and 65535";
  if (needUser && !s.username.trim()) e.username = "Enter the user name — usually your full email address";
  return e;
}

export function ServerFields({
  kind, value, onChange, errors, idPrefix,
}: { kind: "imap" | "smtp"; value: ServerConfig; onChange: (v: ServerConfig) => void; errors: ServerErrors; idPrefix: string }) {
  const title = kind === "imap" ? "Incoming (IMAP)" : "Outgoing (SMTP)";
  return (
    <fieldset className="rounded-table bg-surface-inset p-4 ring-1 ring-border">
      <legend className="px-1 text-[12px] font-semibold uppercase tracking-wide text-muted-foreground">{title}</legend>
      <div className="grid grid-cols-[1fr_96px] gap-3">
        <Field id={`${idPrefix}-${kind}-host`} label="Server" error={errors.host}>
          <Input value={value.host} onChange={(e) => onChange({ ...value, host: e.target.value })} autoComplete="off" spellCheck={false} />
        </Field>
        <Field id={`${idPrefix}-${kind}-port`} label="Port" error={errors.port}>
          <Input inputMode="numeric" value={String(value.port || "")} onChange={(e) => onChange({ ...value, port: Number(e.target.value.replace(/\D/g, "")) || 0 })} />
        </Field>
        <Field id={`${idPrefix}-${kind}-security`} label="Security" hint={value.security === "none" ? "Only allowed for a server on this computer (localhost)." : undefined}>
          <NativeSelect
            value={value.security}
            onChange={(e) => {
              const security = e.target.value as Security;
              const wasDefault = value.port === DEFAULT_PORT[kind][value.security];
              onChange({ ...value, security, port: wasDefault || !value.port ? DEFAULT_PORT[kind][security] : value.port });
            }}
          >
            <option value="tls">SSL/TLS</option>
            <option value="starttls">STARTTLS</option>
            <option value="none">None (local only)</option>
          </NativeSelect>
        </Field>
        <div />
        <Field id={`${idPrefix}-${kind}-user`} label="User name" error={errors.username} className="col-span-2">
          <Input value={value.username} onChange={(e) => onChange({ ...value, username: e.target.value })} autoComplete="off" spellCheck={false} />
        </Field>
      </div>
    </fieldset>
  );
}
