# WolfMail progress

Last updated: 2026-10-28. Say "go" in a new session to continue from the first unchecked item.

Product: a downloadable desktop email client for Windows, macOS and every Linux flavour. Useful with no
IntelligentWolf account; connecting IntelligentWolf unlocks the CRM sidebar, AI, booking links and tasks.
Stack: Tauri 2 + Rust engine (`crates/wolfmail-core`) + React 18 / TypeScript / Tailwind 4 using IntelligentWolf's
live design tokens. Engine ↔ UI contract: `docs/API.md`. IW server side: `docs/IW-INTEGRATION.md`.

Benchmark: Paul's Betterbird profile — 11 IMAP accounts (7 StackMail, 2 Gmail OAuth, 1 Microsoft 365 OAuth,
1 livemail), 206 folders, 25 GB.

## Needs Paul (cannot be done from code)
- [ ] Google Cloud OAuth client (Desktop) + restricted-scope verification / CASA → CI secrets
      `WOLFMAIL_GOOGLE_CLIENT_ID`, `WOLFMAIL_GOOGLE_CLIENT_SECRET`.
- [ ] Microsoft Entra multi-tenant public-client app + publisher verification → `WOLFMAIL_MS_CLIENT_ID`.
- [ ] Code signing: Windows (Azure Trusted Signing or EV), Apple Developer ID + notarisation.
- [ ] A GitHub repo for wolfmail (CI workflow is ready in `.github/workflows/build.yml`).
- [ ] Go-ahead to build + land the IW `/wolfmail/*` routes (php-api is live-on-write).

## Done and verified in the running app (2026-10-28, local GreenMail on 127.0.0.1, dark theme)
- Engine: SQLite+FTS5 store, IMAP sync (header-first, IDLE push measured at ~0.3 s, QRESYNC/CONDSTORE — tested on
  Dovecot by the engine suite), threading, sanitiser (tracking pixel stripped + remote images blocked, seen live),
  SMTP send with undo window (seen: "Sending in 10s… Undo"), Sent copy, draft autosave to server Drafts and
  removal after send (verified in DB), keychain secrets, Betterbird import scan (found Paul's 11 accounts).
- UI: unified inbox, account trees with colours + counts, multi-line list, conversation reader, keyboard nav,
  composer with autocomplete, plain-text mail themed, "All inboxes at once" view + separate window (live).
- Checks: `npx tsc --noEmit`, `npx vite build`, `cargo clippy --workspace --all-targets -D warnings`,
  `cargo test --workspace` (38 unit tests; integration tests need `WOLFMAIL_IT_*` env + local containers).

## Written but NOT yet exercised in the running app
Add-account wizard, import apply, settings dialog, IW connect, outbox dialog, folder create/rename/delete,
drag-drop move, context menus, tags, quick-filter chips, command palette search, attachments open/save,
send-later, attachment reminder, light theme, table layout, bottom/off reading pane, notifications, tray.
OAuth (no client ids), TLS ports 993/465/STARTTLS against real servers, real providers, IW routes (mock only).

## Phase 8 (mail part) — Gmail API + Microsoft Graph (2026-10-28, branch `feat/provider-apis`)
Google/Microsoft OAuth accounts now sync and send through the Gmail API / Graph with the scopes IntelligentWolf's
apps already hold (no `mail.google.com`, no IMAP/SMTP scopes). `crates/wolfmail-core/src/providers/` +
`engine/apiops.rs`; behaviour in `docs/API.md` → "Providers". Verified: 46 unit tests, `tests/providers_mock.rs`
(3 tests: full Gmail and Graph accounts through the real runner/commands against a local wiremock, plus
Retry-After), the GreenMail IMAP round trip still green, clippy -D warnings clean.
NOT exercised: real Google/Microsoft sign-in and APIs (no client ids; mocks only).
Known limits: Gmail can't purge Trash (scope); moving mail INTO a Microsoft 365 account is refused (Graph files MIME
as drafts); Gmail "Move to label" removes Inbox only (Gmail's semantics; the source label is not known from ids);
30 s polling instead of push (needs a server).

### Console steps for Paul (same apps as IntelligentWolf)
Google — the Cloud project that holds IW's OAuth client (`GOOGLE_CLIENT_ID` in IW's .env):
1. Google Auth Platform → Clients → Create client → Application type **Desktop app**, name "WolfMail"
   (Source: developers.google.com/identity/protocols/oauth2/native-app). It uses the loopback redirect
   `http://127.0.0.1:<port>`; WolfMail picks the port.
2. Copy its client ID and client secret into GitHub secrets `WOLFMAIL_GOOGLE_CLIENT_ID` and
   `WOLFMAIL_GOOGLE_CLIENT_SECRET` (Google: desktop apps are public clients, the secret is not confidential).
3. Scopes requested: gmail.modify, gmail.send, gmail.settings.basic, openid, email — all already on IW's consent
   screen (php-api/lib/gmail.php:28-50). Ask Google to confirm the existing restricted-scope verification covers the
   new Desktop client in the same project.
Microsoft — IW's Entra app (`MS_CLIENT_ID`), Microsoft Entra admin center → App registrations → that app:
1. Authentication → Add a platform → **Mobile and desktop applications** → custom redirect URI `http://localhost`
   (Source: learn.microsoft.com v2-oauth2-auth-code-flow — desktop apps using the system browser use
   `http://localhost`; reply-url doc — the port is ignored for localhost).
2. Authentication → Advanced settings → **Allow public client flows: Yes** (reply-url doc: an app with these redirect
   URIs "need[s] to enable a public client flow configuration"). WolfMail sends no client secret.
3. API permissions (Microsoft Graph, delegated): Mail.ReadWrite, Mail.Send, User.Read, offline_access, openid,
   profile, email — IW's MS_SCOPES already has them (php-api/lib/microsoft.php:20-29).
4. GitHub secret `WOLFMAIL_MS_CLIENT_ID` = the same Application (client) ID.

## Phases
- [x] 0 Repo, Tauri skeleton, design system, contract
- [x] 1 Engine core (storage, IMAP sync, folders, flags, move) — see limits below
- [x] 2 Reading (sanitised HTML, remote blocking, trackers, attachments)
- [x] 3 Compose + SMTP + outbox + drafts + identities
- [~] 4 OAuth (code complete, untested without client ids), autodetect, import
- [~] 5 Unified views, search, palette, tags, shortcuts, tray, notifications — written; partly exercised
- [~] 6 IntelligentWolf client side done; server routes NOT built (needs Paul's go-ahead)
- [ ] 7 Calendar (CalDAV/Google — Paul uses 6 calendars) + contacts (CardDAV)
- [~] 8 Gmail API + Microsoft Graph adapters DONE (branch feat/provider-apis, see below); message filters, snooze open
- [ ] 9 Packaging: CI matrix written (deb/rpm/AppImage, MSI/NSIS, DMG arm64+x64) but never run; Flatpak manifest;
      signed auto-update
- [ ] 10 Linux without a Secret Service daemon: encrypted local password store behind a primary password
      (today the app shows a clear error but cannot save passwords there)

## Known limitations (from the engine build)
- No offline action queue (actions while offline return a visible network error).
- Crash between SMTP accept and marking sent → resent on restart (duplicate preferred over loss).
- Gmail over IMAP until phase 8: All Mail bodies not prefetched; duplicates hidden only in threads/search.
- No-CONDSTORE servers: full flag rescan only up to 20 000 messages per folder.
- GreenMail dropped some connections ("End of stream"); engine went offline then recovered by itself. Re-check
  the connection pool against Dovecot/real servers.

## Session log
- 2026-10-28: plan; contract; engine + full UI; All-inboxes view + window; contrast tokens (measured WCAG);
  end-to-end run against local GreenMail; CI workflow.
