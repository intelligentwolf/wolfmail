import { getState } from "../../lib/store";
import type { Address, DraftInput, Identity, MessageFull } from "../../lib/types";
import { escapeHtml, formatAddress, fullDate, stripReplyPrefix } from "../../lib/utils";

// A composer holds three parts and only the first is edited in the rich-text editor:
//   body   — what the person types (Tiptap)
//   sig    — the identity's signature, raw HTML, swapped when the From identity changes
//   quote  — the quoted/forwarded original, raw HTML, so a table or styled newsletter survives untouched
// joinDraftHtml() assembles them for send/save; splitDraftHtml() takes a saved draft apart again using the two
// markers below, so drafts round-trip through the server's Drafts folder.

export const SIG_ATTR = "data-wm-signature";
export const QUOTE_CLASS = "wm-quote";

export type DraftParts = { draft: DraftInput; quoteHtml?: string; quoteOnTop: boolean };

export function allIdentities(): Identity[] {
  return getState().accounts.filter((a) => a.enabled).flatMap((a) => a.identities);
}

export function identityById(id: string): Identity | undefined {
  return getState().accounts.flatMap((a) => a.identities).find((i) => i.id === id);
}

/** Default identity for a brand-new message: the account currently being viewed, else the first enabled account. */
export function defaultIdentity(): Identity | undefined {
  const s = getState();
  const accountId = s.view.kind === "folder" ? s.folders.find((f) => f.id === (s.view as { folderId: string }).folderId)?.accountId : undefined;
  const acct = s.accounts.find((a) => a.id === accountId && a.enabled) ?? s.accounts.find((a) => a.enabled);
  return acct?.identities.find((i) => i.isDefault) ?? acct?.identities[0];
}

/** Reply identity: the identity the original was addressed to (mail to sales@ is answered from sales@), else the
 *  receiving account's default. */
function replyIdentity(msg: MessageFull): Identity | undefined {
  const s = getState();
  const acct = s.accounts.find((a) => a.id === msg.accountId);
  const addressed = new Set([...msg.to, ...msg.cc, ...msg.bcc].map((a) => a.email.toLowerCase()));
  return (
    acct?.identities.find((i) => addressed.has(i.email.toLowerCase())) ??
    allIdentities().find((i) => addressed.has(i.email.toLowerCase())) ??
    acct?.identities.find((i) => i.isDefault) ??
    acct?.identities[0]
  );
}

export function signatureHtml(identity?: Identity): string {
  if (!identity?.signatureHtml?.trim()) return "";
  return `<div ${SIG_ATTR}="1">-- <br>${identity.signatureHtml}</div>`;
}

export function joinDraftHtml(body: string, sig: string, quote: string | undefined, quoteOnTop: boolean): string {
  if (!quote) return body + sig;
  return quoteOnTop ? body + sig + quote : quote + body + sig;
}

/** Inverse of joinDraftHtml for a draft that comes back from the engine. */
export function splitDraftHtml(html: string): { body: string; quote?: string; quoteOnTop: boolean } {
  const doc = new DOMParser().parseFromString(`<div id="wm-root">${html}</div>`, "text/html");
  const root = doc.getElementById("wm-root")!;
  root.querySelectorAll(`[${SIG_ATTR}]`).forEach((n) => n.remove());
  const marker = root.querySelector(`.${QUOTE_CLASS}`);
  if (!marker) return { body: root.innerHTML, quoteOnTop: true };
  // Top-level ancestor of the marker inside root.
  let top: Element = marker;
  while (top.parentElement && top.parentElement !== root) top = top.parentElement;
  const kids = Array.from(root.childNodes);
  const at = kids.indexOf(top);
  const before = kids.slice(0, at);
  const ser = (ns: ChildNode[]) => ns.map((n) => (n.nodeType === 1 ? (n as Element).outerHTML : escapeHtml(n.textContent ?? ""))).join("");
  if (at > 0) {
    return { body: ser(before), quote: ser(kids.slice(at)), quoteOnTop: true };
  }
  // Bottom-posting: marker first; the quote is the marker plus the blockquote that follows it.
  const quoteEnd = kids.findIndex((n, i) => i > at && n.nodeType === 1 && (n as Element).tagName === "BLOCKQUOTE");
  const end = quoteEnd >= 0 ? quoteEnd + 1 : at + 1;
  return { quote: ser(kids.slice(0, end)), body: ser(kids.slice(end)), quoteOnTop: false };
}

/** Plain-text alternative for the multipart/alternative part. */
export function htmlToText(html: string): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  doc.querySelectorAll("blockquote").forEach((bq) => {
    const t = (bq.textContent ?? "").split("\n").map((l) => `> ${l}`).join("\n");
    bq.replaceWith(doc.createTextNode(`\n${t}\n`));
  });
  doc.querySelectorAll("br").forEach((br) => br.replaceWith(doc.createTextNode("\n")));
  doc.querySelectorAll("p, div, li, h1, h2, h3, tr").forEach((el) => el.append(doc.createTextNode("\n")));
  doc.querySelectorAll("a[href]").forEach((a) => {
    const href = a.getAttribute("href") ?? "";
    if (href && href !== a.textContent && /^https?:/.test(href)) a.append(doc.createTextNode(` <${href}>`));
  });
  return (doc.body.textContent ?? "").replace(/\n{3,}/g, "\n\n").trim();
}

export function newDraftFor(to: Address[] = []): DraftInput {
  const id = defaultIdentity();
  return { identityId: id?.id ?? "", to, cc: [], bcc: [], subject: "", html: "<p></p>", text: "", references: [], attachments: [] };
}

function quoteBody(msg: MessageFull): string {
  if (msg.html) return msg.html;
  return `<p>${escapeHtml(msg.text ?? "").replace(/\n/g, "<br>")}</p>`;
}

function prefixed(prefix: "Re" | "Fwd", subject: string) {
  return `${prefix}: ${stripReplyPrefix(subject)}`;
}

export function replyDraft(msg: MessageFull, all: boolean): DraftParts {
  const id = replyIdentity(msg);
  const mine = new Set(allIdentities().map((i) => i.email.toLowerCase()));
  const sentByMe = mine.has(msg.from.email.toLowerCase());
  // Replying to your own sent message goes back to its original recipients, as Thunderbird and Gmail do.
  const to: Address[] = sentByMe ? [...msg.to] : msg.replyTo.length ? [...msg.replyTo] : [msg.from];
  const cc: Address[] = [];
  if (all) {
    const seen = new Set(to.map((a) => a.email.toLowerCase()));
    const add = (list: Address[], into: Address[]) => {
      for (const a of list) {
        const k = a.email.toLowerCase();
        if (seen.has(k) || mine.has(k)) continue;
        seen.add(k);
        into.push(a);
      }
    };
    if (!sentByMe) add(msg.to, to);
    add(msg.cc, cc);
  }
  const header = `On ${escapeHtml(fullDate(msg.date))}, ${escapeHtml(formatAddress(msg.from))} wrote:`;
  return {
    quoteHtml: `<p class="${QUOTE_CLASS}">${header}</p><blockquote type="cite">${quoteBody(msg)}</blockquote>`,
    quoteOnTop: id?.replyOnTop ?? true,
    draft: {
      identityId: id?.id ?? "",
      to, cc, bcc: [],
      subject: prefixed("Re", msg.subject),
      html: "<p></p>", text: "",
      inReplyTo: msg.messageId,
      references: [...msg.references, ...(msg.messageId ? [msg.messageId] : [])].slice(-20),
      replyToMessageId: msg.id,
      attachments: [],
    },
  };
}

export function forwardDraft(msg: MessageFull): DraftParts {
  const id = replyIdentity(msg);
  const rows: [string, string][] = [
    ["Subject", msg.subject],
    ["Date", fullDate(msg.date)],
    ["From", formatAddress(msg.from)],
    ["To", msg.to.map(formatAddress).join(", ")],
  ];
  if (msg.cc.length) rows.push(["Cc", msg.cc.map(formatAddress).join(", ")]);
  const head = rows.map(([k, v]) => `<b>${k}:</b> ${escapeHtml(v)}`).join("<br>");
  return {
    quoteHtml: `<p class="${QUOTE_CLASS}">-------- Forwarded message --------<br>${head}</p><blockquote type="cite">${quoteBody(msg)}</blockquote>`,
    quoteOnTop: true,
    draft: {
      identityId: id?.id ?? "",
      to: [], cc: [], bcc: [],
      subject: prefixed("Fwd", msg.subject),
      html: "<p></p>", text: "",
      references: msg.messageId ? [msg.messageId] : [],
      forwardOfMessageId: msg.id,
      attachments: msg.attachments.filter((a) => !a.inline).map((a) => ({ fromMessage: { messageId: msg.id, attachmentId: a.id } })),
    },
  };
}
