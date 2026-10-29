# Getting WolfMail's Google and Microsoft sign-in verified

Goal: any Gmail, Outlook.com or Microsoft 365 user can sign in, with no "unverified app" warning, no test-user list
and no tenant admin having to approve WolfMail first.

The work is done in Google's and Microsoft's consoles by IntelligentWolf Ltd. This file lists what to do there and
what to paste into each form. The app-side changes are already in the code: the narrowest Gmail scope set (see
"Scopes" below) and a Microsoft client ID that is switched at build time.

---

## What WolfMail requests (source: `crates/wolfmail-core/src/oauth.rs`)

| Sign-in | Google | Microsoft (Graph, delegated) |
|---|---|---|
| Mail (`cfg()`) | `gmail.modify`, `openid`, `email` | `openid profile email offline_access Mail.ReadWrite Mail.Send User.Read` |
| Calendar and contacts (`PIM_*_SCOPES`) | `calendar.events`, `calendar.calendarlist.readonly`, `calendar.calendars`, `contacts.readonly`, `contacts.other.readonly`, `openid`, `email` | `Calendars.ReadWrite Contacts.ReadWrite offline_access openid email` |

Gmail scope classes (Gmail API "Choose scopes" page, read 29 Oct 2026): `gmail.modify` is **restricted**.
`gmail.send` (sensitive) and `gmail.settings.basic` (restricted) used to be requested too. They were dropped
because `gmail.modify` already covers sending and drafts, and WolfMail never calls a settings endpoint. If either
is still listed on the consent screen and IntelligentWolf's web app doesn't need it, remove it there too.

Calendar: the broad `calendar` scope was replaced by `calendar.events` + `calendar.calendarlist.readonly` +
`calendar.calendars`. These are the narrowest scopes the Calendar API reference accepts for each call WolfMail makes
(calendarList.list, events list/instances/insert/patch/delete, calendars.insert). **Add these three to the consent
screen before releasing this build.** Until they're there, Google warns on the calendar sign-in. People already
signed in keep working, because Google refreshes don't send a scope.

---

## Microsoft: publisher verification

### Why the current app can't be verified
The Azure CLI shows the current app "IntellgentWolf" (`010a0450-dbef-41f6-9710-dd499dc7566e`):
- It lives in the tenant created from the personal account `nbtafelberg@hotmail.co.uk`. Its publisher domain is
  `nbtafelberghotmailco.onmicrosoft.com`, and the tenant has no custom domain.
- It has no verified publisher.
- It lists only `User.Read` under API permissions. The mail scopes are requested at sign-in, but nothing is
  declared on the app.
- It has no privacy, terms or support URLs.

Microsoft's publisher verification requirements (learn.microsoft.com, "Publisher verification overview"):
"Apps that are registered by using a Microsoft account can't be publisher verified", and "the app's publisher
domain can't be \*.onmicrosoft.com". In the meantime, users in many work and school tenants **can't consent at
all**: unverified multi-tenant apps registered after 8 Nov 2020 that ask for more than sign-in and profile are
blocked when risk-based step-up consent is on. Mail.ReadWrite counts.

### Steps
1. **Custom domain.** In the Entra admin center, open Domain names → Add custom domain → `intelligentwolf.com`.
   Add the TXT record it shows at the DNS host, then click Verify.
2. **Work account.** In Users → New user, create e.g. `paul@intelligentwolf.com`. Give it the role
   **Application Administrator** (or Cloud Application Administrator) and register MFA for it. Do every step below
   signed in as this account, not the hotmail one.
3. **Partner account.** At partner.microsoft.com, join the Microsoft AI Cloud Partner Program as IntelligentWolf Ltd
   using the `@intelligentwolf.com` work account. Finish its company verification (it is free), and note the
   **Partner One ID** of the partner global account. Microsoft doesn't accept a location ID here. The Entra tenant
   from step 1 must be associated with that partner account.
4. **New app registration** (App registrations → New registration), signed in as the work account:
   - Name: `WolfMail`
   - Supported account types: **Accounts in any organizational directory and personal Microsoft accounts**
   - Authentication → Add a platform → **Mobile and desktop applications** → custom redirect URI
     `http://localhost`. Under Advanced settings, set **Allow public client flows** to **Yes**. Add no client
     secret. WolfMail is a public client.
   - API permissions → Microsoft Graph → Delegated: `Mail.ReadWrite`, `Mail.Send`, `User.Read`,
     `Calendars.ReadWrite`, `Contacts.ReadWrite`, `offline_access`, `openid`, `profile`, `email`.
     Don't grant admin consent: none of these need it.
   - Branding & properties: logo; home page `https://intelligentwolf.com/wolfmail`; terms of service URL;
     privacy statement URL (the hosted `docs/PRIVACY.md`); publisher domain **`intelligentwolf.com`**.
5. **Verify.** In Branding & properties → Publisher verification, enter the Partner One ID and click
   **Verify and save**. The blue badge appears on WolfMail's consent prompt.
6. **Ship it** (code side, after step 5): set the GitHub secret `WOLFMAIL_MS_CLIENT_ID` to the new Application
   (client) ID, change `MS_DEFAULT_CLIENT_ID` in `oauth.rs` to the same value, and cut a release. People who signed
   in through the old app have to sign in once more, because refresh tokens are tied to the client ID that issued
   them.

The old "IntellgentWolf" app keeps serving IntelligentWolf's website (`https://app.intelligentwolf.com/oauth_callback.php`).
Whether to move that one too is IntelligentWolf's call. The same verification problem applies to it.

---

## Google: production and restricted-scope verification

WolfMail's Desktop client lives in IntelligentWolf's Google Cloud project, so it shares that project's consent
screen, branding and verification status.

### First, check the project's current state
Open Google Auth Platform → **Audience** and **Verification Center**:
- **Publishing status "Testing"**: only listed test users can sign in, and their tokens expire after 7 days.
  Click **Publish app**, then submit for verification (below).
- **"In production", verified for the Gmail scopes**: open a case with Google from the Verification Center saying
  that a new **Desktop** client in the same project (WolfMail) uses the same scopes. Ask whether the existing
  verification and the security assessment cover it, or whether a resubmission is needed. The assessment probably
  has to be widened to include the desktop app and its data flows (see "Limited Use" below). Google decides that,
  not us.
- **In production, not verified**: users see "Google hasn't verified this app", and restricted scopes stop at 100
  users. Submit for verification.

A separate Google project just for WolfMail would mean a second verification and a second yearly security
assessment. Use the shared project unless Google says otherwise.

### Before submitting
1. **Homepage** on `intelligentwolf.com` (Search Console-verified domain) that describes WolfMail. It can't be just
   a sign-in page. It must link to the privacy policy. Suggested URL: `https://intelligentwolf.com/wolfmail`.
2. **Privacy policy** = `docs/PRIVACY.md`, hosted on the same domain. Use the same URL on the consent screen and on
   the homepage. It contains the Limited Use disclosure sentence.
3. **Branding**: the consent screen's app name, logo and support email must match what the demo video shows. The
   "Sign in with Google" button in WolfMail's add-account wizard must follow Google's branding guidelines.
4. **Authorized domains**: `intelligentwolf.com`, verified in Search Console by a project Owner or Editor.
5. **Scopes on the consent screen**: list only what is requested: `gmail.modify`, `calendar.events`,
   `calendar.calendarlist.readonly`, `calendar.calendars`, `contacts.readonly`,
   `contacts.other.readonly`, `openid`, `email`, plus whatever IntelligentWolf's web client needs.

### Scope justifications (paste into the form)

**gmail.modify (restricted)**: WolfMail is a desktop email client. It is a full replacement for the Gmail web
interface on the user's computer. It must download the user's messages and labels to show and search their mailbox,
and send mail and save drafts the user writes. It must also change message state: read/unread, starred, archive,
move between labels, move to Trash, and run filters the user set up in WolfMail. Narrower scopes don't work.
`gmail.readonly` and `gmail.metadata` can't change messages or send. `gmail.send` and `gmail.compose` can't read
the mailbox. `gmail.labels` can't read message content. The full `https://mail.google.com/` scope is NOT requested,
so WolfMail never permanently deletes mail; Trash is emptied by Gmail. All mail is stored only on the user's own
computer. WolfMail has no server that receives Gmail data.

**calendar.events, calendar.calendarlist.readonly, calendar.calendars**: WolfMail includes a calendar view.
`calendar.calendarlist.readonly` lists the user's calendars so WolfMail can show them. `calendar.events` lets users
see their events and create, edit and delete them in WolfMail. When they reply to meeting invitations, WolfMail
updates the event with `sendUpdates=all` so the organiser is told. `calendar.calendars` is used only when the user
creates a new calendar from WolfMail. These replace the broader `calendar` scope: WolfMail never shares calendars,
changes sharing permissions or deletes calendars.

**contacts.readonly, contacts.other.readonly (sensitive)**: WolfMail shows the user's Google contacts in its contacts
view and uses them, together with "Other contacts" (addresses the user has emailed), for autocomplete when writing
mail. It only reads them, never writes.

### Demo video (unlisted YouTube, 2–4 min)
Record WolfMail itself, the build from the release, not a dev build. Then:
1. Show WolfMail's homepage and privacy policy URL in a browser.
2. In WolfMail: Add account → enter a Gmail address → "Sign in with Google".
3. In the browser, show the full Google consent screen. Make sure the app name and **every** scope are readable,
   and show the address bar with the **client ID**. Approve.
4. Back in WolfMail, show each scope being used: the inbox loads (read), open a message, star it and mark it
   unread, move it to a label and to Trash (modify), write and send a reply (send), and show a saved draft.
5. Settings → Calendars & contacts → add the same Google account. Show the second consent screen with the calendar
   and contacts scopes, then show the calendar (create and delete an event) and the contacts view and autocomplete.
6. Show that the data stays local: say so in the narration, and show Settings → IntelligentWolf "Not connected".
   Then connect it and show that an AI action on one message only happens after the user clicks it.
7. Show removing the account in WolfMail and revoking access at myaccount.google.com/permissions.

### Security assessment (restricted scopes)
Apps with restricted scopes must pass a yearly assessment by a Google-empanelled assessor under the App Defense
Alliance's CASA framework. Google assigns the tier (AL1 or AL2). It's paid to the assessor and repeated every year.
Google's assessment page lists no exemption for apps that keep data on the device. Points to raise with the
assessor:
- WolfMail stores Google data only on the device: SQLite in the app data folder; tokens in the OS keychain or an
  encrypted vault (`secrets.rs`).
- The only server that can receive Gmail content is IntelligentWolf's `/wolfmail/*` routes, and only on an explicit
  user action on one message (`docs/IW-INTEGRATION.md`). Those routes and the AI provider are in scope for the
  assessment.
- Source is public (GPL-3.0-or-later), and releases are signed with SHA256SUMS, cosign and SLSA provenance
  (`.github/workflows/release.yml`).

---

## Limited Use: the part that needs a decision

Google's Limited Use rules allow Gmail data to be transferred only to provide user-facing features, and never for
ads. Human reading is allowed only with the user's consent for specific items, or for security or the law.
WolfMail sends Gmail data to IntelligentWolf in these cases (`crates/wolfmail-core/src/iw.rs`,
`engine/iwcmds.rs`):

| What | When | Fits Limited Use? |
|---|---|---|
| One address per visible message (`POST /wolfmail/people`) | automatically, once paired | User-facing feature (highlighting), but automatic. Disclose it, and show it in the video. |
| One message's subject, from, to, date and text, to Anthropic's Claude via IW (`/wolfmail/ai`) | user clicks an AI action | User-facing, on request. IW must not keep it or train on it. |
| One message's details saved to the CRM (`/wolfmail/log-email`, `/contact`, `/tasks`) | user clicks | User-facing, on request. Once in the CRM, other workspace members can read it. Google treats that as human access, so the click is the consent for that item. |
| The open email in a chat with the assistant (`/wolfmail/chat`) | user attaches it (removable chip) | User-facing, on request. |

The privacy policy commits IntelligentWolf to not training AI models on this data and not using it for ads. Make
sure those commitments are true of IntelligentWolf's servers before publishing.

---

## Where each statement in PRIVACY.md comes from
- Scopes: `crates/wolfmail-core/src/oauth.rs` (`cfg()`, `PIM_GOOGLE_SCOPES`, `PIM_MS_SCOPES`).
- Local storage: `engine/mod.rs` (`wolfmail.sqlite3` in the app data dir); `secrets.rs` (keychain or encrypted
  vault; never in SQLite or logs).
- Account removal deletes mail and the sign-in: `engine/accounts.rs` `account_remove` (FK `ON DELETE CASCADE`,
  `db.rs`); calendars: `pim/commands.rs` `pim_account_remove`.
- Google contacts are read-only: `pim/google.rs` header. Calendar writes use `sendUpdates=all`: same file.
- IntelligentWolf traffic: `docs/IW-INTEGRATION.md`, `iw.rs`, `engine/iwcmds.rs`; one address per visible row:
  `src/features/iw/people.ts` `otherParty`, `MessageList.tsx` `lookupPeople([party])`; AI via Anthropic's Claude:
  IntelligentWolf `php-api/lib/wolfmail_routes.php` (`claude_chat`).
- Autodetect lookups (ISPDB, domain autoconfig, DNS): `autodetect.rs` header.
- Update check: `src-tauri/tauri.conf.json` updater endpoint.
- Remote images blocked and tracking pixels stripped: the sanitiser in `mime.rs`.
