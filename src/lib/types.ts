// Mirror of docs/API.md (the engine ↔ UI contract). Keep the two in step: a field renamed here and not there
// is a silent undefined at runtime, because serde and TypeScript never see each other.

export type ApiErrorKind =
  | "auth" | "network" | "tls" | "server" | "notFound" | "invalid" | "storage" | "oauthConfig" | "cancelled" | "internal";
export type ApiError = { kind: ApiErrorKind; message: string; field?: string; accountId?: string };

export type Security = "tls" | "starttls" | "none";
export type AuthKind = "password" | "oauthGoogle" | "oauthMicrosoft";
export type ServerConfig = { host: string; port: number; security: Security; username: string; auth: AuthKind };
export type Provider = "imap" | "gmail" | "outlook";

export type Identity = {
  id: string; accountId: string; name: string; email: string; replyTo?: string;
  signatureHtml: string; replyOnTop: boolean; isDefault: boolean;
};

export type AccountStatus = "ok" | "syncing" | "authError" | "offline" | "error" | "disabled";
export type Account = {
  id: string; name: string; email: string; color: string; provider: Provider;
  imap: ServerConfig; smtp: ServerConfig; identities: Identity[];
  bodyDays: number; enabled: boolean; sortOrder: number;
  status: AccountStatus; statusMessage?: string; lastSyncAt?: number;
};

export type FolderRole = "inbox" | "sent" | "drafts" | "trash" | "junk" | "archive" | "all" | "flagged" | "outbox" | null;
export type Folder = {
  id: string; accountId: string; path: string; name: string; parentId?: string;
  role: FolderRole; unread: number; total: number; selectable: boolean; depth: number;
};

export type Address = { name?: string; email: string };
export type Flags = { seen: boolean; flagged: boolean; answered: boolean; forwarded: boolean; draft: boolean };
export type FlagName = keyof Flags;

export type MessageSummary = {
  id: string; accountId: string; folderId: string; threadId: string; messageId?: string;
  subject: string; from: Address; to: Address[]; cc: Address[]; date: number;
  snippet: string; flags: Flags; tags: string[]; hasAttachments: boolean; size: number;
  threadCount: number; threadUnread: number;
  snoozedUntil?: number; // unix ms; set while snoozed on this computer
};

export type Attachment = { id: string; filename: string; mime: string; size: number; inline: boolean; contentId?: string };

export type MessageFull = MessageSummary & {
  bcc: Address[]; replyTo: Address[]; inReplyTo?: string; references: string[];
  html?: string; text?: string; plainText: boolean;
  remoteContentBlocked: boolean; trackersRemoved: number;
  attachments: Attachment[];
  listUnsubscribe?: { mailto?: string; http?: string; oneClick: boolean };
  authResults?: { spf?: string; dkim?: string; dmarc?: string };
  /** Calendar invitation carried by the message (pimTypes.ts Invite). */
  invite?: import("./pimTypes").Invite;
};

export type UnifiedView = "inbox" | "sent" | "drafts" | "trash" | "junk" | "archive" | "flagged" | "unread" | "snoozed";
export type ViewFilter = { unread?: boolean; flagged?: boolean; attachments?: boolean; tag?: string; fromContact?: string };
export type ViewQuery = {
  folderId?: string; unified?: UnifiedView; filter?: ViewFilter; search?: string;
  threaded: boolean; sort: "dateDesc" | "dateAsc"; offset: number; limit: number;
};
export type MessagePage = { total: number; items: MessageSummary[] };

export type DraftAttachment = { path?: string; fromMessage?: { messageId: string; attachmentId: string } };
export type DraftInput = {
  id?: string; identityId: string;
  to: Address[]; cc: Address[]; bcc: Address[];
  subject: string; html: string; text: string;
  inReplyTo?: string; references: string[];
  replyToMessageId?: string; forwardOfMessageId?: string;
  attachments: DraftAttachment[];
  sendAt?: number;
};
export type OutboxItem = { id: string; draft: DraftInput; state: "waiting" | "sending" | "sent" | "failed"; sendAt: number; error?: string };

export type SearchHit = MessageSummary & { highlight: string };
export type Tag = { key: string; name: string; color: string };

export type Settings = {
  theme: "system" | "light" | "dark";
  density: "comfortable" | "compact";
  listLayout: "multiline" | "table";
  readingPane: "right" | "bottom" | "off";
  threaded: boolean;
  undoSendSeconds: number;
  closeToTray: boolean; startInTray: boolean;
  notifications: boolean;
  loadRemoteContent: "never" | "contacts" | "always";
  markReadAfterMs: number;
  iwBaseUrl: string;
};

export type Autodetected = { provider: Provider; imap: ServerConfig; smtp: ServerConfig; source: "builtin" | "ispdb" | "autoconfig" | "srv" | "guess" };
export type AccountTest = { imapOk: boolean; smtpOk: boolean; imapError?: ApiError; smtpError?: ApiError };

export type ImportAccount = {
  name: string; email: string; provider: Provider; imap: ServerConfig; smtp: ServerConfig;
  identities: Omit<Identity, "id" | "accountId">[]; folderCount: number; localSizeBytes: number;
};
export type ImportCandidate = { source: "betterbird" | "thunderbird"; profilePath: string; accounts: ImportAccount[]; tags: Tag[] };

export type IwStatus = { connected: boolean; user?: { name: string; email: string }; org?: { id: string; name: string }; baseUrl: string };
export type IwPerson = {
  kind: "lead" | "contact";
  name: string; email: string; company?: string; phone?: string;
  stage?: string; value?: number; currency?: string; score?: number;
  owner?: string; lastContactAt?: number; leadId?: string; contactId?: string; url: string;
  timeline: { at: number; kind: "email" | "call" | "meeting" | "note" | "task" | "invoice"; title: string }[];
};
export type IwBusinessMailbox = { email: string; imapHost: string; imapPort: number; smtpHost: string; smtpPort: number };

// Filters (engine/rules.rs). Field/op/action names are the engine's serde names.
export type FilterField = "from" | "to" | "cc" | "toOrCc" | "anyAddress" | "subject" | "hasAttachment";
export type FilterOp = "contains" | "notContains" | "is" | "isNot" | "beginsWith" | "endsWith" | "matchesRegex" | "isTrue" | "isFalse";
export type FilterCondition = { field: FilterField; op: FilterOp; value: string };
export type FilterAction =
  | { type: "moveTo"; folderId: string } | { type: "copyTo"; folderId: string }
  | { type: "markRead" } | { type: "star" } | { type: "tag"; tag: string }
  | { type: "archive" } | { type: "trash" } | { type: "junk" } | { type: "stop" };
export type FilterRule = {
  id: string; name: string; enabled: boolean; accountId?: string | null; matchAll: boolean;
  conditions: FilterCondition[]; actions: FilterAction[]; sortOrder: number;
};

export type SecretsStatus = { backend: "keychain" | "file"; initialized: boolean; locked: boolean; reason?: string };

// Events
export type SyncStatusEvent = { accountId: string; status: AccountStatus; message?: string; progress?: { done: number; total: number } };
export type MailChangedEvent = { folderIds: string[] };
export type MailNewEvent = { accountId: string; folderId: string; messages: MessageSummary[] };
export type UndoableEvent = { token: string; label: string; expiresAt: number };
