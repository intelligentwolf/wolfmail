import * as React from "react";
import { Plus, Trash2, ArrowUp, ArrowDown, Pencil, Filter as FilterIcon, X } from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { useStore } from "../../lib/store";
import type { ApiError, FilterAction, FilterCondition, FilterField, FilterOp, FilterRule, MessageSummary } from "../../lib/types";
import { Button, IconButton } from "../../components/ui/button";
import { ConfirmDialog, Dialog } from "../../components/ui/dialog";
import { Field, Input, NativeSelect, Switch } from "../../components/ui/form";
import { EmptyState, Skeleton, Spinner } from "../../components/ui/misc";

// Persona/story: someone with many mailboxes wants newsletters, receipts and noisy senders filed away automatically,
// the way Betterbird's message filters did it, without touching each message.

const FIELDS: { value: FilterField; label: string }[] = [
  { value: "from", label: "From" },
  { value: "to", label: "To" },
  { value: "cc", label: "Cc" },
  { value: "toOrCc", label: "To or Cc" },
  { value: "anyAddress", label: "Any address" },
  { value: "subject", label: "Subject" },
  { value: "hasAttachment", label: "Has an attachment" },
];
const TEXT_OPS: { value: FilterOp; label: string }[] = [
  { value: "contains", label: "contains" },
  { value: "notContains", label: "doesn't contain" },
  { value: "is", label: "is" },
  { value: "isNot", label: "isn't" },
  { value: "beginsWith", label: "begins with" },
  { value: "endsWith", label: "ends with" },
  { value: "matchesRegex", label: "matches pattern (regex)" },
];
const BOOL_OPS: { value: FilterOp; label: string }[] = [
  { value: "isTrue", label: "yes" },
  { value: "isFalse", label: "no" },
];
const ACTIONS: { value: FilterAction["type"]; label: string }[] = [
  { value: "moveTo", label: "Move to folder" },
  { value: "copyTo", label: "Copy to folder" },
  { value: "tag", label: "Tag" },
  { value: "markRead", label: "Mark as read" },
  { value: "star", label: "Star" },
  { value: "archive", label: "Archive" },
  { value: "trash", label: "Delete" },
  { value: "junk", label: "Mark as junk" },
  { value: "stop", label: "Stop — don't run later filters" },
];

/** Prefill from "Create filter from this sender…" in the message list. */
let pendingFromMessage: MessageSummary | null = null;
export function startFilterFromMessage(m: MessageSummary) {
  pendingFromMessage = m;
}

export function FiltersPage() {
  const [rules, setRules] = React.useState<FilterRule[] | null>(null);
  const [error, setError] = React.useState<string>();
  const [editing, setEditing] = React.useState<FilterRule | null>(null);
  const [deleting, setDeleting] = React.useState<FilterRule | null>(null);
  const accounts = useStore((s) => s.accounts);
  const folders = useStore((s) => s.folders);
  const tags = useStore((s) => s.tags);

  const load = React.useCallback(() => {
    api.filtersList().then((r) => { setRules(r); setError(undefined); }).catch((e) => setError((e as ApiError).message ?? String(e)));
  }, []);
  React.useEffect(() => {
    load();
    if (pendingFromMessage) {
      const m = pendingFromMessage;
      pendingFromMessage = null;
      setEditing(blankRule({
        name: `From ${m.from.name || m.from.email}`,
        accountId: m.accountId,
        conditions: [{ field: "from", op: "contains", value: m.from.email }],
      }));
    }
  }, [load]);

  const summary = (r: FilterRule) => {
    const conds = r.conditions.map((c) => {
      const f = FIELDS.find((x) => x.value === c.field)?.label ?? c.field;
      if (c.field === "hasAttachment") return c.op === "isFalse" ? "no attachment" : "has an attachment";
      return `${f} ${TEXT_OPS.find((o) => o.value === c.op)?.label ?? c.op} “${c.value}”`;
    }).join(r.matchAll ? " and " : " or ");
    const acts = r.actions.map((a) => {
      if (a.type === "moveTo" || a.type === "copyTo") return `${a.type === "moveTo" ? "move to" : "copy to"} ${folders.find((f) => f.id === a.folderId)?.name ?? "a folder that no longer exists"}`;
      if (a.type === "tag") return `tag ${tags.find((t) => t.key === a.tag)?.name ?? a.tag}`;
      return ACTIONS.find((x) => x.value === a.type)?.label.toLowerCase() ?? a.type;
    }).join(", ");
    return `${conds} → ${acts}`;
  };

  const persist = async (r: FilterRule) => {
    try { await api.filterUpsert(r); load(); } catch (e) { notify.error(e, "Couldn't save the filter"); }
  };
  const move = async (i: number, d: -1 | 1) => {
    if (!rules) return;
    const next = [...rules];
    const [x] = next.splice(i, 1);
    next.splice(i + d, 0, x);
    setRules(next);
    try { await api.filtersReorder(next.map((r) => r.id)); } catch (e) { notify.error(e, "Couldn't reorder"); load(); }
  };

  return (
    <>
      <header className="mb-6 flex items-start gap-4 pr-10">
        <div className="min-w-0 flex-1">
          <h2 className="font-title text-[22px] leading-tight">Filters</h2>
          <p className="mt-1 text-[13px] text-muted-foreground">They run on new mail as it reaches an Inbox, top to bottom, before you're notified. Right-click a folder to run them on mail already there.</p>
        </div>
        <Button onClick={() => setEditing(blankRule({}))}><Plus /> New filter</Button>
      </header>

      {error ? (
        <p role="alert" className="rounded-lg bg-[var(--iw-red-wash)] p-3 text-sm text-[var(--iw-red-text)]">Couldn't load filters: {error}</p>
      ) : !rules ? (
        <div aria-hidden className="space-y-3"><Skeleton className="h-16 w-full rounded-card" /><Skeleton className="h-16 w-full rounded-card" /></div>
      ) : rules.length === 0 ? (
        <div className="rounded-card ring-1 ring-border">
          <EmptyState icon={<FilterIcon className="size-6" />} title="No filters yet"
            body="File newsletters, tag receipts or star mail from important people automatically."
            action={<Button onClick={() => setEditing(blankRule({}))}><Plus /> New filter</Button>} />
        </div>
      ) : (
        <ol className="space-y-2.5">
          {rules.map((r, i) => {
            const acct = accounts.find((a) => a.id === r.accountId);
            return (
              <li key={r.id} className="flex items-center gap-3 rounded-card bg-surface px-4 py-3 ring-1 ring-border">
                <div className="flex flex-col">
                  <IconButton label={`Move “${r.name}” up`} size="iconSm" disabled={i === 0} onClick={() => move(i, -1)}><ArrowUp /></IconButton>
                  <IconButton label={`Move “${r.name}” down`} size="iconSm" disabled={i === rules.length - 1} onClick={() => move(i, 1)}><ArrowDown /></IconButton>
                </div>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-[13.5px] font-semibold">{r.name}</span>
                    <span className="inline-flex shrink-0 items-center gap-1.5 rounded-full bg-surface-raised px-2 py-px text-[11px] text-muted-foreground ring-1 ring-border">
                      {acct && <span className="size-2 rounded-full" style={{ background: acct.color }} />}{acct ? acct.name : "All accounts"}
                    </span>
                    {!r.enabled && <span className="text-[11px] font-medium text-muted-foreground">Off</span>}
                  </div>
                  <p className="mt-0.5 truncate text-xs text-muted-foreground" title={summary(r)}>{summary(r)}</p>
                </div>
                <div className="w-20"><Switch id={`flt-${r.id}`} label={r.enabled ? "On" : "Off"} checked={r.enabled} onCheckedChange={(v) => persist({ ...r, enabled: v })} /></div>
                <IconButton label={`Edit “${r.name}”`} size="iconSm" onClick={() => setEditing(r)}><Pencil /></IconButton>
                <IconButton label={`Delete “${r.name}”`} size="iconSm" onClick={() => setDeleting(r)}><Trash2 /></IconButton>
              </li>
            );
          })}
        </ol>
      )}

      {editing && <FilterEditor rule={editing} onClose={() => setEditing(null)} onSaved={() => { setEditing(null); load(); }} />}
      <ConfirmDialog open={!!deleting} onOpenChange={(o) => !o && setDeleting(null)} title={`Delete “${deleting?.name ?? ""}”?`}
        body="Mail it already filed stays where it is." confirmLabel="Delete filter" destructive
        onConfirm={async () => {
          if (!deleting) return;
          try { await api.filterDelete(deleting.id); notify.ok("Filter deleted"); load(); } catch (e) { notify.error(e, "Couldn't delete the filter"); }
        }} />
    </>
  );
}

function blankRule(p: Partial<FilterRule>): FilterRule {
  return {
    id: "", name: "", enabled: true, accountId: null, matchAll: true,
    conditions: [{ field: "from", op: "contains", value: "" }], actions: [{ type: "moveTo", folderId: "" }], sortOrder: 0, ...p,
  };
}

function FilterEditor({ rule, onClose, onSaved }: { rule: FilterRule; onClose: () => void; onSaved: () => void }) {
  const accounts = useStore((s) => s.accounts);
  const folders = useStore((s) => s.folders);
  const tags = useStore((s) => s.tags);
  const [r, setR] = React.useState<FilterRule>(rule);
  const [errors, setErrors] = React.useState<Record<string, string>>({});
  const [busy, setBusy] = React.useState(false);
  const summaryRef = React.useRef<HTMLDivElement>(null);

  const accountFolders = folders.filter((f) => f.accountId === r.accountId && f.selectable);
  const needsAccount = r.actions.some((a) => a.type === "moveTo" || a.type === "copyTo");

  const setCond = (i: number, c: Partial<FilterCondition>) => setR((x) => ({ ...x, conditions: x.conditions.map((y, j) => (j === i ? { ...y, ...c } : y)) }));
  const setAct = (i: number, a: FilterAction) => setR((x) => ({ ...x, actions: x.actions.map((y, j) => (j === i ? a : y)) }));

  const save = async () => {
    const e: Record<string, string> = {};
    if (!r.name.trim()) e.name = "Give the filter a name";
    if (r.conditions.some((c) => c.field !== "hasAttachment" && !c.value.trim())) e.conditions = "Every condition needs something to match";
    if (needsAccount && !r.accountId) e.accountId = "Choose the account — a filter that moves mail works within one account";
    if (r.actions.some((a) => (a.type === "moveTo" || a.type === "copyTo") && !a.folderId)) e.actions = "Choose a folder for every move or copy";
    if (r.actions.some((a) => a.type === "tag" && !a.tag)) e.actions = "Choose a tag";
    setErrors(e);
    if (Object.keys(e).length) { setTimeout(() => summaryRef.current?.focus(), 0); return; }
    setBusy(true);
    try {
      await api.filterUpsert({ ...r, name: r.name.trim() });
      notify.ok(rule.id ? "Filter saved" : "Filter created");
      onSaved();
    } catch (ex) {
      const a = ex as ApiError;
      if (a.kind === "invalid" && a.field) { setErrors({ [a.field]: a.message }); setTimeout(() => summaryRef.current?.focus(), 0); }
      else notify.error(ex, "Couldn't save the filter");
    } finally { setBusy(false); }
  };

  const errList = Object.entries(errors);
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()} title={rule.id ? "Edit filter" : "New filter"} className="w-[min(720px,calc(100vw-32px))]"
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button onClick={save} disabled={busy}>{busy && <Spinner />} Save filter</Button></>}>
      <form onSubmit={(e) => { e.preventDefault(); void save(); }} className="space-y-5">
        {errList.length > 0 && (
          <div ref={summaryRef} tabIndex={-1} role="alert" className="rounded-table bg-[var(--iw-red-wash)] p-3 text-[13px] text-[var(--iw-red-text)] ring-1 ring-[var(--iw-red)]/30">
            <p className="font-semibold">There is a problem</p>
            <ul className="mt-1 list-disc pl-5">{errList.map(([k, v]) => <li key={k}>{v}</li>)}</ul>
          </div>
        )}
        <div className="grid gap-4 sm:grid-cols-2">
          <Field id="flt-name" label="Name" error={errors.name}>
            <Input value={r.name} onChange={(e) => setR({ ...r, name: e.target.value })} placeholder="e.g. Newsletters" />
          </Field>
          <Field id="flt-account" label="Applies to" error={errors.accountId}>
            <NativeSelect value={r.accountId ?? ""} onChange={(e) => {
              const acc = e.target.value || null;
              // Folders belong to an account: clear folder picks when the account changes.
              setR({ ...r, accountId: acc, actions: r.actions.map((a) => (a.type === "moveTo" || a.type === "copyTo" ? { ...a, folderId: "" } : a)) });
            }}>
              <option value="">All accounts</option>
              {accounts.map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
            </NativeSelect>
          </Field>
        </div>

        <fieldset className="space-y-2.5">
          <legend className="mb-2 flex items-center gap-2 text-[13px] font-medium">
            When
            <NativeSelect aria-label="Match" className="w-56" value={r.matchAll ? "all" : "any"} onChange={(e) => setR({ ...r, matchAll: e.target.value === "all" })}>
              <option value="all">all of these are true</option>
              <option value="any">any of these is true</option>
            </NativeSelect>
          </legend>
          {errors.conditions && <p className="text-xs font-medium text-[var(--iw-red-text)]">{errors.conditions}</p>}
          {r.conditions.map((c, i) => (
            <div key={i} className="flex flex-wrap items-center gap-2">
              <NativeSelect aria-label={`Condition ${i + 1} field`} className="w-40" value={c.field}
                onChange={(e) => {
                  const field = e.target.value as FilterField;
                  setCond(i, { field, op: field === "hasAttachment" ? "isTrue" : c.op === "isTrue" || c.op === "isFalse" ? "contains" : c.op });
                }}>
                {FIELDS.map((f) => <option key={f.value} value={f.value}>{f.label}</option>)}
              </NativeSelect>
              <NativeSelect aria-label={`Condition ${i + 1} test`} className="w-52" value={c.op} onChange={(e) => setCond(i, { op: e.target.value as FilterOp })}>
                {(c.field === "hasAttachment" ? BOOL_OPS : TEXT_OPS).map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
              </NativeSelect>
              {c.field !== "hasAttachment" && (
                <Input aria-label={`Condition ${i + 1} value`} className="min-w-[160px] flex-1" value={c.value}
                  aria-invalid={!!errors.conditions && !c.value.trim()} onChange={(e) => setCond(i, { value: e.target.value })} />
              )}
              <IconButton label={`Remove condition ${i + 1}`} size="iconSm" disabled={r.conditions.length === 1}
                onClick={() => setR({ ...r, conditions: r.conditions.filter((_, j) => j !== i) })}><X /></IconButton>
            </div>
          ))}
          <Button variant="ghost" size="sm" onClick={() => setR({ ...r, conditions: [...r.conditions, { field: "subject", op: "contains", value: "" }] })}><Plus /> Add condition</Button>
        </fieldset>

        <fieldset className="space-y-2.5">
          <legend className="mb-2 text-[13px] font-medium">Then</legend>
          {errors.actions && <p className="text-xs font-medium text-[var(--iw-red-text)]">{errors.actions}</p>}
          {r.actions.map((a, i) => (
            <div key={i} className="flex flex-wrap items-center gap-2">
              <NativeSelect aria-label={`Action ${i + 1}`} className="w-60" value={a.type}
                onChange={(e) => {
                  const t = e.target.value as FilterAction["type"];
                  setAct(i, t === "moveTo" || t === "copyTo" ? { type: t, folderId: "" } : t === "tag" ? { type: "tag", tag: tags[0]?.key ?? "" } : ({ type: t } as FilterAction));
                }}>
                {ACTIONS.map((x) => <option key={x.value} value={x.value}>{x.label}</option>)}
              </NativeSelect>
              {(a.type === "moveTo" || a.type === "copyTo") && (
                <NativeSelect aria-label={`Action ${i + 1} folder`} className="min-w-[200px] flex-1" value={a.folderId} disabled={!r.accountId}
                  onChange={(e) => setAct(i, { ...a, folderId: e.target.value })}>
                  <option value="">{r.accountId ? "Choose a folder…" : "Choose an account above first"}</option>
                  {accountFolders.map((f) => <option key={f.id} value={f.id}>{"  ".repeat(f.depth)}{f.name}</option>)}
                </NativeSelect>
              )}
              {a.type === "tag" && (
                <NativeSelect aria-label={`Action ${i + 1} tag`} className="min-w-[200px] flex-1" value={a.tag} onChange={(e) => setAct(i, { type: "tag", tag: e.target.value })}>
                  {tags.map((t) => <option key={t.key} value={t.key}>{t.name}</option>)}
                </NativeSelect>
              )}
              <IconButton label={`Remove action ${i + 1}`} size="iconSm" disabled={r.actions.length === 1}
                onClick={() => setR({ ...r, actions: r.actions.filter((_, j) => j !== i) })}><X /></IconButton>
            </div>
          ))}
          <Button variant="ghost" size="sm" onClick={() => setR({ ...r, actions: [...r.actions, { type: "markRead" }] })}><Plus /> Add action</Button>
        </fieldset>

        <Switch id="flt-enabled" label="Filter is on" description="Off keeps the filter but stops it running." checked={r.enabled} onCheckedChange={(v) => setR({ ...r, enabled: v })} />
      </form>
    </Dialog>
  );
}
