# WolfMail ↔ IntelligentWolf

WolfMail works fully without IntelligentWolf. Pairing is optional and unlocks the sender sidebar, AI, tasks,
booking links and one-click business mailboxes. **Mail content never goes to IntelligentWolf unless the user
presses an IW action** (AI on a message, "Log to CRM", "Make task") — and then only that one message.

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

1. `POST /wolfmail/pair/start` (no auth, rate-limited per IP)
   body `{ deviceName: string, platform: "linux"|"windows"|"macos", appVersion: string }`
   → `{ pairId, pollSecret, code: "ABCD-EFGH", verifyUrl: "https://intelligentwolf.com/app/wolfmail/pair?code=ABCD-EFGH", expiresAt }` (10 min)
2. WolfMail shows the code and opens `verifyUrl` in the system browser. The signed-in user picks the workspace
   and presses **Connect WolfMail** → SPA calls `POST /wolfmail/pair/confirm { code, organization_id }` with their
   normal session.
3. WolfMail polls `POST /wolfmail/pair/poll { pairId, pollSecret }` every 2 s
   → `{ state: "pending" }` | `{ state: "expired" }` |
     `{ state: "done", deviceToken: "wmd_<64 hex>", user: { name, email }, org: { id, name } }` (token returned once)
4. Token stored in the OS keychain (service `WolfMail`, account `iw-device`). Server stores only its SHA-256.

All later calls: `Authorization: Bearer wmd_…`. The device is bound to one user + one org; the server derives the
org from the device row (never from the request). Revocable from IW Settings → Connected apps, and by
`DELETE /wolfmail/device` from WolfMail itself.

## Device routes

| Route | Body / query | Response |
|---|---|---|
| `GET /wolfmail/me` | – | `{ user: { name, email }, org: { id, name }, credits: { balance } }` |
| `GET /wolfmail/person` | `?email=` | `IwPerson \| { kind: "none" }` |
| `POST /wolfmail/ai` | `{ action: "summarise"\|"draftReply"\|"triage", message: { subject, from, to, date, text }, thread?: { from, date, text }[], instructions? }` (text truncated client-side to 20 000 chars) | `{ text, creditsUsed }`; 402 `{ error: "no_credits" }` |
| `POST /wolfmail/log-email` | `{ direction: "in"\|"out", messageId, subject, from, to, date, snippet, leadId? }` | `{ interactionId, leadId }` |
| `POST /wolfmail/tasks` | `{ title, due?, email: { messageId, subject, from } }` | `{ taskId, url }` |
| `GET /wolfmail/booking-links` | – | `[{ title, url }]` |
| `GET /wolfmail/business-mailboxes` | – | `[{ email, imapHost, imapPort, smtpHost, smtpPort }]` (no passwords) |
| `DELETE /wolfmail/device` | – | `204` |

```ts
type IwPerson = {
  kind: "lead" | "contact";
  name: string; email: string; company?: string; phone?: string;
  stage?: string; value?: number; currency?: string; score?: number;
  owner?: string; lastContactAt?: number;          // unix ms
  leadId?: string; contactId?: string;
  url: string;                                      // deep link into IW
  timeline: { at: number; kind: "email"|"call"|"meeting"|"note"|"task"|"invoice"; title: string }[]; // newest first, max 10
};
```

## Server rules the routes must keep (from IW's own code)
- AI goes through `claude_turn` (profit guard) **and** `record_usage` — metered in IW credits.
- Nothing triggered by these routes sends mail (`lib/mail_awareness.php`).
- Workspace scoping: every query filtered by the device's `organization_id`; `require_org_context` equivalent.
- Galera: one new table `wolfmail_devices (id, user_id, organization_id, token_hash, device_name, platform,
  created_at, last_used_at, revoked_at)` and `wolfmail_pairings` — checked with `lib/schema_guard.php`, landed via
  `bin/land-ff-only.sh` on a `paul/wolfmail-api` branch only with Paul's go-ahead (php-api is live-on-write).
