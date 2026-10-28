import * as React from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { CalendarPlus, CheckCircle2, ExternalLink, ListTodo, Lock, RefreshCw, Sparkles, UserRound } from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { setState, setUi, useStore } from "../../lib/store";
import type { ApiError } from "../../lib/types";
import { Button } from "../../components/ui/button";
import { Dialog } from "../../components/ui/dialog";
import { Spinner } from "../../components/ui/misc";

type Phase = { k: "intro" } | { k: "starting" } | { k: "waiting"; code: string; verifyUrl: string; expiresAt: number } | { k: "expired" } | { k: "done" } | { k: "error"; message: string };

const perks = [
  { icon: UserRound, title: "Who's writing to you", body: "See the sender's lead or customer record, pipeline stage and recent history beside every email." },
  { icon: Sparkles, title: "Alex drafts and summarises", body: "Summarise long threads and draft replies in your brand voice, paid from your IntelligentWolf credits." },
  { icon: ListTodo, title: "Turn email into work", body: "Log an email to the CRM or make it a task in one click." },
  { icon: CalendarPlus, title: "Booking links and business mailboxes", body: "Drop your booking link into a reply, and add your IntelligentWolf mailboxes without typing server settings." },
];

export function IwConnectDialog() {
  const open = useStore((s) => s.ui.iwConnect);
  const baseUrl = useStore((s) => s.settings?.iwBaseUrl);
  const [phase, setPhase] = React.useState<Phase>({ k: "intro" });
  const [now, setNow] = React.useState(Date.now());

  React.useEffect(() => { if (open) setPhase({ k: "intro" }); }, [open]);

  // Poll every 2 s while waiting (docs/IW-INTEGRATION.md "Pairing" step 3); stops when the dialog closes.
  React.useEffect(() => {
    if (!open || phase.k !== "waiting") return;
    let live = true;
    let timer: ReturnType<typeof setTimeout>;
    const tick = async () => {
      try {
        const r = await api.iwPairPoll();
        if (!live) return;
        if (r.state === "done") {
          const iw = await api.iwStatus();
          if (!live) return;
          setState({ iw });
          setPhase({ k: "done" });
          notify.ok(`Connected to ${iw.org?.name ?? "IntelligentWolf"}`);
          return;
        }
        if (r.state === "expired") { setPhase({ k: "expired" }); return; }
      } catch (e) {
        if (!live) return;
        setPhase({ k: "error", message: (e as ApiError).message ?? String(e) });
        return;
      }
      timer = setTimeout(tick, 2000);
    };
    timer = setTimeout(tick, 2000);
    return () => { live = false; clearTimeout(timer); };
  }, [open, phase.k]);

  React.useEffect(() => {
    if (phase.k !== "waiting") return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [phase.k]);

  const start = async () => {
    setPhase({ k: "starting" });
    try {
      const r = await api.iwPairStart(baseUrl || undefined);
      setPhase({ k: "waiting", ...r });
      openUrl(r.verifyUrl).catch((e) => notify.error(e, "Couldn't open your browser — open the link below yourself"));
    } catch (e) {
      setPhase({ k: "error", message: (e as ApiError).message ?? String(e) });
    }
  };

  const close = () => setUi({ iwConnect: false });
  const left = phase.k === "waiting" ? Math.max(0, Math.round((phase.expiresAt - now) / 1000)) : 0;

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => !o && close()}
      title="Connect IntelligentWolf"
      description="Optional. WolfMail works fully on its own — connecting adds your business brain to your inbox."
      className="w-[min(600px,calc(100vw-32px))]"
      footer={
        phase.k === "done" ? <Button onClick={close}>Done</Button>
          : phase.k === "intro" || phase.k === "starting" ? (
            <>
              <Button variant="outline" onClick={close}>Not now</Button>
              <Button onClick={start} disabled={phase.k === "starting"}>{phase.k === "starting" && <Spinner />} Connect</Button>
            </>
          ) : phase.k === "waiting" ? <Button variant="outline" onClick={close}>Cancel</Button>
          : (
            <>
              <Button variant="outline" onClick={close}>Close</Button>
              <Button onClick={start}><RefreshCw /> Get a new code</Button>
            </>
          )
      }
    >
      {(phase.k === "intro" || phase.k === "starting") && (
        <div className="space-y-5">
          <ul role="list" className="grid gap-3 sm:grid-cols-2">
            {perks.map((p) => (
              <li key={p.title} className="rounded-card bg-surface-raised p-4 ring-1 ring-border">
                <p.icon className="mb-2 size-5 text-[var(--iw-red-text)]" />
                <div className="text-[13.5px] font-semibold">{p.title}</div>
                <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{p.body}</p>
              </li>
            ))}
          </ul>
          <div className="flex gap-3 rounded-card p-4 ring-1 ring-border">
            <Lock className="mt-0.5 size-5 shrink-0 text-[var(--iw-go-text)]" />
            <p className="text-[13px] leading-relaxed">
              <strong>Your mail stays on this computer.</strong> IntelligentWolf only sees a message when you press one of its buttons on that message —
              and then only that one. WolfMail gets its own connection that you can revoke any time from IntelligentWolf's settings.
            </p>
          </div>
        </div>
      )}

      {phase.k === "waiting" && (
        <div className="space-y-5 text-center">
          <p className="text-[13px] text-muted-foreground">Your browser has opened IntelligentWolf. Check the code matches, choose your workspace and press <strong className="text-foreground">Connect WolfMail</strong>.</p>
          <div aria-label={`Pairing code ${phase.code.split("").join(" ")}`} className="mx-auto w-fit rounded-card bg-surface-inset px-8 py-5 font-title text-4xl tracking-[0.18em] tabular-nums ring-1 ring-border selectable">
            {phase.code}
          </div>
          <div className="flex items-center justify-center gap-2 text-xs text-muted-foreground" role="status">
            <Spinner className="size-3.5" /> Waiting for you to approve · expires in {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")}
          </div>
          <Button variant="link" onClick={() => openUrl(phase.verifyUrl).catch((e) => notify.error(e, "Couldn't open your browser"))}>
            <ExternalLink /> Open the page again
          </Button>
          <p className="selectable break-all text-[11px] text-muted-foreground">{phase.verifyUrl}</p>
        </div>
      )}

      {phase.k === "expired" && (
        <div role="alert" className="rounded-card bg-surface-raised p-5 text-center ring-1 ring-[var(--iw-warn)]/60">
          <p className="text-[13.5px] font-semibold">That code has expired</p>
          <p className="mt-1 text-[13px] text-muted-foreground">Codes last 10 minutes. Get a new one and approve it in your browser.</p>
        </div>
      )}

      {phase.k === "error" && (
        <div role="alert" className="rounded-card bg-[var(--iw-red-wash)] p-5 ring-1 ring-[var(--iw-red)]/40">
          <p className="text-[13.5px] font-semibold text-[var(--iw-red-text)]">Couldn't connect to IntelligentWolf</p>
          <p className="selectable mt-1 text-[13px]">{phase.message}</p>
        </div>
      )}

      {phase.k === "done" && (
        <div role="status" className="flex flex-col items-center py-4 text-center">
          <CheckCircle2 className="mb-3 size-12 text-[var(--iw-go-text)]" />
          <p className="font-title text-lg">You're connected</p>
          <p className="mt-1 max-w-sm text-[13px] text-muted-foreground">Open any email to see the sender's IntelligentWolf record in the panel on the right.</p>
        </div>
      )}
    </Dialog>
  );
}
