import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Account, AccountTest, Address, ApiError, Autodetected, DraftInput, Folder, Identity, ImportCandidate,
  IwBusinessMailbox, IwPerson, IwStatus, MailChangedEvent, MailNewEvent, MessageFull, MessagePage, MessageSummary,
  OutboxItem, Provider, SearchHit, ServerConfig, Settings, SyncStatusEvent, Tag, UndoableEvent, ViewQuery, FlagName,
} from "./types";

/** Every engine failure arrives as an ApiError (docs/API.md "Errors"). Anything else — a panic string, a Tauri
 *  "command not found" — is wrapped so the UI always has a sentence to show. */
export function toApiError(e: unknown): ApiError {
  if (e && typeof e === "object" && "kind" in e && "message" in e) return e as ApiError;
  if (typeof e === "string") return { kind: "internal", message: e };
  if (e instanceof Error) return { kind: "internal", message: e.message };
  return { kind: "internal", message: "Something went wrong in WolfMail's engine." };
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toApiError(e);
  }
}

export const api = {
  overviewWindowOpen: () => call<null>("overview_window_open"),
  accountsList: () => call<Account[]>("accounts_list"),
  accountAutodetect: (email: string) => call<Autodetected>("account_autodetect", { email }),
  accountTest: (imap: ServerConfig, smtp: ServerConfig, password?: string) => call<AccountTest>("account_test", { imap, smtp, password }),
  accountAdd: (a: { name: string; email: string; displayName: string; provider: Provider; imap: ServerConfig; smtp: ServerConfig; password?: string; color?: string }) =>
    call<Account>("account_add", a),
  accountUpdate: (id: string, patch: Partial<Account>, password?: string) => call<Account>("account_update", { id, patch, password }),
  accountRemove: (id: string) => call<null>("account_remove", { id }),
  accountsReorder: (ids: string[]) => call<null>("accounts_reorder", { ids }),
  identityUpsert: (identity: Partial<Identity> & { accountId: string }) => call<Identity>("identity_upsert", { identity }),
  identityRemove: (id: string) => call<null>("identity_remove", { id }),
  oauthAuthorize: (provider: "google" | "microsoft", email: string) => call<{ email: string }>("oauth_authorize", { provider, email }),

  foldersList: (accountId?: string) => call<Folder[]>("folders_list", { accountId }),
  folderCreate: (accountId: string, name: string, parentId?: string) => call<Folder>("folder_create", { accountId, name, parentId }),
  folderRename: (id: string, name: string) => call<Folder>("folder_rename", { id, name }),
  folderDelete: (id: string) => call<null>("folder_delete", { id }),
  folderMarkRead: (id: string) => call<null>("folder_mark_read", { id }),

  messagesList: (query: ViewQuery) => call<MessagePage>("messages_list", { query }),
  threadGet: (threadId: string, accountId: string) => call<MessageSummary[]>("thread_get", { threadId, accountId }),
  messageGet: (id: string) => call<MessageFull>("message_get", { id }),
  messageLoadRemote: (id: string) => call<MessageFull>("message_load_remote", { id }),
  messageRaw: (id: string) => call<string>("message_raw", { id }),
  setFlags: (ids: string[], add: FlagName[], remove: FlagName[]) => call<null>("messages_set_flags", { ids, add, remove }),
  setTags: (ids: string[], add: string[], remove: string[]) => call<null>("messages_set_tags", { ids, add, remove }),
  move: (ids: string[], folderId: string) => call<null>("messages_move", { ids, folderId }),
  copy: (ids: string[], folderId: string) => call<null>("messages_copy", { ids, folderId }),
  archive: (ids: string[]) => call<null>("messages_archive", { ids }),
  trash: (ids: string[]) => call<null>("messages_trash", { ids }),
  junk: (ids: string[], junk: boolean) => call<null>("messages_junk", { ids, junk }),
  undo: (token: string) => call<null>("messages_undo", { token }),
  attachmentSave: (messageId: string, attachmentId: string, path: string) => call<null>("attachment_save", { messageId, attachmentId, path }),
  attachmentOpen: (messageId: string, attachmentId: string) => call<null>("attachment_open", { messageId, attachmentId }),

  draftSave: (draft: DraftInput) => call<{ id: string; serverSaved: boolean; serverError?: string }>("draft_save", { draft }),
  draftGet: (id: string) => call<DraftInput>("draft_get", { id }),
  draftDelete: (id: string) => call<null>("draft_delete", { id }),
  send: (draft: DraftInput) => call<OutboxItem>("send", { draft }),
  outboxList: () => call<OutboxItem[]>("outbox_list"),
  outboxCancel: (id: string) => call<DraftInput>("outbox_cancel", { id }),

  search: (query: string, limit = 50, accountId?: string) => call<SearchHit[]>("search", { query, limit, accountId }),
  contactsSuggest: (prefix: string, limit = 8) => call<Address[]>("contacts_suggest", { prefix, limit }),
  syncNow: (accountId?: string) => call<null>("sync_now", { accountId }),
  unsubscribe: (messageId: string) => call<{ method: "oneClick" | "mailto" | "browser" }>("unsubscribe", { messageId }),
  tagsList: () => call<Tag[]>("tags_list"),
  tagUpsert: (tag: { key?: string; name: string; color: string }) => call<Tag>("tag_upsert", { tag }),

  settingsGet: () => call<Settings>("settings_get"),
  settingsSet: (patch: Partial<Settings>) => call<Settings>("settings_set", { patch }),

  importScan: (profilePath?: string) => call<ImportCandidate[]>("import_scan", { profilePath }),
  importApply: (profilePath: string, emails: string[]) => call<Account[]>("import_apply", { profilePath, emails }),

  iwStatus: () => call<IwStatus>("iw_status"),
  iwPairStart: (baseUrl?: string) => call<{ code: string; verifyUrl: string; expiresAt: number }>("iw_pair_start", { baseUrl }),
  iwPairPoll: () => call<{ state: "pending" | "done" | "expired" }>("iw_pair_poll"),
  iwDisconnect: () => call<null>("iw_disconnect"),
  iwPerson: (email: string) => call<IwPerson | null>("iw_person", { email }),
  iwAi: (action: "summarise" | "draftReply" | "triage", messageId: string, instructions?: string) =>
    call<{ text: string; creditsUsed: number }>("iw_ai", { action, messageId, instructions }),
  iwLogEmail: (messageId: string, leadId?: string) => call<null>("iw_log_email", { messageId, leadId }),
  iwCreateTask: (messageId: string, title: string, due?: number) => call<{ taskId: string }>("iw_create_task", { messageId, title, due }),
  iwBookingLinks: () => call<{ url: string; title: string }[]>("iw_booking_link"),
  iwBusinessMailboxes: () => call<IwBusinessMailbox[]>("iw_business_mailboxes"),
};

export const events = {
  syncStatus: (cb: (e: SyncStatusEvent) => void): Promise<UnlistenFn> => listen<SyncStatusEvent>("sync://status", (ev) => cb(ev.payload)),
  mailChanged: (cb: (e: MailChangedEvent) => void): Promise<UnlistenFn> => listen<MailChangedEvent>("mail://changed", (ev) => cb(ev.payload)),
  mailNew: (cb: (e: MailNewEvent) => void): Promise<UnlistenFn> => listen<MailNewEvent>("mail://new", (ev) => cb(ev.payload)),
  outboxChanged: (cb: (e: OutboxItem) => void): Promise<UnlistenFn> => listen<OutboxItem>("outbox://changed", (ev) => cb(ev.payload)),
  undoable: (cb: (e: UndoableEvent) => void): Promise<UnlistenFn> => listen<UndoableEvent>("undoable", (ev) => cb(ev.payload)),
  countsChanged: (cb: (e: { unreadInbox: number }) => void): Promise<UnlistenFn> => listen<{ unreadInbox: number }>("counts://changed", (ev) => cb(ev.payload)),
};
