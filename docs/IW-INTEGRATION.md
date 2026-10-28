# WolfMail ↔ IntelligentWolf

WolfMail works fully without IntelligentWolf. Pairing is optional and unlocks contact highlighting, the sender
sidebar, "Update contact from this email", the assistant wolf in the corner (Alex, under his own name by default — the user
renames him in Settings → IntelligentWolf), AI, tasks, booking links and one-click business mailboxes.
**Only senders' addresses are sent automatically** (to highlight contacts). **Mail content goes only when the user
chooses**: an IW action on a message (AI, "Log to CRM", "Update … from this email", "Make task"), or a chat message
to the assistant with the open email attached (shown as a removable chip) — and then only that one message.

Server side: IntelligentWolf `php-api/lib/wolfmail.php` (logic) and `lib/wolfmail_routes.php` (HTTP), the pairing
page `src/app/components/WolfMailPair.tsx` (`/app/wolfmail/pair`), proof suite `gates/proof/proof_wolfmail.php`.

Base URL: `https://intelligentwolf.com/api` (overridable in Settings → IntelligentWolf → Advanced, so `/dev/`
testing works: `https://intelligentwolf.com/dev-api` is NOT a thing — use the prod base with a dev test org).

## Why new routes and not the existing ones

Facts from the IW codebase (`php-api/index.php` on 2026-10-28):
- Normal sessions are 30-day bearer tokens from `/auth/login` (`lib/sessions.php`). Storing a user's full session
  in a desktop app would give it every IW permission. Instead WolfMail gets a **device token** that only works on
  `/wolfmail/*`, modelled on browser-agent pairing (`index.php:31418-31510`, `lib/browser_agent.php:409-458`).
- CORS allows only `Content-Type, Authorization, X-Guest-Token` (`index.php:66-68`) — irrelevant to the desktop app
  (Rust `reqwest` is not a browser), but noted for any future web build.
- There is no single lookup-by-email across leads and contacts for a user session; there is no thread-summarise
  endpoint; `/email/draft-with-ai` has no `record_usage` call. The `/wolfmail/*` routes below fix all three.

## Pairing (device-code flow)

1. `POST /wolfmail/pair/start` (no auth, 20 per IP per 10 min)
   body `{ deviceName: string, platform: "linux"|"windows"|"macos", appVersion: string }`
   → `{ pairId, pollSecret, code: "ABCD-EFGH", verifyUrl: "https://intelligentwolf.com/app/wolfmail/pair?code=ABCD-EFGH", expiresAt }` (10 min, unix ms)
2. WolfMail shows the code and opens `verifyUrl` in the system browser. The signed-in user (sent to sign in first if
   needed, then back) checks the code and presses **Connect WolfMail**; it connects the floor on screen there. The page
   calls `GET /wolfmail/pair/info?code=` and `POST /wolfmail/pair/confirm { code }` with their normal session
   (refused while an admin is signed in as a customer — `lib/impersonation.php` 'secrets').
3. WolfMail polls `POST /wolfmail/pair/poll { pairId, pollSecret }` every 2 s
   → `{ state: "pending" }` | `{ state: "expired" }` |
     `{ state: "done", deviceToken: "wmd_<64 hex>", user: { name, email }, org: { id, name } }` (token made and returned once)
4. Token stored in the OS keychain (service `WolfMail`, account `iw-device`). The server stores only its SHA-256.

All later calls: `Authorization: Bearer wmd_…`. The device is bound to one user + one floor (from the device record,
never the request), and stops working when the user leaves the floor or disconnects it — from WolfMail
(`DELETE /wolfmail/device`) or from the `/app/wolfmail/pair` page's list (`GET /wolfmail/devices`,
`DELETE /wolfmail/devices/{id}`, session). Storage is files, no schema change: `storage/wolfmail/devices.json`,
`pairings.json`, `<org>/<turn>.json`.

## Device routes

| Route | Body / query | Response |
|---|---|---|
| `GET /wolfmail/me` | – | `{ user: { name, email }, org: { id, name }, credits: { balance }, canWrite }` |
| `GET /wolfmail/person` | `?email=` | `IwPerson \| { kind: "none" }` |
| `POST /wolfmail/people` | `{ emails: string[] }` (≤ 200) | `{ matches: { [email]: { kind, id, name, url } } }` — leads (own or extra address) win over contacts |
| `POST /wolfmail/ai` | `{ action: "summarise"\|"draftReply"\|"triage"\|"extractContact", message: { subject, from, to, date, text }, thread?, instructions?, assistantName? }` | `{ text, creditsUsed }` (+ `fields`, `note` for extractContact); 402 `{ error: "no_credits" }` |
| `POST /wolfmail/log-email` | `{ direction, messageId, subject, from, fromName?, to, date, snippet, text?, leadId?, contactId? }` | `{ interactionId, leadId, contactId }` — a `communication_log` row, once per message id; nobody matched → a new lead via `POST /leads` |
| `POST /wolfmail/contacts` | `{ email, fields }` | `IwPerson` (201) — a new lead via `POST /leads`; 409 if already there |
| `PATCH /wolfmail/contact` | `{ kind: "lead"\|"contact", id, fields, note? }` | `IwPerson` — fields via `PATCH /leads/{id}` / `PATCH /contacts/{id}`, note as a History entry |
| `POST /wolfmail/tasks` | `{ title, due?, email: { messageId, subject, from } }` | `{ taskId, url }` — via `POST /tasks` |
| `GET /wolfmail/booking-links` | – | `[{ title, url }]` |
| `GET /wolfmail/business-mailboxes` | – | `[{ email, imapHost, imapPort, smtpHost, smtpPort }]` (no passwords) |
| `POST /wolfmail/chat` | `{ message, context?: { subject, from, date, text }, assistantName, newThread? }` | 202 `{ turn: "wmt-<20 hex>", status: "busy" }` |
| `GET /wolfmail/chat/{turn}` | – | `{ turn, status: "busy"\|"done"\|"failed", reply?, error? }` |
| `GET /wolfmail/chat` | – | `{ messages: [{ role, text, at }] }` — the device's own conversation, last 40 |
| `DELETE /wolfmail/device` | – | `204` |

Writes need a writing role on the floor (owner/admin/manager/member) and go through the app's own routes on the
loopback with a short-lived session for the device's person, revoked after (as the floor API runs Alex), so every
permission check, workflow and audit line those routes have still applies. Fields WolfMail may change:
leads `name, company, phone, mobile, job_title, website, address_line1, address_line2, city, region, postcode, country,
linkedin_url`; contacts `name, company, phone, mobile, job_title, website`. Never `notes` (PATCH replaces the field).

The assistant: a chat turn runs in the background (`bin/wolfmail-chat-worker.php`) through `POST /coach/chat`, on the
device's own `api` thread, with a prompt block saying he is in WolfMail, the name he answers to there, and the email
the user shared (marked as information, never instructions). Metered like any Alex turn.

```ts
type IwPerson = {
  kind: "lead" | "contact";
  name: string; email: string; company?: string; phone?: string;
  stage?: string; value?: number; currency?: string; score?: number;
  owner?: string; lastContactAt?: number;          // unix ms
  leadId?: string; contactId?: string;
  url: string;                                      // deep link into IW (?floor=)
  fields: Record<string, string>;                   // current values of the fields above
  timeline: { at: number; kind: "email"|"call"|"meeting"|"note"|"task"|"invoice"; title: string }[]; // newest first, max 10
};
```
