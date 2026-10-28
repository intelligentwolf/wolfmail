import * as React from "react";
import { Sparkles, X, Search, Paperclip, Link2 } from "lucide-react";
import { api } from "../../lib/api";
import { openSearchHit, setUi, useStore } from "../../lib/store";
import type { SearchHit } from "../../lib/types";
import { displayName, listDate } from "../../lib/utils";
import { Button, IconButton } from "../../components/ui/button";
import { Avatar, Spinner } from "../../components/ui/misc";

/**
 * Ask — find mail by describing it (IntelligentWolf ticket #423, Ian: "an AI-driven … pull-out panel on the right,
 * like Gemini").
 *
 * Who and what: someone who remembers roughly what an email was ("the quote Sarah sent last week") but not the words
 * in it wants it open in a few seconds without learning search syntax. Flow: open the panel (the ✨ Ask button over the
 * list, or the command palette) → type the question → the assistant turns ONLY the question into WolfMail's search
 * (php-api /wolfmail/search-query) → the search runs here over the user's own mail → click a result to open it. The
 * query is shown and editable, so a near miss is one tweak away, and works without IntelligentWolf too.
 * Opened by the `wm:ask` event (detail = an optional question to run at once).
 */
type Phase = { k: "idle" } | { k: "asking" } | { k: "searching" } | { k: "done"; hits: SearchHit[] } | { k: "error"; message: string };

export function AskPanel() {
  const [open, setOpen] = React.useState(false);
  const [question, setQuestion] = React.useState("");
  const [query, setQuery] = React.useState("");
  const [explanation, setExplanation] = React.useState("");
  const [phase, setPhase] = React.useState<Phase>({ k: "idle" });
  const connected = useStore((s) => !!s.iw?.connected);
  const assistant = useStore((s) => s.settings?.assistantName || "Alex");
  const inputRef = React.useRef<HTMLInputElement>(null);
  const returnFocus = React.useRef<HTMLElement | null>(null);

  const run = React.useCallback(async (q: string) => {
    const t = q.trim();
    if (!t) return;
    setPhase({ k: "searching" });
    try { setPhase({ k: "done", hits: await api.search(t, 50) }); }
    catch (e) { setPhase({ k: "error", message: (e as { message?: string }).message ?? String(e) }); }
  }, []);

  const ask = React.useCallback(async (text: string) => {
    const t = text.trim();
    if (!t) return;
    setPhase({ k: "asking" });
    setExplanation("");
    try {
      const d = new Date();
      const today = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
      const r = await api.iwSearchQuery(t, today);
      setQuery(r.query);
      setExplanation(r.explanation);
      await run(r.query);
    } catch (e) {
      setPhase({ k: "error", message: (e as { message?: string }).message ?? String(e) });
    }
  }, [run]);

  React.useEffect(() => {
    const h = (e: Event) => {
      returnFocus.current = document.activeElement as HTMLElement | null;
      setOpen(true);
      const q = (e as CustomEvent<string | null>).detail;
      if (q) { setQuestion(q); void ask(q); }
      window.setTimeout(() => inputRef.current?.focus(), 0);
    };
    window.addEventListener("wm:ask", h);
    return () => window.removeEventListener("wm:ask", h);
  }, [ask]);

  const close = React.useCallback(() => { setOpen(false); returnFocus.current?.focus?.(); }, []);
  React.useEffect(() => {
    if (!open) return;
    const h = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); close(); } };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [open, close]);

  if (!open) return null;
  const busy = phase.k === "asking" || phase.k === "searching";

  return (
    <section role="dialog" aria-modal="false" aria-labelledby="wm-ask-title"
      className="fixed inset-y-0 right-0 z-40 flex w-[min(440px,100vw)] flex-col border-l border-border bg-surface shadow-theme-lg">
      <header className="flex items-center gap-2 border-b border-border bg-[var(--iw-red-wash)] px-4 py-3">
        <Sparkles aria-hidden className="size-4 text-[var(--iw-red-text)]" />
        <h2 id="wm-ask-title" className="font-title text-[15px]">Ask {assistant}</h2>
        <span className="text-xs text-muted-foreground">find an email by describing it</span>
        <div className="flex-1" />
        <IconButton label="Close Ask" size="iconSm" onClick={close}><X /></IconButton>
      </header>

      <form className="space-y-2 border-b border-border p-4" onSubmit={(e) => { e.preventDefault(); void ask(question); }}>
        <label htmlFor="wm-ask-q" className="block text-[13px] font-medium">What are you looking for?</label>
        <div className="flex gap-2">
          <input id="wm-ask-q" ref={inputRef} value={question} onChange={(e) => setQuestion(e.target.value)} maxLength={500}
            placeholder="e.g. the quote Sarah sent last week" disabled={!connected}
            className="min-w-0 flex-1 rounded-lg bg-input-background px-3 py-2 text-[13px] ring-1 ring-border outline-none focus-visible:ring-2 focus-visible:ring-[var(--iw-red)] disabled:opacity-60" />
          <Button type="submit" disabled={!connected || busy || !question.trim()}>{phase.k === "asking" ? <Spinner /> : <Sparkles />} Ask</Button>
        </div>
        {!connected && (
          <p className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            Asking needs IntelligentWolf. You can still search below.
            <Button type="button" size="sm" variant="outline" onClick={() => { setUi({ iwConnect: true }); close(); }}><Link2 /> Connect</Button>
          </p>
        )}
        {connected && <p className="text-[11px] text-muted-foreground">Only your question goes to IntelligentWolf (it uses a credit). The search runs here, over your own mail.</p>}
      </form>

      <form className="space-y-1.5 border-b border-border px-4 py-3" onSubmit={(e) => { e.preventDefault(); void run(query); }}>
        <label htmlFor="wm-ask-query" className="block text-[12px] font-medium text-muted-foreground">Search {explanation ? `— ${explanation}` : ""}</label>
        <div className="flex gap-2">
          <input id="wm-ask-query" value={query} onChange={(e) => setQuery(e.target.value)} spellCheck={false}
            placeholder="from:sarah subject:quote after:2026-10-21"
            className="selectable min-w-0 flex-1 rounded-lg bg-surface-raised px-3 py-1.5 font-mono text-[12px] ring-1 ring-border outline-none focus-visible:ring-2 focus-visible:ring-[var(--iw-red)]" />
          <Button type="submit" size="sm" variant="outline" disabled={busy || !query.trim()}><Search /> Search</Button>
        </div>
      </form>

      <div className="min-h-0 flex-1 overflow-y-auto" aria-live="polite" aria-busy={busy}>
        {phase.k === "asking" && <p role="status" className="flex items-center gap-2 p-4 text-[13px] text-muted-foreground"><Spinner /> {assistant} is working out the search…</p>}
        {phase.k === "searching" && <p role="status" className="flex items-center gap-2 p-4 text-[13px] text-muted-foreground"><Spinner /> Searching your mail…</p>}
        {phase.k === "error" && <p role="alert" className="m-4 rounded-lg bg-[var(--iw-red-wash)] p-3 text-[13px] text-[var(--iw-red-text)]">{phase.message}</p>}
        {phase.k === "done" && phase.hits.length === 0 && (
          <div className="p-6 text-center text-[13px] text-muted-foreground">
            <p className="font-medium text-foreground">Nothing matched</p>
            <p className="mt-1">Try fewer words in the search above, or ask in a different way.</p>
          </div>
        )}
        {phase.k === "done" && phase.hits.length > 0 && (
          <>
            <p className="px-4 pt-3 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">{phase.hits.length === 50 ? "First 50 results" : `${phase.hits.length} result${phase.hits.length === 1 ? "" : "s"}`}</p>
            <ul role="list" className="divide-y divide-border/60">
              {phase.hits.map((h) => (
                <li key={h.id}>
                  <button type="button" onClick={() => void openSearchHit(h)}
                    className="flex w-full gap-3 px-4 py-3 text-left hover:bg-accent/70 focus-visible:bg-accent/70 focus-visible:outline-none">
                    <Avatar address={h.from} size={30} />
                    <span className="min-w-0 flex-1">
                      <span className="flex items-center gap-2">
                        <span className="min-w-0 flex-1 truncate text-[13px] font-medium">{displayName(h.from)}</span>
                        {h.hasAttachments && <Paperclip aria-label="Has attachments" className="size-3.5 text-muted-foreground" />}
                        <span className="shrink-0 text-[11.5px] tabular-nums text-muted-foreground">{listDate(h.date)}</span>
                      </span>
                      <span className="block truncate text-[13px]">{h.subject || "(no subject)"}</span>
                      <span className="block truncate text-[12px] text-muted-foreground">{h.highlight}</span>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </>
        )}
      </div>
    </section>
  );
}
