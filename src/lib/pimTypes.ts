// Mirror of crates/wolfmail-core/src/pim/model.rs (docs/API.md "Calendar & contacts"). camelCase on the wire.

export type PimKind = "local" | "dav" | "google" | "microsoft";
export type PimStatus = "ok" | "syncing" | "authError" | "offline" | "error";

export type PimAccount = {
  id: string; kind: PimKind; name: string; email?: string; url?: string; username?: string;
  enabled: boolean; status: PimStatus; statusMessage?: string; lastSyncAt?: number;
};

export type NewPimAccount = { kind: PimKind; name: string; email?: string; url?: string; username?: string; password?: string };

export type Calendar = {
  id: string; accountId: string; name: string; color: string; visible: boolean; readOnly: boolean; isDefault: boolean; sortOrder: number;
};

export type Partstat = "NEEDS-ACTION" | "ACCEPTED" | "DECLINED" | "TENTATIVE" | "DELEGATED";
export type Attendee = { email: string; name?: string; partstat: Partstat | string; optional: boolean };
export type Organizer = { email: string; name?: string };

export type CalEvent = {
  id: string; calendarId: string; uid: string; title: string; location: string; description: string;
  start: number; end: number; allDay: boolean; timezone: string;
  rrule?: string; exdates: number[]; recurrenceId?: number;
  status: "CONFIRMED" | "TENTATIVE" | "CANCELLED" | string;
  organizer?: Organizer; attendees: Attendee[]; reminders: number[]; sequence: number; readOnly: boolean;
};

export type Occurrence = {
  eventId: string; calendarId: string; title: string; location: string; start: number; end: number; allDay: boolean;
  color: string; recurring: boolean; occurrenceStart: number; status: string; myPartstat?: string; readOnly: boolean; hasAttendees: boolean;
};

export type EditScope = "this" | "all";

export type AddressBook = { id: string; accountId: string; name: string; readOnly: boolean };
export type Labeled = { label: string; value: string };
export type Contact = {
  id: string; bookId: string; uid: string; fullName: string; givenName: string; familyName: string; org: string; title: string;
  emails: Labeled[]; phones: Labeled[]; addresses: Labeled[]; notes: string; birthday?: string; categories: string[];
  photo?: string; readOnly: boolean;
};

export type Invite = {
  method: string; uid: string; title: string; start: number; end: number; allDay: boolean; location: string; description: string;
  organizer?: Organizer; attendees: Attendee[]; recurrenceId?: number; rrule?: string; sequence: number;
  myEmail?: string; myPartstat?: string; existingEventId?: string; conflicts: Occurrence[]; replyFrom?: Attendee;
};

export type PimImportItem = {
  kind: "caldav" | "carddav" | "local-calendar" | "google-calendar" | "google-contacts" | string;
  name: string; url?: string; username?: string; color?: string; enabled: boolean; localEvents: number;
};
export type PimImportCandidate = { profilePath: string; items: PimImportItem[] };

export type ReminderEvent = { eventId: string; title: string; start: number; allDay: boolean; location: string; minutes: number };
