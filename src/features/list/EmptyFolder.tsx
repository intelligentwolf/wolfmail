import * as React from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Trash2 } from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { refreshFolders, reloadList } from "../../lib/store";
import type { Account, Folder } from "../../lib/types";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/dialog";

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
        try {
          const n = await api.folderEmpty(folder.id);
          await refreshFolders();
          void reloadList();
          notify.ok(n === 0 ? `${name} was already empty` : `Erased ${n} message${n === 1 ? "" : "s"} from ${name}`);
        } catch (e) { notify.error(e, `Couldn't empty ${name}`); }
      }}
    />
  );
}

/** A slim strip over the list while Trash or Spam is open, with "Empty Trash" / "Empty Spam" — its own line, so a narrow list keeps
 *  its title and count readable. */
export function EmptyFolderStrip({ folder, account }: { folder: Folder; account?: Account }) {
  const [open, setOpen] = React.useState(false);
  const gmail = account?.imap.auth === "oauthGoogle";
  return (
    <div className="flex items-center gap-3 border-b border-border bg-surface-raised px-4 py-2 text-xs text-muted-foreground">
      <span className="min-w-0 flex-1">{gmail ? `Gmail erases mail in ${emptyName(folder)} after 30 days.` : `Mail in ${emptyName(folder)} can be erased for good.`}</span>
      <Button size="sm" variant="outline" onClick={() => setOpen(true)}><Trash2 /> Empty {emptyName(folder)}</Button>
      <EmptyFolderDialog folder={open ? folder : null} account={account} onClose={() => setOpen(false)} />
    </div>
  );
}
