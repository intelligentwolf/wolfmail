# WolfMail engine ↔ UI contract

The React UI talks to the Rust engine only through Tauri commands (`invoke`) and Tauri events (`listen`).
Every command returns `Result<T, ApiError>`; the UI shows every `ApiError` visibly (see the toast rules in
`docs/UX.md`). The TypeScript mirror of these types lives in `src/lib/types.ts`; the Rust side in
`crates/wolfmail-core/src/model.rs`. **Field names are camelCase on the wire** (`#[serde(rename_all = "camelCase")]`).
Ids are strings (UUID v4 for accounts/identities/drafts/outbox; `i64` rowids rendered as strings for folders and
messages so JS never loses precision).

## Errors

```ts
type ApiError = {
  kind: "auth" | "network" | "tls" | "server" | "notFound" | "invalid" | "storage" | "oauthConfig" | "cancelled" | "internal";
  message: string;          // human sentence, safe to show (never contains a password or token)
  field?: string;           // for "invalid": the input field name the UI should mark aria-invalid
  accountId?: string;
};
```

## Types

```ts
type Security = "tls" | "starttls" | "none";            // "none" only allowed for localhost hosts
type AuthKind = "password" | "oauthGoogle" | "oauthMicrosoft";
type ServerConfig = { host: string; port: number; security: Security; username: string; auth: AuthKind };

type Identity = {
  id: string; accountId: string; name: string; email: string; replyTo?: string;
  signatureHtml: string; replyOnTop: boolean; isDefault: boolean;
};

type AccountStatus = "ok" | "syncing" | "authError" | "offline" | "error" | "disabled";
type Account = {
  id: string; name: string; email: string; color: string;       // color = hex, user-chosen
  provider: "imap" | "gmail" | "outlook";
  imap: ServerConfig; smtp: ServerConfig;
  identities: Identity[];
  bodyDays: number;           // full bodies cached for messages newer than N days (0 = headers only, -1 = everything)
  enabled: boolean; sortOrder: number;
  status: AccountStatus; statusMessage?: string; lastSyncAt?: number; // unix ms
};

type FolderRole = "inbox" | "sent" | "drafts" | "trash" | "junk" | "archive" | "all" | "flagged" | "outbox" | null;
type Folder = {
  id: string; accountId: string; path: string; name: string; parentId?: string;
  role: FolderRole; unread: number; total: number; selectable: boolean; depth: number;
};

type Address = { name?: string; email: string };
type Flags = { seen: boolean; flagged: boolean; answered: boolean; forwarded: boolean; draft: boolean };

type MessageSummary = {
  id: string; accountId: string; folderId: string; threadId: string; messageId?: string;
  subject: string; from: Address; to: Address[]; cc: Address[]; date: number; // unix ms
  snippet: string; flags: Flags; tags: string[]; hasAttachments: boolean; size: number;
  threadCount: number;        // messages in this thread inside the current view (1 when not threaded)
  threadUnread: number;
  person: boolean;      // written by a person: not bulk/machine mail (List-*, Precedence, Auto-Submitted, Feedback-ID, no-reply senders,
                        // Gmail's Promotions/Social/Updates/Forums, Outlook's "Other"), or from someone this account has written to
};

type Attachment = { id: string; filename: string; mime: string; size: number; inline: boolean; contentId?: string };

type MessageFull = MessageSummary & {
  bcc: Address[]; replyTo: Address[]; inReplyTo?: string; references: string[];
  html?: string;              // ALREADY SANITISED by the engine (ammonia); remote resources rewritten to blocked placeholders
  text?: string;
  plainText: boolean;              // no HTML part; `html` was generated from the text → render in the app theme
  remoteContentBlocked: boolean;   // true when the html had remote images/css that were removed
  trackersRemoved: number;         // count of 1x1 / known tracking pixels stripped
  attachments: Attachment[];
  listUnsubscribe?: { mailto?: string; http?: string; oneClick: boolean };
  authResults?: { spf?: string; dkim?: string; dmarc?: string };
};

type ViewQuery = {
  // exactly one of folderId / unified
  folderId?: string;
  unified?: "inbox" | "sent" | "drafts" | "trash" | "junk" | "archive" | "flagged" | "unread";
  filter?: { unread?: boolean; flagged?: boolean; attachments?: boolean; tag?: string; fromContact?: string };
  search?: string;            // quick-filter text; matches subject/from/to (and body FTS when cached)
  threaded: boolean;
  sort: "dateDesc" | "dateAsc";
  offset: number; limit: number;
  keepIds?: string[];         // unread-only lists (unified "unread" or filter.unread): ids already on screen stay
                              // listed after being read; moved/deleted rows still drop out. Ignored elsewhere.
};
type MessagePage = { total: number; items: MessageSummary[] };

type DraftInput = {
  id?: string;                // existing draft id when re-saving
  identityId: string;
  to: Address[]; cc: Address[]; bcc: Address[];
  subject: string; html: string; text: string;
  inReplyTo?: string; references: string[];
  replyToMessageId?: string;  // message being answered → set \Answered after send
  forwardOfMessageId?: string;
  attachments: { path?: string; fromMessage?: { messageId: string; attachmentId: string } }[];
  sendAt?: number;            // unix ms; omitted = send now (subject to undo delay)
};
type OutboxItem = { id: string; draft: DraftInput; state: "waiting" | "sending" | "sent" | "failed"; sendAt: number; error?: string };

type SearchHit = MessageSummary & { highlight: string };

type Settings = {
  theme: "system" | "light" | "dark";
  density: "comfortable" | "compact";
  listLayout: "multiline" | "table";
  readingPane: "right" | "bottom" | "off";
  threaded: boolean;
  undoSendSeconds: number;    // 0–30
  closeToTray: boolean; startInTray: boolean;
  notifications: boolean;
  loadRemoteContent: "never" | "contacts" | "always";
  markReadAfterMs: number;    // -1 = never automatically
  iwBaseUrl: string;          // default "https://intelligentwolf.com/api"; must be https (http only to 127.0.0.1)
  showAssistant: boolean;     // default true: the assistant wolf in the bottom-left corner (mail view)
  assistantName: string;      // default "Alex"; 1–24 letters, spaces, hyphens, apostrophes (iw::valid_assistant_name)
};

type ImportCandidate = {
  source: "betterbird" | "thunderbird"; profilePath: string;
  accounts: { name: string; email: string; provider: "imap" | "gmail" | "outlook"; imap: ServerConfig; smtp: ServerConfig;
              identities: Omit<Identity, "id" | "accountId">[]; folderCount: number; localSizeBytes: number }[];
  tags: { key: string; name: string; color: string }[];
};
```

## Commands

| Command | Args | Returns |
|---|---|---|
| `accounts_list` | – | `Account[]` |
| `account_autodetect` | `{ email }` | `{ provider, imap: ServerConfig, smtp: ServerConfig, source: "builtin"\|"ispdb"\|"autoconfig"\|"srv"\|"guess" }` |
| `account_test` | `{ imap, smtp, password? }` | `{ imapOk, smtpOk, imapError?: ApiError, smtpError?: ApiError }` |
| `account_add` | `{ name, email, displayName, provider, imap, smtp, password?, color? }` | `Account` (OAuth providers: run `oauth_authorize` first, pass nothing for password) |
| `account_update` | `{ id, patch: Partial<Account>, password? }` | `Account` |
| `account_remove` | `{ id }` | `null` (deletes local cache + keychain secrets) |
| `accounts_reorder` | `{ ids }` | `null` |
| `identity_upsert` | `Identity` (id optional) | `Identity` |
| `identity_remove` | `{ id }` | `null` |
| `oauth_cancel` | – | `null` — stops a waiting browser sign-in (it then fails with kind `cancelled`) |
| `oauth_authorize` | `{ provider: "google"\|"microsoft", email }` | `{ email }` — opens system browser, PKCE + loopback redirect, stores refresh token in keychain under the email |
| `folders_list` | `{ accountId? }` | `Folder[]` |
| `folder_create` | `{ accountId, parentId?, name }` | `Folder` |
| `folder_rename` | `{ id, name }` | `Folder` |
| `folder_delete` | `{ id }` | `null` |
| `folder_mark_read` | `{ id }` | `null` |
| `folder_empty` | `{ id }` | `number` — messages stored here; the empty runs IN THE BACKGROUND and reports `folder://empty` `{ folderId, name, role, state: "running" \| "done" \| "failed", done?, total?, count?, message? }`. Trash and Junk only; `invalid` for any other folder, for one already being emptied, and for a Gmail-API account (gmail.modify cannot erase; Gmail empties them after 30 days). |
| `messages_list` | `ViewQuery` | `MessagePage` |
| `thread_get` | `{ threadId, accountId }` | `MessageSummary[]` (oldest first) |
| `message_get` | `{ id }` | `MessageFull` (fetches body from server if not cached) |
| `message_load_remote` | `{ id }` | `MessageFull` (html sanitised but with remote images allowed for this one message) |
| `message_raw` | `{ id }` | `string` (RFC 5322 source) |
| `messages_set_flags` | `{ ids, add: (keyof Flags)[], remove: (keyof Flags)[] }` | `null` |
| `messages_set_tags` | `{ ids, add: string[], remove: string[] }` | `null` |
| `messages_move` | `{ ids, folderId }` | `null` |
| `messages_copy` | `{ ids, folderId }` | `null` |
| `messages_archive` | `{ ids }` | `null` (moves to the account's archive role folder; creates "Archive" if none) |
| `messages_trash` | `{ ids }` | `null` (to Trash; from Trash = permanent delete) |
| `messages_junk` | `{ ids, junk: boolean }` | `null` |
| `messages_undo` | `{ token }` | `null` (every move/trash/archive returns via event `undoable` a token valid 5 minutes; the UI keeps them for Ctrl/⌘+Z and the menus — no toast) |
| `attachment_save` | `{ messageId, attachmentId, path }` | `null` |
| `attachment_open` | `{ messageId, attachmentId }` | `null` (writes to app cache dir, opens with system handler) |
| `draft_save` | `DraftInput` | `{ id, serverSaved, serverError? }` — always stored locally first; `serverSaved=false` (with the reason) when the APPEND to the server's Drafts folder failed — the local copy is safe and the next save retries. The new server copy replaces the previous one. |
| `draft_get` | `{ id }` | `DraftInput` — `id` may be a local draft id OR the message id of a row in a Drafts folder; a message id is adopted as a local draft (attachments copied out to the cache) and the returned `id` is that local draft, so a later `draft_save`/`send` replaces the server copy (deleted after the new APPEND succeeds / after send). |
| `draft_delete` | `{ id }` | `null` — local draft id (also removes its server copy) or Drafts-folder message id |
| `send` | `DraftInput` | `OutboxItem` (queued; sent after `undoSendSeconds` or at `sendAt`) |
| `outbox_list` | – | `OutboxItem[]` |
| `outbox_cancel` | `{ id }` | `DraftInput` (returns it so the UI can reopen the composer). Waiting or failed items only; no event is emitted. |
| `search` | `{ query, accountId?, limit? }` (limit default 50) | `SearchHit[]` (FTS5 over cached headers+bodies; supports `from:` `to:` `cc:` `subject:` `has:attachment` `is:unread`/`read`/`starred` `tag:` `after:YYYY-MM-DD` (on or after) `before:YYYY-MM-DD` (before); the same syntax filters the list via `ViewQuery.search`) |
| `contacts_suggest` | `{ prefix, limit? }` (limit default 8) | `Address[]` (from address book + addresses seen in mail, ranked by frequency/recency) |
| `sync_now` | `{ accountId? }` | `null` |
| `unsubscribe` | `{ messageId }` | `{ method: "oneClick"\|"mailto"\|"browser" }` |
| `tags_list` | – | `{ key, name, color }[]` |
| `tag_upsert` | `{ key?, name, color }` | `{ key, name, color }` |
| `settings_get` | – | `Settings` |
| `settings_set` | `Partial<Settings>` | `Settings` |
| `import_scan` | `{ profilePath? }` | `ImportCandidate[]` (auto-finds Betterbird/Thunderbird profiles incl. Flatpak paths) |
| `import_apply` | `{ profilePath, emails: string[] }` | `Account[]` (password accounts are created with status `authError` until a password is entered) |
| `iw_status` | – | `{ connected: boolean, user?: { name, email }, org?: { id, name }, baseUrl }` |
| `iw_pair_start` | `{ baseUrl? }` | `{ code, verifyUrl, expiresAt }` |
| `iw_pair_poll` | – | `{ state: "pending"\|"done"\|"expired" }` |
| `iw_disconnect` | – | `null` |
| `iw_person` | `{ email }` | `IwPerson \| null` (see `docs/IW-INTEGRATION.md`) |
| `iw_ai` | `{ action: "summarise"\|"draftReply"\|"triage"\|"extractContact", messageId, instructions? }` | `{ text, creditsUsed }`; extractContact adds `{ fields, note }` (the sender's details found in the email) |
| `iw_log_email` | `{ messageId, leadId?, contactId?, includeText? }` | `{ leadId, contactId }` — `includeText` sends the whole text, not just the snippet |
| `iw_search_query` | `{ question, today: "YYYY-MM-DD" }` | `{ query, explanation, creditsUsed }` — Ask: only the question is sent; the query is then run locally with `search` |
| `iw_people` | `{ emails: string[] }` | `{ [email]: { kind, id, name, url } }` — only addresses are sent; batched by 200 |
| `iw_contact_update` | `{ kind: "lead"\|"contact", id, fields, note? }` | `IwPerson` |
| `iw_contact_create` | `{ email, fields }` | `IwPerson` (a new lead) |
| `iw_chat_send` | `{ message, messageId?, newThread? }` | `{ turn, status: "busy" }` — with `messageId` the open email goes too; the assistant's name comes from settings |
| `iw_chat_poll` | `{ turn }` | `{ turn, status: "busy"\|"done"\|"failed", reply?, error? }` |
| `iw_chat_history` | – | `{ messages: { role, text, at }[] }` |
| `iw_create_task` | `{ messageId, title, due? }` | `{ taskId }` |
| `iw_booking_link` | – | `{ url, title }[]` |
| `iw_business_mailboxes` | – | `{ email, imapHost, imapPort, smtpHost, smtpPort }[]` (no passwords) |

## Secrets (keychain or encrypted vault)

On start the app probes the OS credential store (Keychain / Credential Manager / Secret Service). If it is unusable —
typically Linux without a Secret Service daemon — or a vault already exists, secrets go to `secrets.vault` in the data
dir: Argon2id-derived key, XChaCha20-Poly1305, primary password (see `crates/wolfmail-core/src/secrets.rs`).

| Command | Args | Returns |
|---|---|---|
| `secrets_status` | – | `{ backend: "keychain"\|"file", initialized, locked, reason? }` |
| `secrets_create` | `{ password }` | same — first run of the file backend; ≥ 8 chars; refuses if a vault exists |
| `secrets_unlock` | `{ password }` | same — wrong password → `ApiError { kind: "auth", field: "password" }`; on success triggers a sync |
| `overview_window_open` | – | `null` — opens/focuses the "All inboxes" window |

## Events (engine → UI)

| Event | Payload |
|---|---|
| `sync://status` | `{ accountId, status: AccountStatus, message?, progress?: { done, total } }` |
| `mail://changed` | `{ folderIds: string[] }` — UI re-queries the visible view if it intersects |
| `mail://new` | `{ accountId, folderId, messages: MessageSummary[] }` — drives notifications + tray badge |
| `outbox://changed` | `OutboxItem` |
| `oauth://pending` | `{ provider: "google"\|"microsoft", email, url }` — browser sign-in started; the UI shows the link with Open again / Copy / Cancel |
| `oauth://done` | `{ email, ok }` — the sign-in finished, failed or was cancelled |
| `link://mailto` | `string` (the mailto: URL) — a mailto link clicked in a message; sent only to the window it was clicked in, which opens a composer |
| `link://failed` | `string` (message) — a web link clicked in a message could not be handed to the default browser |
| `undoable` | `{ token, label, expiresAt }` |
| `counts://changed` | `{ unreadInbox: number }` (unified, enabled accounts) |

## Engine notes (behaviour the UI can rely on)

- `send`: after SMTP accepts, a copy is APPENDed to Sent for `provider: "imap"` accounts only — Gmail and Microsoft 365
  file submitted mail in Sent themselves. Replies set `\Answered`, forwards `$Forwarded` on the original.
- Outbox retries transient failures (network, SMTP 4xx) after 1, 2, 5, 10, 30, 60, 120, 240 minutes with the reason in
  `error`, then `failed`. Auth and 5xx failures go straight to `failed`. A crash while `sending` resets to `waiting`.
- An account whose sign-in is refused goes to `authError` and is NOT retried until the user changes the password
  (`account_update` with `password`) or signs in again (`oauth_authorize`) — repeated bad logins lock accounts.
- OAuth needs build-time `WOLFMAIL_GOOGLE_CLIENT_ID` (+ `WOLFMAIL_GOOGLE_CLIENT_SECRET` for Google "Desktop app"
  credentials) / `WOLFMAIL_MS_CLIENT_ID`; without them `oauth_authorize` fails with kind `oauthConfig`.

### Providers (Gmail API / Microsoft Graph)

- An account signed in with Google (`auth: oauthGoogle`) syncs and sends through the **Gmail API**; one signed in
  with Microsoft (`oauthMicrosoft`) through **Microsoft Graph**. Password accounts — including Gmail/Outlook with an
  app password — stay on IMAP/SMTP. Reason: WolfMail uses the same OAuth apps as IntelligentWolf, which are approved
  for `gmail.modify` (the only Gmail scope WolfMail requests) and Graph `Mail.ReadWrite`/`Mail.Send`/`User.Read`, not for
  IMAP/SMTP (`https://mail.google.com/`, `IMAP.AccessAsUser.All`). Contract types are unchanged.
- `account_test` for an OAuth config makes one authenticated API call (Gmail `users.getProfile`, Graph
  `GET /me/mailFolders/inbox`); `imapOk`/`smtpOk` both report it.
- **Gmail folders are labels.** Inbox, Sent, Drafts, Trash, Spam (role `junk`) and Starred (role `flagged`) are the
  system labels; user labels are folders nested by `/`; "All Mail" (role `all`) is every message. A message is ONE
  row that appears in each of its label folders — `folderId` on a summary is always the account's All Mail folder.
  Trash/Spam membership hides a message from its other labels, All Mail and the unified views (as Gmail does).
  Archive removes the Inbox label; Move to a label adds it and removes Inbox; Copy adds the label; Junk adds Spam and
  removes Inbox. Tags are user labels with the tag's name (created when missing). Answered/forwarded stay local.
- **Gmail can't erase mail from Trash**: permanent deletion needs `https://mail.google.com/`, which these OAuth apps
  are not approved for. `messages_trash` on a message already in Trash returns `invalid` with the explanation (Gmail
  empties Trash itself after 30 days).
- **Graph:** folders are the mailbox's mail folders (search folders are skipped); roles come from the well-known
  names (inbox, sentitems, drafts, deleteditems, junkemail, archive, outbox). Archive moves to `archive`; Trash moves
  to `deleteditems`, and from there `permanentDelete`s. Tags are Outlook categories (unknown categories become tags).
  Ids are immutable (`Prefer: IdType="ImmutableId"`), so a moved message keeps its id.
- Moving/copying messages INTO a Microsoft 365 account from another account is refused (`invalid`): Graph can only
  create MIME messages as drafts. Into Gmail it works (`messages.insert`).
- Sync: Gmail = `history.list` since the stored history id (404 → full relisting) plus a paged full listing on first
  sync; Graph = per-folder delta rounds (410 → new round). Rows a completed full listing did not see are removed.
  Both poll every 30 s while WolfMail runs and immediately on `sync_now` — push needs a server (Gmail Pub/Sub, Graph
  webhooks) and WolfMail has none. Bodies for the last `bodyDays` are prefetched (50 per pass).
- Throttling: 429/503 (and Gmail's 403 rate-limit reasons) are retried up to 4 times, honouring `Retry-After`, else
  the account goes `offline` and retries with backoff.

## Calendar & contacts

Engine: `crates/wolfmail-core/src/pim/` with its **own store file `pim.sqlite3`** (own migration list in
`pim/store.rs`), so it never takes a number in the mail store's migrations. Tauri: `src-tauri/src/pim_cmds.rs`.
UI binding: `src/lib/pimApi.ts`, types `src/lib/pimTypes.ts` (mirror of `pim/model.rs`).

```ts
type PimKind = "local" | "dav" | "google" | "microsoft";
type PimAccount = { id; kind; name; email?; url?; username?; enabled; status: "ok"|"syncing"|"authError"|"offline"|"error"; statusMessage?; lastSyncAt? };
type Calendar = { id; accountId; name; color; visible; readOnly; isDefault; sortOrder };
type Attendee = { email; name?; partstat: "NEEDS-ACTION"|"ACCEPTED"|"DECLINED"|"TENTATIVE"|"DELEGATED"; optional };
type Event = { id; calendarId; uid; title; location; description; start; end /* exclusive; all-day = midnight UTC */;
  allDay; timezone /* IANA or "" */; rrule? /* RRULE value */; exdates: number[]; recurrenceId? /* override */;
  status: "CONFIRMED"|"TENTATIVE"|"CANCELLED"; organizer?: { email; name? }; attendees: Attendee[];
  reminders: number[] /* minutes before */; sequence; readOnly };
type Occurrence = { eventId; calendarId; title; location; start; end; allDay; color; recurring; occurrenceStart;
  status; myPartstat?; readOnly; hasAttendees };
type AddressBook = { id; accountId; name; readOnly };
type Contact = { id; bookId; uid; fullName; givenName; familyName; org; title; emails: {label,value}[]; phones; addresses;
  notes; birthday? /* "YYYY-MM-DD" | "--MM-DD" */; categories: string[]; photo? /* data: URL */; readOnly };
type Invite = { method; uid; title; start; end; allDay; location; description; organizer?; attendees; recurrenceId?;
  rrule?; sequence; myEmail?; myPartstat?; existingEventId?; conflicts: Occurrence[]; replyFrom? };
```

`MessageFull` gains `invite?: Invite` (set by `message_get` when the message has a text/calendar part).
`DraftInput` gains `calendar?: { method, ics }` — sent as a `text/calendar; method=…` part (RFC 6047).

| Command | Args | Returns |
|---|---|---|
| `pim_accounts_list` | – | `PimAccount[]` (always includes "On this computer", kind `local`) |
| `pim_oauth_authorize` | `{ provider: "google"\|"microsoft", email }` | `{ email }` — calendar/contacts grant, stored separately from mail |
| `pim_account_add` | `{ account: { kind, name, email?, url?, username?, password? } }` | `PimAccount` — DAV: discovery runs first; wrong password → `invalid` on `password`, bad address → `invalid` on `url`. Google/Microsoft: sign in first. |
| `pim_account_update` | `{ id, patch: { name?, enabled?, url?, username? }, password? }` | `PimAccount` |
| `pim_account_remove` | `{ id }` | `null` (local data only; nothing deleted on the server) |
| `pim_sync_now` | `{ accountId? }` | `null` (with id: syncs now and returns its error, if any) |
| `calendars_list` | – | `Calendar[]` (enabled accounts) |
| `calendar_update` | `{ id, patch: { name?, color?, visible? } }` | `Calendar` |
| `calendar_create` | `{ accountId, name, color? }` | `null` — local, DAV (MKCALENDAR), Google (`calendars.insert`), Graph |
| `calendar_delete` | `{ id }` | `null` — local calendars only |
| `events_range` | `{ from, to, calendarIds? }` | `Occurrence[]` — series expanded, overrides applied, ≤ ~400 days |
| `event_get` | `{ id }` | `Event` |
| `event_save` | `{ event, scope?: "this"\|"all", occurrenceStart? }` | `Event` — empty `id` creates. For a series, `occurrenceStart` is the clicked occurrence; "all" applies times as a shift. |
| `event_delete` | `{ id, scope: "this"\|"all", occurrenceStart? }` | `null` |
| `address_books_list` | – | `AddressBook[]` |
| `contacts_list` | `{ bookId?, query?, limit? }` | `Contact[]` (FTS prefix search over name, email, company, phone, notes, groups) |
| `contact_get` / `contact_save` / `contact_delete` | `{ id }` / `{ contact }` / `{ id }` | `Contact` / `Contact` / `null` — Google books are read-only (`invalid`) |
| `contacts_import_vcf` | `{ bookId, path }` | number imported |
| `contacts_export_vcf` | `{ ids, path }` | number exported (vCard 3.0) |
| `invite_respond` | `{ messageId, response: "ACCEPTED"\|"TENTATIVE"\|"DECLINED", calendarId? }` | `null` |
| `invite_apply` | `{ messageId }` | `null` — applies a CANCEL or a guest's REPLY to the matching event |
| `pim_import_scan` | `{ profilePath? }` | `{ profilePath, items: { kind, name, url?, username?, color?, enabled, localEvents }[] }[]` |
| `pim_import_apply` | `{ profilePath, items }` | `PimAccount[]` — Google/DAV accounts created in `authError` until signed in / password given |

| Event | Payload |
|---|---|
| `pim://changed` | `{}` — refetch calendars/books/visible ranges |
| `pim://status` | `{ accountId, status, message? }` |
| `pim://notice` | `{ message }` — a local edit lost to a concurrent server change (server version kept) |
| `calendar://reminder` | `{ eventId, title, start, allDay, location, minutes }` |

Behaviour:
- **Writes**: local → store; DAV/Google → store as dirty + immediate background push (works offline); Microsoft →
  Graph directly, then re-read (series edits need the server).
- **Conflicts**: DAV PUT/DELETE carry `If-Match`/`If-None-Match: *`; Google PATCH/DELETE carry `If-Match`. On 412 the
  server copy wins and `pim://notice` says which item.
- **Invitations sent**: events in local or DAV calendars that have guests get iMIP REQUEST / CANCEL mail from the
  organizer identity when time, title, place, rule, guests or status change. Google (`sendUpdates=all`) and Microsoft
  send their own. A DAV server doing RFC 6638 implicit scheduling would also mail guests (possible duplicate).
- **Answering invitations**: an invite already in a Google calendar is answered by patching our attendee status
  (Google notifies the organizer); in a Microsoft calendar by Graph `accept`/`tentativelyAccept`/`decline`;
  otherwise by iMIP REPLY mail from the invited identity.
- **Sync**: every 10 min, on `pim_sync_now`, and right after local edits; one sync per account at a time. DAV uses
  RFC 6578 sync-collection (falls back to etag listing), Google `syncToken`, Graph a calendarView window of
  −180…+540 days and a full contacts listing.
- **Google** uses Calendar API v3 + People API with IntelligentWolf's approved scopes (`auth/calendar`,
  `contacts.readonly`, `contacts.other.readonly`), so Google contacts are read-only. **Microsoft** needs
  `Calendars.ReadWrite` and `Contacts.ReadWrite` on the Entra app.
