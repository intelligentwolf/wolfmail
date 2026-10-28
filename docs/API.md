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
| `oauth_authorize` | `{ provider: "google"\|"microsoft", email }` | `{ email }` — opens system browser, PKCE + loopback redirect, stores refresh token in keychain under the email |
| `folders_list` | `{ accountId? }` | `Folder[]` |
| `folder_create` | `{ accountId, parentId?, name }` | `Folder` |
| `folder_rename` | `{ id, name }` | `Folder` |
| `folder_delete` | `{ id }` | `null` |
| `folder_mark_read` | `{ id }` | `null` |
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
| `messages_undo` | `{ token }` | `null` (every move/trash/archive returns via event `undoable` a token valid 10 s) |
| `attachment_save` | `{ messageId, attachmentId, path }` | `null` |
| `attachment_open` | `{ messageId, attachmentId }` | `null` (writes to app cache dir, opens with system handler) |
| `draft_save` | `DraftInput` | `{ id, serverSaved, serverError? }` — always stored locally first; `serverSaved=false` (with the reason) when the APPEND to the server's Drafts folder failed — the local copy is safe and the next save retries. The new server copy replaces the previous one. |
| `draft_get` | `{ id }` | `DraftInput` — `id` may be a local draft id OR the message id of a row in a Drafts folder; a message id is adopted as a local draft (attachments copied out to the cache) and the returned `id` is that local draft, so a later `draft_save`/`send` replaces the server copy (deleted after the new APPEND succeeds / after send). |
| `draft_delete` | `{ id }` | `null` — local draft id (also removes its server copy) or Drafts-folder message id |
| `send` | `DraftInput` | `OutboxItem` (queued; sent after `undoSendSeconds` or at `sendAt`) |
| `outbox_list` | – | `OutboxItem[]` |
| `outbox_cancel` | `{ id }` | `DraftInput` (returns it so the UI can reopen the composer). Waiting or failed items only; no event is emitted. |
| `search` | `{ query, accountId?, limit? }` (limit default 50) | `SearchHit[]` (FTS5 over cached headers+bodies; supports `from:` `to:` `subject:` `has:attachment` `is:unread`) |
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
| `iw_ai` | `{ action: "summarise"\|"draftReply"\|"triage", messageId, instructions? }` | `{ text, creditsUsed }` |
| `iw_log_email` | `{ messageId, leadId? }` | `null` |
| `iw_create_task` | `{ messageId, title, due? }` | `{ taskId }` |
| `iw_booking_link` | – | `{ url, title }[]` |
| `iw_business_mailboxes` | – | `{ email, imapHost, imapPort, smtpHost, smtpPort }[]` (no passwords) |

## Events (engine → UI)

| Event | Payload |
|---|---|
| `sync://status` | `{ accountId, status: AccountStatus, message?, progress?: { done, total } }` |
| `mail://changed` | `{ folderIds: string[] }` — UI re-queries the visible view if it intersects |
| `mail://new` | `{ accountId, folderId, messages: MessageSummary[] }` — drives notifications + tray badge |
| `outbox://changed` | `OutboxItem` |
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
