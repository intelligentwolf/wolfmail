import * as React from "react";
import { X } from "lucide-react";
import { api } from "../../lib/api";
import type { Address } from "../../lib/types";
import { cn, displayName, formatAddress, parseAddresses } from "../../lib/utils";
import { Avatar } from "../../components/ui/misc";

/** Recipient chips with autocomplete (combobox pattern, W3C APG): type → suggestions from contacts_suggest,
 *  ↑/↓ to choose, Enter/Tab/comma to commit, Backspace on empty removes the last chip, paste accepts lists. */
export function AddressField({
  label, value, onChange, autoFocus, id, invalid, describedBy, trailing,
}: {
  label: string; value: Address[]; onChange: (v: Address[]) => void; autoFocus?: boolean; id: string;
  invalid?: boolean; describedBy?: string; trailing?: React.ReactNode;
}) {
  const [text, setText] = React.useState("");
  const [sugs, setSugs] = React.useState<Address[]>([]);
  const [active, setActive] = React.useState(0);
  const [bad, setBad] = React.useState<string | null>(null);
  const inputRef = React.useRef<HTMLInputElement>(null);
  const listId = `${id}-suggestions`;

  React.useEffect(() => {
    const q = text.trim();
    if (q.length < 1) { setSugs([]); return; }
    let live = true;
    const t = setTimeout(() => {
      api.contactsSuggest(q, 8)
        .then((r) => {
          if (!live) return;
          const have = new Set(value.map((a) => a.email.toLowerCase()));
          setSugs(r.filter((a) => !have.has(a.email.toLowerCase())));
          setActive(0);
        })
        .catch(() => live && setSugs([]));
    }, 90);
    return () => { live = false; clearTimeout(t); };
  }, [text, value]);

  const commit = (raw?: string) => {
    const src = raw ?? text;
    if (!src.trim()) return true;
    const { ok, bad: wrong } = parseAddresses(src);
    const have = new Set(value.map((a) => a.email.toLowerCase()));
    const add = ok.filter((a) => !have.has(a.email.toLowerCase()));
    if (add.length) onChange([...value, ...add]);
    if (wrong.length) { setBad(`“${wrong.join(", ")}” isn't an email address`); setText(wrong.join(", ")); return false; }
    setBad(null);
    setText("");
    return true;
  };
  const pick = (a: Address) => {
    onChange([...value, a]);
    setText("");
    setSugs([]);
    setBad(null);
    inputRef.current?.focus();
  };

  const open = sugs.length > 0 && text.trim().length > 0;

  return (
    <div className="relative flex min-h-10 items-start gap-2 border-b border-border px-4 py-1.5" onClick={() => inputRef.current?.focus()}>
      <label htmlFor={id} className="mt-[7px] w-12 shrink-0 text-[13px] text-muted-foreground">{label}</label>
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1">
        {value.map((a, i) => (
          <span key={a.email + i} title={formatAddress(a)} className="inline-flex h-7 max-w-[260px] items-center gap-1.5 rounded-full bg-surface-raised pl-0.5 pr-1 text-[12.5px] ring-1 ring-border">
            <Avatar address={a} size={22} />
            <span className="truncate">{displayName(a)}</span>
            <button type="button" aria-label={`Remove ${formatAddress(a)}`} onClick={(e) => { e.stopPropagation(); onChange(value.filter((_, j) => j !== i)); }}
              className="flex size-5 items-center justify-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground">
              <X className="size-3" />
            </button>
          </span>
        ))}
        <input
          ref={inputRef}
          id={id}
          autoFocus={autoFocus}
          value={text}
          role="combobox"
          aria-expanded={open}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={open ? `${listId}-${active}` : undefined}
          aria-invalid={invalid || !!bad || undefined}
          aria-describedby={[describedBy, bad ? `${id}-bad` : undefined].filter(Boolean).join(" ") || undefined}
          onChange={(e) => { setText(e.target.value); if (bad) setBad(null); }}
          onPaste={(e) => {
            const t = e.clipboardData.getData("text");
            if (/[,;\n]/.test(t)) { e.preventDefault(); commit(text + t); }
          }}
          onKeyDown={(e) => {
            if (open && e.key === "ArrowDown") { e.preventDefault(); setActive((a) => Math.min(sugs.length - 1, a + 1)); return; }
            if (open && e.key === "ArrowUp") { e.preventDefault(); setActive((a) => Math.max(0, a - 1)); return; }
            if ((e.key === "Enter" || e.key === "Tab") && open) { e.preventDefault(); pick(sugs[active]); return; }
            if (e.key === "Enter" || e.key === "," || e.key === ";") { if (text.trim()) { e.preventDefault(); commit(); } return; }
            if (e.key === "Tab" && text.trim()) { if (!commit()) e.preventDefault(); return; }
            if (e.key === "Backspace" && !text && value.length) onChange(value.slice(0, -1));
            if (e.key === "Escape" && open) { e.stopPropagation(); setSugs([]); }
          }}
          onBlur={() => { setTimeout(() => { commit(); setSugs([]); }, 120); }}
          className="h-7 min-w-[120px] flex-1 bg-transparent text-[13px] outline-none"
        />
      </div>
      {trailing}
      {bad && <p id={`${id}-bad`} role="alert" className="absolute -bottom-5 left-[72px] z-10 rounded bg-popover px-1.5 text-[11.5px] font-medium text-[var(--iw-red-text)]">{bad}</p>}
      {open && (
        <ul id={listId} role="listbox" aria-label={`${label} suggestions`} className="absolute left-16 top-full z-30 mt-1 w-[min(420px,90%)] overflow-hidden rounded-table bg-popover p-1 shadow-[var(--shadow-pop)] ring-1 ring-border">
          {sugs.map((a, i) => (
            <li key={a.email} id={`${listId}-${i}`} role="option" aria-selected={i === active}
              onMouseDown={(e) => { e.preventDefault(); pick(a); }}
              onMouseEnter={() => setActive(i)}
              className={cn("flex cursor-default items-center gap-2.5 rounded-md px-2 py-1.5", i === active && "bg-accent")}>
              <Avatar address={a} size={26} />
              <div className="min-w-0">
                <div className="truncate text-[13px] font-medium">{displayName(a)}</div>
                {a.name && <div className="truncate text-xs text-muted-foreground">{a.email}</div>}
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
