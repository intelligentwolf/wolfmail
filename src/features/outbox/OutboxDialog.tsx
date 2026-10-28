import * as React from "react";
import { format } from "date-fns";
import { AlertTriangle, CheckCircle2, Clock, Loader2, Pencil, Undo2, Upload } from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { openComposer, setState, setUi, useStore } from "../../lib/store";
import type { OutboxItem } from "../../lib/types";
import { cn, displayName } from "../../lib/utils";
import { Button } from "../../components/ui/button";
import { Dialog } from "../../components/ui/dialog";
import { EmptyState, Spinner } from "../../components/ui/misc";
import { splitDraftHtml } from "../compose/drafts";

const statePill: Record<OutboxItem["state"], { label: string; icon: React.ComponentType<{ className?: string }>; cls: string }> = {
  waiting: { label: "Scheduled", icon: Clock, cls: "text-foreground ring-border" },
  sending: { label: "Sending", icon: Loader2, cls: "text-foreground ring-border" },
  sent: { label: "Sent", icon: CheckCircle2, cls: "text-[var(--iw-go-text)] ring-[var(--iw-go-text)]/40" },
  failed: { label: "Not sent", icon: AlertTriangle, cls: "text-[var(--iw-red-text)] ring-[var(--iw-red)]/40" },
};

export function OutboxDialog() {
  const open = useStore((s) => s.ui.outbox);
  const outbox = useStore((s) => s.outbox);
  const items = [...outbox].sort((a, b) => a.sendAt - b.sendAt);
  return (
    <Dialog open={open} onOpenChange={(o) => !o && setUi({ outbox: false })} title="Outbox"
      description="Messages waiting to go: scheduled sends, ones inside the undo window, and any the server refused."
      className="w-[min(640px,calc(100vw-32px))]"
      footer={<Button onClick={() => setUi({ outbox: false })}>Close</Button>}>
      {items.length === 0 ? (
        <EmptyState icon={<Upload className="size-6" />} title="Nothing waiting to send" body="Scheduled messages and ones that couldn't be sent show up here." />
      ) : (
        <ul role="list" className="space-y-2.5">
          {items.map((it) => <Row key={it.id} item={it} />)}
        </ul>
      )}
    </Dialog>
  );
}

function Row({ item }: { item: OutboxItem }) {
  const [busy, setBusy] = React.useState<null | "edit" | "cancel">(null);
  const pill = statePill[item.state];
  const recipients = [...item.draft.to, ...item.draft.cc, ...item.draft.bcc];

  // outbox_cancel returns the draft (docs/API.md); the engine then emits outbox://changed, but drop it locally too
  // so the list updates without waiting.
  const takeBack = async (): Promise<typeof item.draft> => {
    const back = await api.outboxCancel(item.id);
    setState((s) => ({ outbox: s.outbox.filter((o) => o.id !== item.id) }));
    return back;
  };

  const edit = async () => {
    setBusy("edit");
    try {
      const back = await takeBack();
      const parts = splitDraftHtml(back.html);
      setUi({ outbox: false });
      openComposer({ mode: "draft", draft: { ...back, sendAt: undefined, html: parts.body }, quoteHtml: parts.quote, quoteOnTop: parts.quoteOnTop, sourceId: back.replyToMessageId });
    } catch (e) {
      notify.error(e, "Couldn't take the message back — it may already have been sent");
    } finally {
      setBusy(null);
    }
  };

  const cancel = async () => {
    setBusy("cancel");
    try {
      const back = await takeBack();
      await api.draftSave({ ...back, sendAt: undefined });
      notify.ok("Send cancelled — the message is in Drafts");
    } catch (e) {
      notify.error(e, "Couldn't cancel the send");
    } finally {
      setBusy(null);
    }
  };

  const canChange = item.state === "waiting" || item.state === "failed";

  return (
    <li className="rounded-card p-4 ring-1 ring-border">
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <div className="truncate text-[13.5px] font-semibold">{item.draft.subject || "(no subject)"}</div>
          <div className="truncate text-xs text-muted-foreground">To {recipients.map((a) => displayName(a)).join(", ") || "(no recipients)"}</div>
          <div className="mt-1 text-xs tabular-nums text-muted-foreground">
            {item.state === "failed" ? "Tried " : "Sends "}{format(new Date(item.sendAt), "EEE d MMM yyyy, HH:mm")}
          </div>
        </div>
        <span className={cn("inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-semibold ring-1", pill.cls)}>
          <pill.icon className={cn("size-3.5", item.state === "sending" && "wm-spin")} /> {pill.label}
        </span>
      </div>
      {item.error && <p role="alert" className="selectable mt-2 rounded-lg bg-[var(--iw-red-wash)] px-3 py-2 text-xs text-[var(--iw-red-text)]">{item.error}</p>}
      {canChange && (
        <div className="mt-3 flex gap-2">
          <Button size="sm" variant="outline" onClick={edit} disabled={!!busy}>{busy === "edit" ? <Spinner /> : <Pencil />} Edit</Button>
          <Button size="sm" variant="ghost" onClick={cancel} disabled={!!busy}>{busy === "cancel" ? <Spinner /> : <Undo2 />} Cancel send</Button>
        </div>
      )}
    </li>
  );
}
