import * as React from "react";
import { MessageSquarePlus, Send, X, Mail, Link2 } from "lucide-react";
import { api } from "../../lib/api";
import { notify } from "../../lib/notify";
import { selectOpenRow, setUi, useStore } from "../../lib/store";
import type { IwChatMessage } from "../../lib/types";
import { cn } from "../../lib/utils";
import { Button, IconButton } from "../../components/ui/button";
import { Spinner } from "../../components/ui/misc";
import { invalidatePeople } from "../iw/people";

/**
 * The little wolf in the bottom-left corner — Alex from IntelligentWolf, called whatever the user likes ("Alex" by
 * default; Settings → IntelligentWolf). Paul, 28 Oct 2026: "a little wolf (see how intelligent wolf does Alex) called
 * Alex (the user can change them) that sits in the bottom left corner and works like Alex".
 *
 * Who and what: someone in their inbox wants to ask their IntelligentWolf assistant about an email ("who is this?",
 * "draft a reply", "make a task for Friday") without leaving WolfMail. Flow: click the wolf → the chat opens above him
 * with their conversation → type, optionally with the open email attached (shown as a chip they can remove) → his reply
 * arrives in the chat; if the chat was closed meanwhile, the wolf shows a badge.
 *
 * He is the same wolf as on the IntelligentWolf floor (sprites from IntelligentWolf public/office/sprites, drawing and
 * motion from src/app/components/call/AlexWolf.tsx and layout/wolfRoam.ts): a slow bob and a blink while idle, a short
 * wander along the bottom of the sidebar, a soft ring while he thinks, his mouth moving as a reply lands. Each message
 * runs as a turn on IntelligentWolf (POST /wolfmail/chat) that WolfMail polls until he has answered.
 */

const SIZE = 56;
/** Source: IntelligentWolf src/app/components/call/AlexWolf.tsx WALK_FRAMES / WALK_FRAME_MS. */
const WALK_FRAMES = 4;
const WALK_FRAME_MS = 160;
/** How far he wanders from his corner, and how fast (px/s; IntelligentWolf AlexOrbHost uses orbPx * 0.9). */
const ROAM_PX = 40;
const PACE = SIZE * 0.9;
/** Poll a running turn every 2 s, for up to the server's turn timeout (php-api/lib/wolfmail.php WOLFMAIL_TURN_TIMEOUT 600 s). */
const POLL_MS = 2000;
const POLL_FOR_MS = 600_000;

type Mode = "idle" | "listening" | "speaking";
type Action = "walk" | "hop" | "sit" | null;
type Facing = "down" | "left" | "right";

const walkSrc = (dir: "left" | "right", i: number) => `/wolf/alex-walk-${dir}-${i}.png`;
const FACING_SRC: Record<Facing, string> = { down: "/wolf/wolf-down.png", left: "/wolf/wolf-left.png", right: "/wolf/wolf-right.png" };
const BLINK_SRC = "/wolf/alex-blink-down.png";
const TALK_SRC = "/wolf/alex-talk-down.png";

const held: HTMLImageElement[] = [];
function warmFrames() {
  if (held.length) return;
  for (const p of [...Object.values(FACING_SRC), BLINK_SRC, TALK_SRC, ...[0, 1, 2, 3].flatMap((i) => [walkSrc("left", i), walkSrc("right", i)])]) {
    const img = new Image(); img.src = p; held.push(img);
  }
}

function useReducedMotion(): boolean {
  const [reduce, setReduce] = React.useState(() => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false);
  React.useEffect(() => {
    const mq = window.matchMedia?.("(prefers-reduced-motion: reduce)");
    if (!mq) return;
    const on = () => setReduce(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return reduce;
}

function useTicker(on: boolean, ms: number, n: number): number {
  const [i, setI] = React.useState(0);
  React.useEffect(() => {
    if (!on) { setI(0); return; }
    const t = window.setInterval(() => setI((x) => (x + 1) % n), ms);
    return () => window.clearInterval(t);
  }, [on, ms, n]);
  return i;
}

/** True for ~150 ms every 3–6 s while `on` (Source: AlexWolf.tsx useBlink). */
function useBlink(on: boolean): boolean {
  const [shut, setShut] = React.useState(false);
  React.useEffect(() => {
    if (!on) { setShut(false); return; }
    let open: number | undefined;
    let next: number | undefined;
    const schedule = () => {
      next = window.setTimeout(() => { setShut(true); open = window.setTimeout(() => { setShut(false); schedule(); }, 150); }, 3000 + Math.random() * 3000);
    };
    schedule();
    return () => { window.clearTimeout(next); window.clearTimeout(open); };
  }, [on]);
  return shut;
}

function Wolf({ mode, facing, action }: { mode: Mode; facing: Facing; action: Action }) {
  React.useEffect(warmFrames, []);
  const reduce = useReducedMotion();
  const walking = action === "walk" && facing !== "down" && !reduce;
  const frame = useTicker(walking, WALK_FRAME_MS, WALK_FRAMES);
  // His mouth while a reply lands: open/closed on a 140 ms beat (there is no audio level in text chat).
  const mouth = useTicker(mode === "speaking", 140, 2);
  const blinking = useBlink(mode !== "speaking" && facing === "down" && !walking);
  const src = walking ? walkSrc(facing as "left" | "right", frame)
    : mode === "speaking" && facing === "down" && mouth === 1 ? TALK_SRC
    : blinking ? BLINK_SRC
    : FACING_SRC[facing];
  const motion = action === "hop" ? "wm-wolf-hop" : action === "sit" ? "wm-wolf-sit" : action === "walk" ? "" : "wm-wolf-idle";
  return (
    <span className="relative flex h-full w-full items-center justify-center">
      {mode === "listening" && <span aria-hidden className="wm-wolf-listen absolute inset-[6%] rounded-full bg-[var(--iw-red)]/30" />}
      <span className={cn("relative flex h-full w-full origin-bottom items-center justify-center", motion)}>
        <img src={src} alt="" draggable={false} className="h-[84%] w-auto max-w-[84%] object-contain" style={{ imageRendering: "pixelated" }} />
      </span>
    </span>
  );
}

/** A short wander along the bottom of the sidebar while nothing is happening (Source: IntelligentWolf layout/wolfRoam.ts
 *  planWolf — walk, hop, sit, rest). Stops while he is hovered, focused or busy; never under reduced motion. */
function useRoam(enabled: boolean) {
  const reduce = useReducedMotion();
  const [x, setX] = React.useState(0);
  const [facing, setFacing] = React.useState<Facing>("down");
  const [action, setAction] = React.useState<Action>(null);
  const xRef = React.useRef(0);
  React.useEffect(() => {
    if (!enabled || reduce) {
      // Hovered, focused or busy: stop where he is and face you. He must not jump away from the pointer that is about
      // to click him (the next wander takes him home); only reduced motion puts him straight back in his corner.
      setAction(null); setFacing("down");
      if (reduce) { xRef.current = 0; setX(0); }
      return;
    }
    let timer: number | undefined;
    let raf: number | undefined;
    let stopped = false;
    const rest = (ms: number) => { setAction(null); setFacing("down"); timer = window.setTimeout(step, ms); };
    const walkTo = (target: number) => {
      const from = xRef.current;
      const dir: Facing = target > from ? "right" : "left";
      const dur = (Math.abs(target - from) / PACE) * 1000;
      const t0 = performance.now();
      setFacing(dir); setAction("walk");
      const tick = (t: number) => {
        if (stopped) return;
        const k = Math.min(1, (t - t0) / dur);
        xRef.current = from + (target - from) * k;
        setX(xRef.current);
        if (k < 1) raf = requestAnimationFrame(tick); else rest(2500 + Math.random() * 4000);
      };
      raf = requestAnimationFrame(tick);
    };
    const step = () => {
      if (stopped) return;
      const r = Math.random();
      if (r < 0.45) {
        const target = xRef.current > ROAM_PX / 2 ? Math.random() * 20 : ROAM_PX * (0.4 + Math.random() * 0.6);
        walkTo(target);
      } else if (r < 0.6) { setFacing("down"); setAction("hop"); timer = window.setTimeout(() => rest(3000 + Math.random() * 4000), 1100); }
      else if (r < 0.8) { setFacing("down"); setAction("sit"); timer = window.setTimeout(() => rest(2000 + Math.random() * 3000), 6400); }
      else rest(5000 + Math.random() * 5000);
    };
    timer = window.setTimeout(step, 6000 + Math.random() * 6000);
    return () => { stopped = true; window.clearTimeout(timer); if (raf) cancelAnimationFrame(raf); };
  }, [enabled, reduce]);
  return { x, facing, action };
}

export function Assistant() {
  const show = useStore((s) => s.settings?.showAssistant ?? true);
  const name = useStore((s) => s.settings?.assistantName || "Alex");
  const connected = useStore((s) => !!s.iw?.connected);
  const openRow = useStore(selectOpenRow);
  const [open, setOpen] = React.useState(false);
  const [hover, setHover] = React.useState(false);
  const [messages, setMessages] = React.useState<IwChatMessage[]>([]);
  const [historyLoaded, setHistoryLoaded] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [speaking, setSpeaking] = React.useState(false);
  const [unread, setUnread] = React.useState(false);
  const [draft, setDraft] = React.useState("");
  const [shareEmail, setShareEmail] = React.useState(true);
  const [fresh, setFresh] = React.useState(false);
  const [error, setError] = React.useState<string>();
  const buttonRef = React.useRef<HTMLButtonElement>(null);
  const inputRef = React.useRef<HTMLTextAreaElement>(null);
  const endRef = React.useRef<HTMLDivElement>(null);
  const openRef = React.useRef(open);
  openRef.current = open;

  // Connecting or disconnecting changes who is a contact; and a new workspace is a new conversation.
  React.useEffect(() => { invalidatePeople(); setMessages([]); setHistoryLoaded(false); }, [connected]);

  React.useEffect(() => {
    if (!open || !connected || historyLoaded) return;
    setHistoryLoaded(true);
    api.iwChatHistory().then((h) => setMessages((cur) => (cur.length ? cur : h.messages))).catch((e) => setError((e as { message?: string }).message ?? String(e)));
  }, [open, connected, historyLoaded]);
  React.useEffect(() => { endRef.current?.scrollIntoView({ block: "end" }); }, [messages, busy, open]);
  React.useEffect(() => { if (open) { setUnread(false); inputRef.current?.focus(); } }, [open]);
  // A new email opened: offer it again.
  React.useEffect(() => { setShareEmail(true); }, [openRow?.id]);

  const roam = useRoam(show && !open && !hover && !busy && !speaking && !unread);

  const close = React.useCallback(() => { setOpen(false); buttonRef.current?.focus(); }, []);
  React.useEffect(() => {
    if (!open) return;
    const h = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); close(); } };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [open, close]);

  const send = async () => {
    const text = draft.trim();
    if (!text || busy) return;
    setError(undefined);
    setDraft("");
    setMessages((m) => [...m, { role: "user", text, at: Date.now() }]);
    setBusy(true);
    try {
      let t = await api.iwChatSend(text, shareEmail && openRow ? openRow.id : undefined, fresh);
      setFresh(false);
      const until = Date.now() + POLL_FOR_MS;
      while (t.status === "busy" && Date.now() < until) {
        await new Promise((r) => window.setTimeout(r, POLL_MS));
        t = await api.iwChatPoll(t.turn);
      }
      if (t.status === "busy") throw { kind: "timeout", message: `${name} is taking too long — the answer will be in your IntelligentWolf chat when it is ready.` };
      if (t.reply) {
        setMessages((m) => [...m, { role: "assistant", text: t.reply!, at: Date.now() }]);
        setSpeaking(true);
        window.setTimeout(() => setSpeaking(false), Math.min(4000, 800 + t.reply.length * 25));
        if (!openRef.current) setUnread(true);
      }
      if (t.status === "failed") throw { kind: "server", message: t.error ?? `${name} couldn't answer.` };
    } catch (e) {
      const msg = (e as { message?: string }).message ?? String(e);
      setError(msg);
      if (!openRef.current) notify.error(e, `${name} couldn't answer`);
    } finally { setBusy(false); }
  };

  if (!show) return null;
  const mode: Mode = busy ? "listening" : speaking ? "speaking" : "idle";
  const label = busy ? `${name} is thinking` : unread ? `${name} replied — open the chat` : open ? `Hide ${name}` : `Talk to ${name}`;

  return (
    <>
      {open && (
        <section role="dialog" aria-modal="false" aria-labelledby="wm-assistant-title"
          className="fixed bottom-[72px] left-3 z-40 flex h-[min(560px,calc(100vh-96px))] w-[min(380px,calc(100vw-24px))] flex-col overflow-hidden rounded-2xl bg-surface shadow-theme-lg ring-1 ring-border">
          <header className="flex items-center gap-2.5 bg-[var(--iw-red-wash)] px-3.5 py-2.5">
            <span className="relative size-9 shrink-0 rounded-full bg-surface ring-1 ring-border">
              <img src="/wolf/wolf-down.png" alt="" className="absolute inset-0 m-auto h-[80%]" style={{ imageRendering: "pixelated" }} />
              <span aria-hidden className={cn("absolute -bottom-0.5 -right-0.5 size-2.5 rounded-full ring-2 ring-surface", connected ? "bg-[var(--iw-go)]" : "bg-muted-foreground")} />
            </span>
            <div className="min-w-0 flex-1">
              <h2 id="wm-assistant-title" className="font-title text-[15px] leading-tight">{name}</h2>
              <p className="truncate text-[11.5px] text-muted-foreground">{connected ? "Your IntelligentWolf assistant" : "Not connected to IntelligentWolf"}</p>
            </div>
            {connected && (
              <IconButton label="New conversation" size="iconSm" onClick={() => { setMessages([]); setFresh(true); setError(undefined); inputRef.current?.focus(); }}>
                <MessageSquarePlus />
              </IconButton>
            )}
            <IconButton label={`Hide ${name}`} size="iconSm" onClick={close}><X /></IconButton>
          </header>

          {!connected ? (
            <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
              <img src="/wolf/wolf-down.png" alt="" className="h-16" style={{ imageRendering: "pixelated" }} />
              <p className="text-[13.5px] font-semibold">Hi, I'm {name}.</p>
              <p className="text-[13px] text-muted-foreground">
                Connect WolfMail to IntelligentWolf and I can tell you who's writing, update your contacts, draft replies and make tasks — right here in your inbox.
              </p>
              <Button onClick={() => { setUi({ iwConnect: true }); close(); }}><Link2 /> Connect IntelligentWolf</Button>
            </div>
          ) : (
            <>
              <div className="min-h-0 flex-1 space-y-2.5 overflow-y-auto px-3.5 py-3" aria-live="polite" aria-relevant="additions">
                {messages.length === 0 && !busy && (
                  <p className="pt-6 text-center text-[13px] text-muted-foreground">Ask me about an email, a customer, or what to do next.</p>
                )}
                {messages.map((m, i) => (
                  <div key={i} className={cn("flex", m.role === "user" ? "justify-end" : "justify-start")}>
                    <p className={cn("selectable max-w-[85%] whitespace-pre-wrap rounded-2xl px-3 py-2 text-[13px] leading-relaxed",
                      m.role === "user" ? "bg-[var(--iw-red)] text-white" : "bg-surface-raised ring-1 ring-border")}>
                      <span className="sr-only">{m.role === "user" ? "You: " : `${name}: `}</span>{m.text}
                    </p>
                  </div>
                ))}
                {busy && (
                  <p role="status" className="flex items-center gap-2 text-[12.5px] text-muted-foreground"><Spinner /> {name} is thinking…</p>
                )}
                {error && <p role="alert" className="rounded-lg bg-[var(--iw-red-wash)] p-2.5 text-[12.5px] text-[var(--iw-red-text)]">{error}</p>}
                <div ref={endRef} />
              </div>
              <form className="space-y-2 border-t border-border p-3" onSubmit={(e) => { e.preventDefault(); void send(); }}>
                {openRow && shareEmail && (
                  <span className="inline-flex max-w-full items-center gap-1.5 rounded-full bg-surface-raised py-0.5 pl-2 pr-0.5 text-[11.5px] ring-1 ring-border">
                    <Mail aria-hidden className="size-3.5 shrink-0" />
                    <span className="truncate">With this email: {openRow.subject || "(no subject)"}</span>
                    <button type="button" aria-label="Don't send the open email" onClick={() => setShareEmail(false)}
                      className="flex size-5 shrink-0 items-center justify-center rounded-full hover:bg-accent"><X className="size-3" /></button>
                  </span>
                )}
                {openRow && !shareEmail && (
                  <button type="button" onClick={() => setShareEmail(true)} className="text-[11.5px] font-medium text-[var(--iw-red-text)] hover:underline">
                    + Include the open email
                  </button>
                )}
                <div className="flex items-end gap-2">
                  <label htmlFor="wm-assistant-input" className="sr-only">Message {name}</label>
                  <textarea id="wm-assistant-input" ref={inputRef} rows={2} value={draft} onChange={(e) => setDraft(e.target.value)}
                    onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); void send(); } }}
                    placeholder={`Message ${name}…`} maxLength={8000}
                    className="min-h-[40px] flex-1 resize-none rounded-xl bg-input-background px-3 py-2 text-[13px] ring-1 ring-border outline-none focus-visible:ring-2 focus-visible:ring-[var(--iw-red)]" />
                  <IconButton label="Send" type="submit" disabled={busy || !draft.trim()}><Send /></IconButton>
                </div>
                <p className="text-[10.5px] text-muted-foreground">Uses your IntelligentWolf credits. Only the email shown above is sent with your message.</p>
              </form>
            </>
          )}
        </section>
      )}

      <button ref={buttonRef} type="button" aria-label={label} title={label} aria-expanded={open}
        onClick={() => (open ? close() : setOpen(true))}
        onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)} onFocus={() => setHover(true)} onBlur={() => setHover(false)}
        className="fixed bottom-1 left-2 z-40 rounded-full outline-none focus-visible:ring-2 focus-visible:ring-[var(--iw-red)]"
        style={{ width: SIZE, height: SIZE, transform: `translateX(${roam.x}px)` }}>
        <Wolf mode={mode} facing={roam.facing} action={roam.action} />
        {unread && <span aria-hidden className="absolute right-0.5 top-0.5 size-3 rounded-full bg-[var(--iw-red)] ring-2 ring-surface" />}
      </button>
    </>
  );
}
