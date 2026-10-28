import * as React from "react";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Copy, ExternalLink, Globe, Check } from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { Dialog } from "../../components/ui/dialog";
import { Button } from "../../components/ui/button";
import { Spinner } from "../../components/ui/misc";

type Pending = { provider: "google" | "microsoft"; email: string; url: string };

/** While a browser sign-in is waiting, say so and give the link (engine `oauth://pending` → `oauth://done`).
 *  Without this the sign-in button just spun when the browser opened behind other windows or on another workspace
 *  (Paul, 28 Oct 2026). Every Google/Microsoft sign-in in the app goes through the engine flow that emits these. */
export function OAuthHost() {
  const [pending, setPending] = React.useState<Pending | null>(null);
  const [copied, setCopied] = React.useState(false);

  React.useEffect(() => {
    const offs: Promise<() => void>[] = [
      listen<Pending>("oauth://pending", (e) => { setPending(e.payload); setCopied(false); }),
      listen("oauth://done", () => setPending(null)),
    ];
    return () => { offs.forEach((p) => p.then((u) => u()).catch(() => {})); };
  }, []);

  if (!pending) return null;
  const name = pending.provider === "google" ? "Google" : "Microsoft";
  const copy = async () => {
    try { await navigator.clipboard.writeText(pending.url); setCopied(true); }
    catch (e) { notify.error(e, "Couldn't copy the link"); }
  };
  const cancel = () => { api.oauthCancel().catch((e) => notify.error(e, "Couldn't cancel sign-in")); };

  return (
    <Dialog
      open
      onOpenChange={(o) => { if (!o) cancel(); }}
      title={`Finish signing in with ${name}`}
      description={`We've opened ${name}'s sign-in page in your web browser for ${pending.email || "your account"}. Approve access there and WolfMail carries on by itself.`}
      footer={<Button variant="outline" onClick={cancel}>Cancel sign-in</Button>}
    >
      <div className="space-y-4">
        <div role="status" aria-live="polite" className="flex items-center gap-3 rounded-table bg-surface-raised p-3.5 text-[13px] ring-1 ring-border">
          <Spinner className="text-[var(--iw-red-text)]" />
          Waiting for you to finish in the browser…
        </div>
        <div className="space-y-2 text-[13px]">
          <p className="flex items-center gap-2 font-medium"><Globe className="size-4 text-muted-foreground" /> Don't see it?</p>
          <p className="text-muted-foreground">It may have opened in a browser window on another workspace or screen. Open it again, or copy the link into any browser on this computer.</p>
          <div className="flex flex-wrap gap-2 pt-1">
            <Button size="sm" onClick={() => openUrl(pending.url).catch((e) => notify.error(e, "Couldn't open the browser"))}><ExternalLink /> Open again</Button>
            <Button size="sm" variant="outline" onClick={copy}>{copied ? <Check /> : <Copy />} {copied ? "Copied" : "Copy link"}</Button>
          </div>
        </div>
      </div>
    </Dialog>
  );
}
