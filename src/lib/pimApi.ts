import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { toApiError } from "./api";
import type {
  AddressBook, CalEvent, Calendar, Contact, EditScope, NewPimAccount, Occurrence, PimAccount, PimImportCandidate,
  PimImportItem, PimStatus, ReminderEvent,
} from "./pimTypes";

// Binding of src-tauri/src/pim_cmds.rs (Tauri maps these camelCase keys to the snake_case parameters).
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toApiError(e);
  }
}

export const pimApi = {
  accountsList: () => call<PimAccount[]>("pim_accounts_list"),
  oauthAuthorize: (provider: "google" | "microsoft", email: string) => call<{ email: string }>("pim_oauth_authorize", { provider, email }),
  accountAdd: (account: NewPimAccount) => call<PimAccount>("pim_account_add", { account }),
  accountUpdate: (id: string, patch: { name?: string; enabled?: boolean; url?: string; username?: string }, password?: string) =>
    call<PimAccount>("pim_account_update", { id, patch, password }),
  accountRemove: (id: string) => call<null>("pim_account_remove", { id }),
  syncNow: (accountId?: string) => call<null>("pim_sync_now", { accountId }),

  calendarsList: () => call<Calendar[]>("calendars_list"),
  calendarUpdate: (id: string, patch: { name?: string; color?: string; visible?: boolean }) => call<Calendar>("calendar_update", { id, patch }),
  calendarCreate: (accountId: string, name: string, color?: string) => call<null>("calendar_create", { accountId, name, color }),
  calendarDelete: (id: string) => call<null>("calendar_delete", { id }),

  eventsRange: (from: number, to: number, calendarIds?: string[]) => call<Occurrence[]>("events_range", { from, to, calendarIds }),
  eventGet: (id: string) => call<CalEvent>("event_get", { id }),
  eventSave: (event: CalEvent, scope?: EditScope, occurrenceStart?: number) => call<CalEvent>("event_save", { event, scope, occurrenceStart }),
  eventDelete: (id: string, scope: EditScope, occurrenceStart?: number) => call<null>("event_delete", { id, scope, occurrenceStart }),

  addressBooksList: () => call<AddressBook[]>("address_books_list"),
  contactsList: (bookId?: string, query?: string, limit?: number) => call<Contact[]>("contacts_list", { bookId, query, limit }),
  contactGet: (id: string) => call<Contact>("contact_get", { id }),
  contactSave: (contact: Contact) => call<Contact>("contact_save", { contact }),
  contactDelete: (id: string) => call<null>("contact_delete", { id }),
  contactsImportVcf: (bookId: string, path: string) => call<number>("contacts_import_vcf", { bookId, path }),
  contactsExportVcf: (ids: string[], path: string) => call<number>("contacts_export_vcf", { ids, path }),

  inviteRespond: (messageId: string, response: "ACCEPTED" | "TENTATIVE" | "DECLINED", calendarId?: string) =>
    call<null>("invite_respond", { messageId, response, calendarId }),
  inviteApply: (messageId: string) => call<null>("invite_apply", { messageId }),

  importScan: (profilePath?: string) => call<PimImportCandidate[]>("pim_import_scan", { profilePath }),
  importApply: (profilePath: string, items: PimImportItem[]) => call<PimAccount[]>("pim_import_apply", { profilePath, items }),
};

export const pimEvents = {
  changed: (cb: () => void): Promise<UnlistenFn> => listen("pim://changed", () => cb()),
  status: (cb: (e: { accountId: string; status: PimStatus; message?: string }) => void): Promise<UnlistenFn> =>
    listen<{ accountId: string; status: PimStatus; message?: string }>("pim://status", (ev) => cb(ev.payload)),
  notice: (cb: (e: { message: string }) => void): Promise<UnlistenFn> => listen<{ message: string }>("pim://notice", (ev) => cb(ev.payload)),
  reminder: (cb: (e: ReminderEvent) => void): Promise<UnlistenFn> => listen<ReminderEvent>("calendar://reminder", (ev) => cb(ev.payload)),
};
