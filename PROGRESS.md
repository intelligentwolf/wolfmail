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

## Phases
- [x] 0 Repo, Tauri skeleton, design system, contract
- [x] 1 Engine core (storage, IMAP sync, folders, flags, move) — see limits below
- [x] 2 Reading (sanitised HTML, remote blocking, trackers, attachments)
- [x] 3 Compose + SMTP + outbox + drafts + identities
- [~] 4 OAuth (code complete, untested without client ids), autodetect, import
- [~] 5 Unified views, search, palette, tags, shortcuts, tray, notifications — written; partly exercised
- [~] 6 IntelligentWolf client side done; server routes NOT built (needs Paul's go-ahead)
- [ ] 7 Calendar (CalDAV/Google — Paul uses 6 calendars) + contacts (CardDAV)
- [ ] 8 Gmail API + Microsoft Graph adapters, message filters, snooze
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
