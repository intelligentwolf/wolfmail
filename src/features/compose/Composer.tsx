import * as React from "react";
import { useEditor, EditorContent, type Editor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import Placeholder from "@tiptap/extension-placeholder";
import Image from "@tiptap/extension-image";
import TextAlign from "@tiptap/extension-text-align";
import * as Popover from "@radix-ui/react-popover";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import {
  X, Minus, Maximize2, Minimize2, Send, ChevronDown, Paperclip, Trash2, Bold, Italic, Underline, Strikethrough, List,
  ListOrdered, Quote, Link2, RemoveFormatting, Clock, CalendarClock, AlignLeft, AlignCenter, FileText, Sparkles, CalendarPlus,
} from "lucide-react";
import { addDays, format, nextMonday, setHours, setMinutes, startOfHour, addHours } from "date-fns";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { closeComposer, openComposer, updateComposer, useStore, type Composer } from "../../lib/store";
import type { DraftInput, Identity } from "../../lib/types";
import { cn, escapeHtml, modKey } from "../../lib/utils";
import { Button, IconButton } from "../../components/ui/button";
import { ConfirmDialog, Dialog } from "../../components/ui/dialog";
import { Input, Field } from "../../components/ui/form";
import { Menu, Spinner } from "../../components/ui/misc";
import { AddressField } from "./AddressField";
import { MessageBody } from "../reader/MessageBody";
import { allIdentities, htmlToText, identityById, joinDraftHtml, signatureHtml, splitDraftHtml } from "./drafts";

const ATTACH_WORDS = /\b(attach(ed|ment|ments|ing)?|enclosed|i('ve| have) included the file|see the (file|document)|pdf attached)\b/i;

export function ComposerDock() {
  const composers = useStore((s) => s.composers);
  const open = composers.find((c) => !c.minimized);
  const minimized = composers.filter((c) => c.minimized);
  return (
    <>
      {minimized.length > 0 && (
        <div className="pointer-events-none fixed bottom-0 right-4 z-40 flex items-end gap-2" style={{ right: open ? 680 : 16 }}>
          {minimized.map((c) => (
            <div key={c.key} className="pointer-events-auto flex h-10 w-60 items-center gap-2 rounded-t-xl bg-plum px-3 text-white shadow-[var(--shadow-pop)]">
              <button type="button" className="min-w-0 flex-1 truncate text-left text-[13px] font-medium" onClick={() => {
                for (const o of composers) if (!o.minimized) updateComposer(o.key, { minimized: true });
                updateComposer(c.key, { minimized: false });
              }}>
                {c.draft.subject || (c.mode === "new" ? "New message" : "Draft")}
              </button>
              <button type="button" aria-label="Close draft" className="rounded p-1 opacity-75 hover:opacity-100" onClick={() => closeComposer(c.key)}><X className="size-3.5" /></button>
            </div>
          ))}
        </div>
      )}
      {open && <ComposerWindow key={open.key} c={open} />}
    </>
  );
}

function ComposerWindow({ c }: { c: Composer }) {
  const settings = useStore((s) => s.settings);
  const accounts = useStore((s) => s.accounts);
  const iwConnected = useStore((s) => !!s.iw?.connected);
  const assistant = settings?.assistantName || "Alex";
  const [draft, setDraft] = React.useState<DraftInput>(c.draft);
  const [showCc, setShowCc] = React.useState(c.draft.cc.length > 0);
  const [showBcc, setShowBcc] = React.useState(c.draft.bcc.length > 0);
  const [savedAt, setSavedAt] = React.useState<number | null>(null);
  const [saving, setSaving] = React.useState(false);
  const [serverWarn, setServerWarn] = React.useState<string | null>(null);
  const [sending, setSending] = React.useState(false);
  const [toError, setToError] = React.useState<string>();
  const [confirm, setConfirm] = React.useState<null | { title: string; body: string; label: string; go: () => void }>(null);
  const [discardOpen, setDiscardOpen] = React.useState(false);
  const [laterOpen, setLaterOpen] = React.useState(false);
  const [attachMeta, setAttachMeta] = React.useState<Record<string, { name: string }>>({});
  const dirty = React.useRef(false);
  const draftRef = React.useRef(draft);
  draftRef.current = draft;

  const identity = identityById(draft.identityId);
  const sig = signatureHtml(identity);

  const editor = useEditor({
    extensions: [
      StarterKit.configure({ link: { openOnClick: false, autolink: true, HTMLAttributes: { rel: "noopener noreferrer" } } }),
      Placeholder.configure({ placeholder: c.mode === "new" ? "Write your message…" : "Write your reply…" }),
      Image.configure({ allowBase64: true, inline: false }),
      TextAlign.configure({ types: ["heading", "paragraph"] }),
    ],
    content: c.draft.html || "<p></p>",
    autofocus: c.mode === "new" && c.draft.to.length === 0 ? false : "start",
    onUpdate: () => { dirty.current = true; scheduleSave(); },
    editorProps: { attributes: { "aria-label": "Message body", role: "textbox", "aria-multiline": "true" } },
  });

  const build = React.useCallback((): DraftInput => {
    const body = editor?.getHTML() ?? draftRef.current.html;
    const html = joinDraftHtml(body, signatureHtml(identityById(draftRef.current.identityId)), c.quoteHtml, c.quoteOnTop);
    return { ...draftRef.current, html, text: htmlToText(html) };
  }, [editor, c.quoteHtml, c.quoteOnTop]);

  const saveTimer = React.useRef<ReturnType<typeof setTimeout>>();
  const saveNow = React.useCallback(async (): Promise<string | undefined> => {
    clearTimeout(saveTimer.current);
    if (!dirty.current) return draftRef.current.id;
    setSaving(true);
    try {
      const { id, serverSaved, serverError } = await api.draftSave(build());
      dirty.current = false;
      setServerWarn(serverSaved ? null : serverError ?? "The mail server didn't accept the draft.");
      setDraft((d) => ({ ...d, id }));
      draftRef.current = { ...draftRef.current, id };
      setSavedAt(Date.now());
      return id;
    } catch (e) {
      notify.error(e, "Couldn't save the draft");
      return undefined;
    } finally {
      setSaving(false);
    }
  }, [build]);
  const scheduleSave = React.useCallback(() => {
    clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => void saveNow(), 4000);
  }, [saveNow]);
  React.useEffect(() => () => clearTimeout(saveTimer.current), []);

  const patch = (p: Partial<DraftInput>) => {
    setDraft((d) => ({ ...d, ...p }));
    dirty.current = true;
    scheduleSave();
    if (p.to && toError) setToError(undefined);
  };

  const hasContent = () => {
    const t = (editor?.getText() ?? "").trim();
    return !!(t || draftRef.current.to.length || draftRef.current.subject.trim() || draftRef.current.attachments.length);
  };

  const close = async () => {
    if (hasContent() && dirty.current) {
      const id = await saveNow();
      if (id) notify.ok("Draft saved");
    }
    closeComposer(c.key);
  };

  const discard = async () => {
    clearTimeout(saveTimer.current);
    if (draftRef.current.id) {
      try { await api.draftDelete(draftRef.current.id); } catch (e) { notify.error(e, "Couldn't delete the saved draft"); }
    }
    closeComposer(c.key);
    notify.ok("Draft discarded");
  };

  const doSend = async (sendAt?: number) => {
    setSending(true);
    clearTimeout(saveTimer.current);
    try {
      const d = { ...build(), sendAt };
      const item = await api.send(d);
      closeComposer(c.key);
      if (sendAt) {
        notify.ok(`Scheduled for ${format(new Date(sendAt), "EEE d MMM, HH:mm")}`);
      } else {
        const secs = settings?.undoSendSeconds ?? 0;
        if (secs > 0) {
          notify.info(`Sending in ${secs}s…`, {
            duration: secs * 1000,
            action: {
              label: "Undo",
              onClick: () => {
                api.outboxCancel(item.id)
                  .then((back) => {
                    const parts = splitDraftHtml(back.html);
                    openComposer({ mode: "draft", draft: { ...back, html: parts.body }, quoteHtml: parts.quote, quoteOnTop: parts.quoteOnTop, sourceId: c.sourceId });
                  })
                  .catch((e) => notify.error(e, "Too late to undo — the message has gone"));
              },
            },
          });
        }
      }
    } catch (e) {
      notify.error(e, "Couldn't send");
    } finally {
      setSending(false);
    }
  };

  const trySend = (sendAt?: number) => {
    const d = draftRef.current;
    if (!identity) { notify.error({ kind: "invalid", message: "Pick who the message is from." }, "Can't send yet"); return; }
    if (d.to.length + d.cc.length + d.bcc.length === 0) {
      setToError("Add at least one recipient");
      document.getElementById(`${c.key}-to`)?.focus();
      return;
    }
    const text = editor?.getText() ?? "";
    const checks: (() => boolean)[] = [];
    const run = (i: number) => {
      if (i >= checks.length) { void doSend(sendAt); return; }
      if (!checks[i]()) run(i + 1);
    };
    checks.push(() => {
      if (d.subject.trim()) return false;
      setConfirm({ title: "Send without a subject?", body: "Messages without a subject are easy to miss and often caught by spam filters.", label: "Send anyway", go: () => run(1) });
      return true;
    });
    checks.push(() => {
      if (d.attachments.length || !ATTACH_WORDS.test(text)) return false;
      setConfirm({ title: "Forgot the attachment?", body: "Your message mentions an attachment, but nothing is attached.", label: "Send without it", go: () => run(2) });
      return true;
    });
    run(0);
  };

  const addFiles = async () => {
    const picked = await openFileDialog({ multiple: true, title: "Attach files" });
    if (!picked) return;
    const paths = (Array.isArray(picked) ? picked : [picked]).map(String);
    const meta = { ...attachMeta };
    for (const p of paths) meta[p] = { name: p.split(/[\\/]/).pop() || p };
    setAttachMeta(meta);
    patch({ attachments: [...draftRef.current.attachments, ...paths.map((path) => ({ path }))] });
  };

  const onDropFiles = (e: React.DragEvent) => {
    // Tauri delivers OS file drops as a window event with paths (see App.tsx `wm:files-dropped`); the DOM drop has none.
    e.preventDefault();
  };
  React.useEffect(() => {
    const h = (e: Event) => {
      const paths = (e as CustomEvent<string[]>).detail;
      if (!paths?.length) return;
      setAttachMeta((m) => { const n = { ...m }; for (const p of paths) n[p] = { name: p.split(/[\\/]/).pop() || p }; return n; });
      patch({ attachments: [...draftRef.current.attachments, ...paths.map((path) => ({ path }))] });
    };
    window.addEventListener("wm:files-dropped", h);
    return () => window.removeEventListener("wm:files-dropped", h);
  }); // eslint-disable-line react-hooks/exhaustive-deps

  const insertBooking = async () => {
    try {
      const links = await api.iwBookingLinks();
      if (!links.length) { notify.info("You have no booking pages in IntelligentWolf yet."); return; }
      const l = links[0];
      editor?.chain().focus().insertContent(`<p><a href="${escapeHtml(l.url)}">${escapeHtml(l.title)}</a></p>`).run();
    } catch (e) { notify.error(e, "Couldn't get your booking link"); }
  };

  const draftWithAi = async () => {
    if (!c.sourceId) return;
    try {
      notify.info(`${assistant} is drafting a reply…`, { id: "ai-draft" });
      const r = await api.iwAi("draftReply", c.sourceId, editor?.getText() || undefined);
      notify.dismiss("ai-draft");
      editor?.chain().focus().setContent(r.text.split(/\n{2,}/).map((p) => `<p>${escapeHtml(p).replace(/\n/g, "<br>")}</p>`).join("")).run();
      dirty.current = true;
      scheduleSave();
      notify.ok(`Draft written · ${r.creditsUsed} credits`);
    } catch (e) { notify.dismiss("ai-draft"); notify.error(e, `${assistant} couldn't draft a reply`); }
  };

  const expanded = !!c.expanded;
  const identities = allIdentities();
  const title = draft.subject || (c.mode === "new" ? "New message" : c.mode === "forward" ? "Forward" : c.mode === "draft" ? "Draft" : "Reply");

  const quoteBlock = c.quoteHtml ? (
    <details className="group mx-4 mb-3 rounded-lg bg-surface-inset ring-1 ring-border" open={!c.quoteOnTop}>
      <summary className="cursor-pointer select-none px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground">
        {c.mode === "forward" ? "Forwarded message" : "Quoted message"} <span className="group-open:hidden">· show</span>
      </summary>
      <div className="px-2 pb-2"><MessageBody html={c.quoteHtml} remote={false} dark={false} /></div>
    </details>
  ) : null;

  return (
    <div
      role="dialog"
      aria-modal="false"
      aria-labelledby={`${c.key}-title`}
      onDragOver={(e) => e.preventDefault()}
      onDrop={onDropFiles}
      onKeyDown={(e) => {
        if ((e.ctrlKey || e.metaKey) && e.key === "Enter") { e.preventDefault(); trySend(); }
        if (e.key === "Escape" && !e.defaultPrevented) { e.preventDefault(); updateComposer(c.key, { minimized: true }); }
        if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") { e.preventDefault(); dirty.current = true; void saveNow().then((id) => id && notify.ok("Draft saved")); }
      }}
      className={cn(
        "fixed z-40 flex flex-col overflow-hidden bg-popover shadow-[var(--shadow-pop)] ring-1 ring-border",
        expanded ? "inset-x-[6vw] bottom-[5vh] top-[5vh] rounded-card" : "bottom-0 right-4 h-[min(640px,calc(100vh-24px))] w-[660px] max-w-[calc(100vw-32px)] rounded-t-card",
      )}
    >
      <div className="flex h-11 items-center gap-1 bg-plum pl-4 pr-1.5 text-white">
        <h2 id={`${c.key}-title`} className="min-w-0 flex-1 truncate text-[13px] font-semibold">{title}</h2>
        <span className="mr-2 text-[11px] opacity-70" aria-live="polite" title={serverWarn ?? undefined}>
          {saving ? "Saving…" : savedAt ? (serverWarn ? `Saved on this computer only · ${format(savedAt, "HH:mm")}` : `Saved ${format(savedAt, "HH:mm")}`) : ""}
        </span>
        <button type="button" aria-label="Minimise" onClick={() => updateComposer(c.key, { minimized: true })} className="flex size-8 items-center justify-center rounded opacity-80 hover:bg-white/10 hover:opacity-100"><Minus className="size-4" /></button>
        <button type="button" aria-label={expanded ? "Shrink" : "Expand"} onClick={() => updateComposer(c.key, { expanded: !expanded })} className="flex size-8 items-center justify-center rounded opacity-80 hover:bg-white/10 hover:opacity-100">
          {expanded ? <Minimize2 className="size-4" /> : <Maximize2 className="size-4" />}
        </button>
        <button type="button" aria-label="Close (saves a draft)" onClick={close} className="flex size-8 items-center justify-center rounded opacity-80 hover:bg-white/10 hover:opacity-100"><X className="size-4" /></button>
      </div>

      <div className="flex min-h-10 items-center gap-2 border-b border-border px-4">
        <label htmlFor={`${c.key}-from`} className="w-12 shrink-0 text-[13px] text-muted-foreground">From</label>
        <select
          id={`${c.key}-from`}
          value={draft.identityId}
          onChange={(e) => patch({ identityId: e.target.value })}
          className="h-8 min-w-0 flex-1 cursor-pointer appearance-none bg-transparent text-[13px] outline-none"
        >
          {identities.length === 0 && <option value="">Add an account first</option>}
          {accounts.filter((a) => a.enabled).map((a) => (
            <optgroup key={a.id} label={a.name}>
              {a.identities.map((i: Identity) => <option key={i.id} value={i.id}>{i.name ? `${i.name} <${i.email}>` : i.email}</option>)}
            </optgroup>
          ))}
        </select>
        {identity && <span aria-hidden className="size-2.5 rounded-full" style={{ background: accounts.find((a) => a.id === identity.accountId)?.color }} />}
      </div>

      <AddressField
        id={`${c.key}-to`} label="To" value={draft.to} onChange={(to) => patch({ to })}
        autoFocus={c.mode === "new" || c.mode === "forward"} invalid={!!toError} describedBy={toError ? `${c.key}-to-error` : undefined}
        trailing={
          <div className="mt-1 flex shrink-0 gap-1 text-xs">
            {!showCc && <button type="button" className="rounded px-1.5 py-1 text-muted-foreground hover:bg-accent hover:text-foreground" onClick={() => setShowCc(true)}>Cc</button>}
            {!showBcc && <button type="button" className="rounded px-1.5 py-1 text-muted-foreground hover:bg-accent hover:text-foreground" onClick={() => setShowBcc(true)}>Bcc</button>}
          </div>
        }
      />
      {toError && <p id={`${c.key}-to-error`} role="alert" className="border-b border-border bg-[var(--iw-red-wash)] px-4 py-1 text-xs font-medium text-[var(--iw-red-text)]">{toError}</p>}
      {showCc && <AddressField id={`${c.key}-cc`} label="Cc" value={draft.cc} onChange={(cc) => patch({ cc })} autoFocus={draft.cc.length === 0} />}
      {showBcc && <AddressField id={`${c.key}-bcc`} label="Bcc" value={draft.bcc} onChange={(bcc) => patch({ bcc })} autoFocus={draft.bcc.length === 0} />}
      <div className="flex min-h-10 items-center gap-2 border-b border-border px-4">
        <label htmlFor={`${c.key}-subject`} className="w-12 shrink-0 text-[13px] text-muted-foreground">Subject</label>
        <input id={`${c.key}-subject`} value={draft.subject} onChange={(e) => patch({ subject: e.target.value })} className="h-9 min-w-0 flex-1 bg-transparent text-[13.5px] font-medium outline-none" />
      </div>

      <EditorToolbar editor={editor} />

      {/* overflow-y-scroll: the quoted message is sized to its width like the reader's, so the same scrollbar flicker (Reader.tsx). */}
      <div className="min-h-0 flex-1 overflow-y-scroll">
        {!c.quoteOnTop && quoteBlock}
        <div className="wm-editor selectable px-4 pt-3" onClick={() => editor?.commands.focus()}>
          <EditorContent editor={editor} />
        </div>
        {sig && (
          <div className="mx-4 mb-3 mt-1 rounded-lg ring-1 ring-dashed ring-border">
            <div className="px-3 pt-1.5 text-[11px] uppercase tracking-wide text-muted-foreground">Signature · {identity?.email}</div>
            <div className="px-2 pb-1"><MessageBody html={sig} remote={false} dark={false} /></div>
          </div>
        )}
        {c.quoteOnTop && quoteBlock}
        {draft.attachments.length > 0 && (
          <ul role="list" aria-label="Attachments" className="flex flex-wrap gap-2 px-4 pb-3">
            {draft.attachments.map((a, i) => {
              const name = a.path ? attachMeta[a.path]?.name ?? a.path.split(/[\\/]/).pop() : `Attachment ${i + 1} from the original`;
              return (
                <li key={(a.path ?? a.fromMessage?.attachmentId ?? "") + i} className="flex items-center gap-2 rounded-lg bg-surface-raised py-1 pl-2.5 pr-1 text-[12.5px] ring-1 ring-border">
                  <FileText className="size-4 text-[var(--iw-red-text)]" />
                  <span className="max-w-[220px] truncate">{name}</span>
                  <button type="button" aria-label={`Remove ${name}`} className="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
                    onClick={() => patch({ attachments: draftRef.current.attachments.filter((_, j) => j !== i) })}><X className="size-3.5" /></button>
                </li>
              );
            })}
          </ul>
        )}
      </div>

      <div className="flex items-center gap-1.5 border-t border-border px-3 py-2.5">
        <div className="flex">
          <Button onClick={() => trySend()} disabled={sending} className="rounded-r-none pr-3">
            {sending ? <Spinner /> : <Send />} Send
          </Button>
          <Menu.Root>
            <Menu.Trigger asChild>
              <Button aria-label="More send options" className="rounded-l-none border-l border-white/20 px-2" disabled={sending}><ChevronDown /></Button>
            </Menu.Trigger>
            <Menu.Content align="start" side="top">
              <Menu.Label>Send later</Menu.Label>
              {laterPresets().map((p) => (
                <Menu.Item key={p.label} onSelect={() => trySend(p.at.getTime())}><Clock /> {p.label}<span className="ml-auto pl-4 text-[11px] text-muted-foreground">{format(p.at, "EEE HH:mm")}</span></Menu.Item>
              ))}
              <Menu.Item onSelect={() => setLaterOpen(true)}><CalendarClock /> Pick a date and time…</Menu.Item>
            </Menu.Content>
          </Menu.Root>
        </div>
        <span className="ml-1 hidden text-[11px] text-muted-foreground sm:inline">{modKey}+Enter</span>
        <div className="flex-1" />
        {iwConnected && c.sourceId && (
          <IconButton label={`Draft a reply with ${assistant} (uses IntelligentWolf credits)`} side="top" onClick={draftWithAi}><Sparkles /></IconButton>
        )}
        {iwConnected && <IconButton label="Insert my booking link" side="top" onClick={insertBooking}><CalendarPlus /></IconButton>}
        <IconButton label="Attach files" side="top" onClick={addFiles}><Paperclip /></IconButton>
        <IconButton label="Discard draft" side="top" onClick={() => (hasContent() ? setDiscardOpen(true) : void discard())}><Trash2 /></IconButton>
      </div>

      {confirm && (
        <ConfirmDialog open onOpenChange={(o) => !o && setConfirm(null)} title={confirm.title} body={confirm.body} confirmLabel={confirm.label}
          onConfirm={() => { const go = confirm.go; setConfirm(null); go(); }} />
      )}
      <ConfirmDialog open={discardOpen} onOpenChange={setDiscardOpen} title="Discard this draft?" body="The message and its saved draft are deleted." confirmLabel="Discard" destructive onConfirm={discard} />
      <SendLaterDialog open={laterOpen} onClose={() => setLaterOpen(false)} onPick={(ms) => { setLaterOpen(false); trySend(ms); }} />
    </div>
  );
}

function laterPresets() {
  const now = new Date();
  const inHour = addHours(startOfHour(now), 2);
  const tomorrow8 = setMinutes(setHours(addDays(now, 1), 8), 0);
  const monday8 = setMinutes(setHours(nextMonday(now), 8), 0);
  const p = [{ label: "In an hour or so", at: inHour }, { label: "Tomorrow morning", at: tomorrow8 }];
  if (now.getDay() !== 0) p.push({ label: "Monday morning", at: monday8 });
  return p;
}

function SendLaterDialog({ open, onClose, onPick }: { open: boolean; onClose: () => void; onPick: (ms: number) => void }) {
  const [val, setVal] = React.useState("");
  const [err, setErr] = React.useState<string>();
  React.useEffect(() => { if (open) { setVal(format(addHours(new Date(), 1), "yyyy-MM-dd'T'HH:mm")); setErr(undefined); } }, [open]);
  const submit = () => {
    const t = new Date(val).getTime();
    if (!val || Number.isNaN(t)) { setErr("Enter a date and time"); return; }
    if (t < Date.now() + 60_000) { setErr("Pick a time at least a minute from now"); return; }
    onPick(t);
  };
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()} title="Send later"
      description="WolfMail sends it at this time. Your computer needs to be on and WolfMail running (it can be in the tray)."
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button onClick={submit}><CalendarClock /> Schedule</Button></>}>
      <Field id="send-later-at" label="Date and time" error={err}>
        <Input type="datetime-local" value={val} onChange={(e) => setVal(e.target.value)} />
      </Field>
    </Dialog>
  );
}

function EditorToolbar({ editor }: { editor: Editor | null }) {
  const [, force] = React.useReducer((x: number) => x + 1, 0);
  React.useEffect(() => {
    if (!editor) return;
    editor.on("transaction", force);
    return () => { editor.off("transaction", force); };
  }, [editor]);
  if (!editor) return null;
  const b = (label: string, icon: React.ReactNode, active: boolean, run: () => void, shortcut?: string) => (
    <IconButton label={label} shortcut={shortcut} size="iconSm" aria-pressed={active} onMouseDown={(e) => e.preventDefault()} onClick={run}
      className={cn(active && "bg-select-strong text-[var(--iw-red-text)]")}>{icon}</IconButton>
  );
  return (
    <div role="toolbar" aria-label="Formatting" className="flex flex-wrap items-center gap-0.5 border-b border-border px-3 py-1">
      {b("Bold", <Bold />, editor.isActive("bold"), () => editor.chain().focus().toggleBold().run(), `${modKey}+B`)}
      {b("Italic", <Italic />, editor.isActive("italic"), () => editor.chain().focus().toggleItalic().run(), `${modKey}+I`)}
      {b("Underline", <Underline />, editor.isActive("underline"), () => editor.chain().focus().toggleUnderline().run(), `${modKey}+U`)}
      {b("Strikethrough", <Strikethrough />, editor.isActive("strike"), () => editor.chain().focus().toggleStrike().run())}
      <span className="mx-1 h-4 w-px bg-border" />
      {b("Bulleted list", <List />, editor.isActive("bulletList"), () => editor.chain().focus().toggleBulletList().run())}
      {b("Numbered list", <ListOrdered />, editor.isActive("orderedList"), () => editor.chain().focus().toggleOrderedList().run())}
      {b("Quote", <Quote />, editor.isActive("blockquote"), () => editor.chain().focus().toggleBlockquote().run())}
      {b("Align left", <AlignLeft />, editor.isActive({ textAlign: "left" }), () => editor.chain().focus().setTextAlign("left").run())}
      {b("Centre", <AlignCenter />, editor.isActive({ textAlign: "center" }), () => editor.chain().focus().setTextAlign("center").run())}
      <span className="mx-1 h-4 w-px bg-border" />
      <LinkButton editor={editor} />
      {b("Clear formatting", <RemoveFormatting />, false, () => editor.chain().focus().unsetAllMarks().clearNodes().run())}
    </div>
  );
}

function LinkButton({ editor }: { editor: Editor }) {
  const [open, setOpen] = React.useState(false);
  const [url, setUrl] = React.useState("");
  const [err, setErr] = React.useState<string>();
  const apply = () => {
    const u = url.trim();
    if (!u) { editor.chain().focus().extendMarkRange("link").unsetLink().run(); setOpen(false); return; }
    const href = /^(https?:|mailto:)/i.test(u) ? u : /@/.test(u) ? `mailto:${u}` : `https://${u}`;
    try { new URL(href); } catch { setErr("That isn't a web address"); return; }
    if (editor.state.selection.empty) editor.chain().focus().insertContent(`<a href="${escapeHtml(href)}">${escapeHtml(u)}</a>`).run();
    else editor.chain().focus().extendMarkRange("link").setLink({ href }).run();
    setOpen(false);
  };
  return (
    <Popover.Root open={open} onOpenChange={(o) => { setOpen(o); if (o) { setUrl(editor.getAttributes("link").href ?? ""); setErr(undefined); } }}>
      <Popover.Trigger asChild>
        <IconButton label="Link" size="iconSm" onMouseDown={(e) => e.preventDefault()} className={cn(editor.isActive("link") && "bg-select-strong text-[var(--iw-red-text)]")}><Link2 /></IconButton>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content sideOffset={6} className="z-[80] w-80 rounded-table bg-popover p-3 shadow-[var(--shadow-pop)] ring-1 ring-border">
          <form onSubmit={(e) => { e.preventDefault(); apply(); }} className="space-y-2">
            <Field id="link-url" label="Link address" error={err}>
              <Input autoFocus value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://…" />
            </Field>
            <div className="flex justify-end gap-2">
              <Button size="sm" variant="outline" onClick={() => setOpen(false)}>Cancel</Button>
              <Button size="sm" type="submit">{url.trim() ? "Apply" : "Remove link"}</Button>
            </div>
          </form>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

