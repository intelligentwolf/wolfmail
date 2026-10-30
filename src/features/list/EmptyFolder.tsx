import * as React from "react";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Trash2 } from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { refreshFolders, reloadList } from "../../lib/store";
import type { Account, Folder } from "../../lib/types";
import { Button } from "../../components/ui/button";
import { Spinner } from "../../components/ui/misc";
import { ConfirmDialog } from "../../components/ui/dialog";

/** Folders being emptied right now (engine `folder://empty` running … done | failed), for the buttons to say so. */
const emptyingNow = new Set<string>();
const subs = new Set<() => void>();
const changed = () => subs.forEach((f) => f());
function useEmptying(folderId: string): boolean {
  return React.useSyncExternalStore((f) => { subs.add(f); return () => { subs.delete(f); }; }, () => emptyingNow.has(folderId));
}

type EmptyEvent = { folderId: string; name: string; role: string; state: "running" | "done" | "failed"; done?: number; total?: number; count?: number; message?: string };

/**
 * Says how an empty is going (Paul, 30 Oct 2026, after emptying a Junk of 84,547 locked WolfMail up: "it needs to have some thing
 * to tell us its emptying"). The engine empties in the background and reports `folder://empty`; this keeps one notice per folder:
 * "Emptying Junk… 41,000 of 84,547", then "Erased 84,547 messages from Junk" — or the reason it failed, which stays until closed.
 * Mounted once (App).
 */
export function EmptyProgress() {
  React.useEffect(() => {
    const off = listen<EmptyEvent>("folder://empty", ({ payload: e }) => {
      const id = `empty-${e.folderId}`;
      const name = e.role === "junk" ? "Spam" : e.role === "trash" ? "Trash" : e.name;
      if (e.state === "running") {
        const first = !emptyingNow.has(e.folderId);
        emptyingNow.add(e.folderId); changed();
        const text = e.total ? `Emptying ${name}… ${(e.done ?? 0).toLocaleString()} of ${e.total.toLocaleString()}` : `Emptying ${name}…`;
        // Announced once; the count then updates on screen only (a screen reader is not read every step).
        if (first) notify.info(text, { id, duration: Infinity });
        else toast(text, { id, duration: Infinity });
        return;
      }
      emptyingNow.delete(e.folderId); changed();
      notify.dismiss(id);
      void refreshFolders();
      void reloadList();
      if (e.state === "done") notify.ok(e.count === 0 ? `${name} was already empty` : `Erased ${(e.count ?? 0).toLocaleString()} message${e.count === 1 ? "" : "s"} from ${name}`);
      else notify.error({ kind: "server", message: e.message ?? "It stopped part way. Try Empty again." }, `Couldn't finish emptying ${name}`);
    });
    return () => { off.then((u) => u()).catch(() => {}); };
  }, []);
  return null;
}

/** Trash and Junk are the folders that can be emptied (engine folder_empty). */
export const canEmpty = (f?: Folder | null) => f?.role === "trash" || f?.role === "junk";
export const emptyName = (f?: Folder | null) => (f?.role === "junk" ? "Spam" : "Trash");

/**
 * Empty Trash / Empty Spam (Paul, 30 Oct 2026: "make empty trash and empty spam work"). Asks first — everything in the folder is
 * erased for good, on the server. A Gmail account (signed in with Google) can't erase mail: Gmail empties these itself after
 * 30 days, so it is said, with a button to the folder in Gmail on the web.
 */
export function EmptyFolderDialog({ folder, account, onClose }: { folder: Folder | null; account?: Account; onClose: () => void }) {
  const name = emptyName(folder);
  const gmail = account?.imap.auth === "oauthGoogle";
  if (gmail) {
    return (
      <ConfirmDialog
        open={!!folder}
        onOpenChange={(o) => !o && onClose()}
        title={`Empty ${name} in Gmail`}
        body={<>Gmail empties {name} by itself after 30 days. WolfMail's access to Gmail can move mail to {name} but can't erase it for good, so to empty it now, use Gmail on the web — the button opens it there for {account?.email}.</>}
        confirmLabel={`Open ${name} in Gmail`}
        onConfirm={async () => {
          const tab = folder?.role === "junk" ? "spam" : "trash";
          try { await openUrl(`https://mail.google.com/mail/u/?authuser=${encodeURIComponent(account?.email ?? "")}#${tab}`); }
          catch (e) { notify.error(e, "Couldn't open the browser"); }
        }}
      />
    );
  }
  return (
    <ConfirmDialog
      open={!!folder}
      onOpenChange={(o) => !o && onClose()}
      title={`Empty ${name}?`}
      body={<>Every message in {folder?.name ?? name}{account ? <> for {account.email}</> : null} is erased for good — on the server, for every device. This can't be undone.</>}
      confirmLabel={`Empty ${name}`}
      destructive
      onConfirm={async () => {
        if (!folder) return;
        // Starts it and returns at once — the work and its notice are EmptyProgress's; the dialog closes straight away.
        try { await api.folderEmpty(folder.id); }
        catch (e) { notify.error(e, `Couldn't empty ${name}`); }
      }}
    />
  );
}

/** A slim strip over the list while Trash or Spam is open, with "Empty Trash" / "Empty Spam" — its own line, so a narrow list keeps
 *  its title and count readable. */
export function EmptyFolderStrip({ folder, account }: { folder: Folder; account?: Account }) {
  const [open, setOpen] = React.useState(false);
  const gmail = account?.imap.auth === "oauthGoogle";
  const busy = useEmptying(folder.id);
  return (
    <div className="flex items-center gap-3 border-b border-border bg-surface-raised px-4 py-2 text-xs text-muted-foreground">
      <span className="min-w-0 flex-1">{gmail ? `Gmail erases mail in ${emptyName(folder)} after 30 days.` : `Mail in ${emptyName(folder)} can be erased for good.`}</span>
      <Button size="sm" variant="outline" disabled={busy} onClick={() => setOpen(true)}>{busy ? <Spinner /> : <Trash2 />} {busy ? "Emptying…" : `Empty ${emptyName(folder)}`}</Button>
      <EmptyFolderDialog folder={open ? folder : null} account={account} onClose={() => setOpen(false)} />
    </div>
  );
}
