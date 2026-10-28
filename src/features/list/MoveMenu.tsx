import { Folder as FolderIcon } from "lucide-react";
import { useStore, getState } from "../../lib/store";
import { Ctx, Menu } from "../../components/ui/misc";

/** Folder picker for Move/Copy. IMAP can't move between accounts in one command, so only folders of the account
 *  that owns the selected messages are offered (when the selection spans accounts, the first one's). */
export function MoveMenuItems({ kind, onPick }: { kind: "menu" | "ctx"; onPick: (folderId: string) => void }) {
  const folders = useStore((s) => s.folders);
  const accounts = useStore((s) => s.accounts);
  const s = getState();
  const firstSel = s.list.items.find((m) => s.selected.includes(m.id));
  const accountId = firstSel?.accountId;
  const acct = accounts.find((a) => a.id === accountId);
  const list = folders
    .filter((f) => f.accountId === accountId && f.selectable && f.id !== firstSel?.folderId)
    .sort((a, b) => a.path.localeCompare(b.path, undefined, { sensitivity: "base" }));
  const Item = kind === "menu" ? Menu.Item : Ctx.Item;
  if (!acct) return <Item disabled>Select a message first</Item>;
  return (
    <>
      <div className="px-2.5 pb-1 pt-1.5 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">{acct.name}</div>
      {list.map((f) => (
        <Item key={f.id} onSelect={() => onPick(f.id)}>
          <span style={{ paddingLeft: f.depth * 12 }} className="flex items-center gap-2.5"><FolderIcon />{f.name}</span>
        </Item>
      ))}
    </>
  );
}
