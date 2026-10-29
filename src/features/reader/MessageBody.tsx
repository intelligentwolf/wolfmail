import * as React from "react";
import { escapeHtml } from "../../lib/utils";

// The engine has already sanitised the HTML (ammonia: no scripts, forms, handlers; remote resources stripped unless
// the user allowed them). This is the second wall: a sandboxed iframe with NO allow-scripts, plus a CSP meta that
// forbids everything except inline styles and data:/blob: images (and https: images only when remote content was
// explicitly loaded). `allow-same-origin` without `allow-scripts` lets us measure the height while the message itself
// still cannot run anything (the parent measures it, and re-measures while images arrive). Links: `<base target="_blank">` makes each click a new-window request, which the app
// opens in the browser (or the composer, for mailto:) — src-tauri/src/lib.rs external_links(). WebKit runs no event
// listeners inside a script-less frame, so a click handler here would never fire.

function frameDoc(inner: string, opts: { remote: boolean; plain: boolean; dark: boolean }) {
  const img = opts.remote ? "data: blob: https:" : "data: blob:";
  const csp = `default-src 'none'; img-src ${img}; style-src 'unsafe-inline'; font-src data:; media-src data: blob:;`;
  const plainStyles = opts.plain
    ? `body{white-space:pre-wrap;word-wrap:break-word;font:14px/1.6 "Inter Variable",Inter,system-ui,sans-serif;color:${opts.dark ? "#eceff1" : "#17141a"};background:transparent}
       a{color:${opts.dark ? "#ff8a8a" : "#c41e1e"}}
       blockquote{margin:0 0 0 .2em;padding-left:.8em;border-left:3px solid ${opts.dark ? "rgba(224,230,230,.25)" : "rgba(45,27,45,.18)"};color:${opts.dark ? "#a9adb4" : "#6b6470"}}`
    : `body{font:14px/1.5 -apple-system,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;color:#17141a;background:#fff}
       img{max-width:100%;height:auto}
       blockquote[type=cite]{margin:0 0 0 .2em;padding-left:.8em;border-left:3px solid rgba(45,27,45,.18)}`;
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${csp}">
<meta name="color-scheme" content="${opts.plain && opts.dark ? "dark" : "light"}">
<base target="_blank"><style>:root{color-scheme:${opts.plain && opts.dark ? "dark" : "light"}}html,body{margin:0;padding:0}body{padding:4px 2px 12px;overflow-x:auto}
pre{white-space:pre-wrap}table{max-width:100%}${plainStyles}</style></head><body>${inner}</body></html>`;
}

export function textToHtml(text: string): string {
  // Quote levels ("> ") become nested blockquotes; URLs become links. Everything else is escaped.
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  let out = "";
  let depth = 0;
  for (const line of lines) {
    const m = line.match(/^((?:>\s?)+)/);
    const d = m ? (m[1].match(/>/g) || []).length : 0;
    while (depth < d) { out += "<blockquote>"; depth++; }
    while (depth > d) { out += "</blockquote>"; depth--; }
    const content = m ? line.slice(m[1].length) : line;
    out += escapeHtml(content).replace(/\b(https?:\/\/[^\s<>"']+[^\s<>"'.,;:!?)\]])/g, '<a href="$1">$1</a>') + "\n";
  }
  while (depth-- > 0) out += "</blockquote>";
  return out;
}

export function MessageBody({ html, text, plainText, remote, dark }: { html?: string; text?: string; plainText?: boolean; remote: boolean; dark: boolean }) {
  const ref = React.useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = React.useState(40);
  // Hidden until the first measurement, so the frame doesn't show at 40 px and then jump to the message's height.
  const [measured, setMeasured] = React.useState(false);
  const plain = plainText ?? !html;
  const doc = React.useMemo(
    () => frameDoc(plain ? (text != null ? textToHtml(text) : html ?? "") : html!, { remote, plain, dark }),
    [html, text, remote, plain, dark],
  );

  // Stops the image watch below when the frame reloads or the reader unmounts.
  const stopWatch = React.useRef<() => void>(() => {});
  React.useEffect(() => () => stopWatch.current(), []);
  // Should the frame's load event never arrive, show it anyway rather than leave the message invisible.
  React.useEffect(() => {
    const t = setTimeout(() => setMeasured(true), 1000);
    return () => clearTimeout(t);
  }, []);

  const onLoad = React.useCallback(() => {
    stopWatch.current();
    const f = ref.current;
    const d = f?.contentDocument;
    if (!f || !d) { setMeasured(true); return; }
    // Measure the body, not the document: documentElement.scrollHeight never drops below the frame's own height,
    // so a frame measured that way can grow but never shrink to fit a one-line message.
    const measure = () => {
      const b = d.body;
      if (!b) return;
      const st = d.defaultView?.getComputedStyle(b);
      const pad = st ? parseFloat(st.marginTop) + parseFloat(st.marginBottom) : 0;
      setHeight(Math.max(24, Math.ceil(b.getBoundingClientRect().height + pad)));
    };
    measure();
    setMeasured(true); // even when the body couldn't be measured: a frame at the old height beats an invisible one
    const ro = new ResizeObserver(measure);
    ro.observe(d.body);
    // Images finishing late change the height. Their load events never reach a listener inside this script-less frame
    // (see the header), so the parent looks: every 250 ms until every image is complete, for at most 20 s.
    const imgs = Array.from(d.images);
    let timer: number | undefined;
    if (imgs.length) {
      const until = Date.now() + 20_000;
      timer = window.setInterval(() => {
        measure();
        if (imgs.every((i) => i.complete) || Date.now() > until) { window.clearInterval(timer); measure(); }
      }, 250);
    }
    stopWatch.current = () => { ro.disconnect(); window.clearInterval(timer); };
  }, []);

  return (
    <iframe
      ref={ref}
      title="Message content"
      sandbox="allow-same-origin allow-popups allow-popups-to-escape-sandbox"
      srcDoc={doc}
      onLoad={onLoad}
      className={plain ? "w-full border-0 bg-transparent" : "w-full rounded-lg border-0 bg-white"}
      // The frame's color-scheme must match its document's: when they differ WebKit paints an opaque canvas behind
      // the content (white in a dark app), which made plain-text mail light-grey-on-white.
      style={{ height, colorScheme: plain && dark ? "dark" : "light", visibility: measured ? "visible" : "hidden" }}
    />
  );
}
