import * as React from "react";
import { Download, UserPlus, Link2, ShieldCheck, Zap, Keyboard, ArrowRight } from "lucide-react";
import { api } from "../../lib/api";
import { setUi } from "../../lib/store";
import { Button } from "../../components/ui/button";

/** First run: one screen, three ways in. Import is first because most people arrive from Thunderbird/Betterbird
 *  with many accounts already set up (Paul's own profile has 11). */
export function Welcome() {
  const [found, setFound] = React.useState<{ accounts: number; source: string } | null>(null);
  React.useEffect(() => {
    api.importScan()
      .then((c) => {
        const n = c.reduce((s, x) => s + x.accounts.length, 0);
        if (n > 0) setFound({ accounts: n, source: c[0].source === "betterbird" ? "Betterbird" : "Thunderbird" });
      })
      .catch(() => setFound(null));
  }, []);

  return (
    <main className="relative flex h-full items-center justify-center overflow-y-auto bg-surface-inset p-6">
      <div aria-hidden className="pointer-events-none absolute inset-0 overflow-hidden">
        <div className="absolute -top-40 left-1/2 h-[520px] w-[920px] -translate-x-1/2 rounded-full bg-[var(--iw-red)] opacity-[0.07] blur-3xl" />
      </div>
      <div className="relative w-full max-w-[880px]">
        <div className="mb-10 flex flex-col items-center text-center">
          <img src="/icon.png" alt="" className="mb-5 size-16 rounded-2xl shadow-[var(--shadow-card)]" />
          <h1 className="font-title text-[40px] leading-none tracking-tight">Welcome to WolfMail</h1>
          <p className="mt-3 max-w-xl text-[15px] text-muted-foreground">
            Every inbox in one fast, private window — with your IntelligentWolf CRM and Alex right beside your mail.
          </p>
        </div>

        <div className="grid gap-4 md:grid-cols-3">
          <Card
            primary
            icon={<Download className="size-5" />}
            title={found ? `Bring over ${found.accounts} accounts` : "Import from Thunderbird"}
            body={found ? `We found your ${found.source} profile. Accounts, identities, signatures and tags come across in one go.` : "Copy accounts, identities, signatures and tags from Betterbird or Thunderbird."}
            cta="Import"
            onClick={() => setUi({ importer: true })}
          />
          <Card icon={<UserPlus className="size-5" />} title="Add an account" body="Gmail, Microsoft 365, or any IMAP mailbox — we find the server settings for you." cta="Add account" onClick={() => setUi({ addAccount: true })} />
          <Card icon={<Link2 className="size-5" />} title="Connect IntelligentWolf" body="See who's writing in your CRM, summarise threads with Alex, and turn emails into tasks." cta="Connect" onClick={() => setUi({ iwConnect: true })} />
        </div>

        <ul className="mt-10 grid gap-3 text-[13px] text-muted-foreground sm:grid-cols-3">
          <li className="flex items-center justify-center gap-2"><ShieldCheck className="size-4 text-[var(--iw-go-text)]" /> Mail stays on this computer</li>
          <li className="flex items-center justify-center gap-2"><Zap className="size-4 text-[var(--iw-warn-text)]" /> New mail the moment it lands</li>
          <li className="flex items-center justify-center gap-2"><Keyboard className="size-4 text-[var(--iw-red-text)]" /> Keyboard-first, Ctrl+K for anything</li>
        </ul>
      </div>
    </main>
  );
}

function Card({ icon, title, body, cta, onClick, primary }: { icon: React.ReactNode; title: string; body: string; cta: string; onClick: () => void; primary?: boolean }) {
  return (
    <div className={`flex flex-col rounded-card bg-surface p-5 shadow-[var(--shadow-card)] ring-1 ${primary ? "ring-[var(--iw-red)]/40" : "ring-border"}`}>
      <div className={`mb-4 flex size-10 items-center justify-center rounded-xl ${primary ? "bg-[var(--iw-red)] text-white" : "bg-surface-raised text-[var(--iw-red-text)]"}`}>{icon}</div>
      <h2 className="font-title text-[17px]">{title}</h2>
      <p className="mt-1.5 flex-1 text-[13px] leading-relaxed text-muted-foreground">{body}</p>
      <Button variant={primary ? "default" : "outline"} className="mt-5 w-full" onClick={onClick}>{cta} <ArrowRight /></Button>
    </div>
  );
}
