import * as React from "react";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { Panel, PanelGroup, PanelResizeHandle, type ImperativePanelHandle } from "react-resizable-panels";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { AlertTriangle, RefreshCw } from "lucide-react";
import { boot, getState, isOverviewWindow, openComposer, setUi, togglePanel, useStore } from "./lib/store";
import { PanelRail } from "./components/PanelRail";
import { ResizeGrip } from "./components/ResizeGrip";
import { Overview } from "./features/overview/Overview";
import { usePim } from "./lib/pim";
import { PimLayer } from "./features/pim/ModeSwitch";
import { CalendarView } from "./features/calendar/CalendarView";
import { ContactsView } from "./features/contacts/ContactsView";
import { VaultGate } from "./features/onboarding/VaultGate";
import { dailyUpdateCheck } from "./lib/updates";
import { modalOpen } from "./lib/dom";
import { SnoozeHost } from "./features/snooze/SnoozeHost";
import { OAuthHost } from "./features/accounts/OAuthHost";
import { EmptyProgress } from "./features/list/EmptyFolder";
import { startFilterFromMessage } from "./features/settings/FiltersPage";
import type { MessageSummary } from "./lib/types";
import { events } from "./lib/api";
import { Toaster, notify } from "./lib/notify";
import { displayName, parseAddresses } from "./lib/utils";
import { Sidebar } from "./features/sidebar/Sidebar";
import { MessageList } from "./features/list/MessageList";
import { Reader } from "./features/reader/Reader";
import { ComposerDock } from "./features/compose/Composer";
import { newDraftFor } from "./features/compose/drafts";
import { CommandPalette } from "./features/palette/CommandPalette";
import { installShortcuts } from "./features/shortcuts";
import { IwPanel } from "./features/iw/IwPanel";
import { UpdateContactHost } from "./features/iw/UpdateContactDialog";
import { Assistant } from "./features/assistant/Assistant";
import { AskPanel } from "./features/ask/AskPanel";
import { IW_ENABLED } from "./lib/features";
import { Welcome } from "./features/onboarding/Welcome";
import { AddAccountDialog } from "./features/accounts/AddAccountDialog";
import { ImportDialog } from "./features/accounts/ImportDialog";
import { SettingsDialog } from "./features/settings/SettingsDialog";
import { IwConnectDialog } from "./features/iw/IwConnectDialog";
import { OutboxDialog } from "./features/outbox/OutboxDialog";
import { EmptyState, Skeleton } from "./components/ui/misc";
import { Button } from "./components/ui/button";

function storedTheme(): "light" | "dark" | null {
  try { const t = localStorage.getItem("wm.theme"); return t === "dark" || t === "light" ? t : null; } catch { return null; }
}

function useTheme(): "light" | "dark" {
  const pref = useStore((s) => s.settings?.theme ?? "system");
  const loaded = useStore((s) => !!s.settings);
  const [sys, setSys] = React.useState(() => window.matchMedia("(prefers-color-scheme: dark)").matches);
  React.useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const h = () => setSys(mq.matches);
    mq.addEventListener("change", h);
    return () => mq.removeEventListener("change", h);
  }, []);
  // Until settings arrive, keep the theme public/boot.js applied (the one last used), so the splash and first paint
  // don't switch theme halfway through starting.
  const theme = !loaded ? (storedTheme() ?? (sys ? "dark" : "light")) : pref === "system" ? (sys ? "dark" : "light") : pref;
  React.useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
    if (loaded) { try { localStorage.setItem("wm.theme", theme); } catch { /* per-viewer convenience only */ } }
  }, [theme, loaded]);
  return theme;
}

export function App() {
  const ready = useStore((s) => s.ready);
  const bootError = useStore((s) => s.bootError);
  const hasAccounts = useStore((s) => s.accounts.length > 0);
  const vaultLocked = useStore((s) => s.secrets?.backend === "file" && s.secrets.locked);
  const pimMode = usePim((s) => s.mode);
  const theme = useTheme();

  React.useEffect(() => { void boot(); }, []);
  // The splash in index.html covers the start; fade it out once the engine has answered (or failed to).
  React.useEffect(() => {
    if (!ready) return;
    const splash = document.getElementById("wm-splash");
    if (!splash) return;
    splash.classList.add("wm-splash-out");
    const t = setTimeout(() => splash.remove(), 250);
    return () => clearTimeout(t);
  }, [ready]);
  React.useEffect(() => installShortcuts(), []);
  React.useEffect(() => {
    const h = (e: Event) => { startFilterFromMessage((e as CustomEvent<MessageSummary>).detail); setUi({ settings: "filters" }); };
    window.addEventListener("wm:filter-from", h);
    return () => window.removeEventListener("wm:filter-from", h);
  }, []);
  React.useEffect(() => { if (!isOverviewWindow()) void dailyUpdateCheck(); }, []);
  useDesktopIntegration();

  return (
    <TooltipPrimitive.Provider delayDuration={350} skipDelayDuration={150}>
      {!ready ? (
        <BootSkeleton />
      ) : bootError ? (
        <div className="flex h-full items-center justify-center bg-surface">
          <EmptyState tone="error" icon={<AlertTriangle className="size-6" />} title="WolfMail couldn't start its mail engine" body={bootError}
            action={<Button onClick={() => window.location.reload()}><RefreshCw /> Try again</Button>} />
        </div>
      ) : vaultLocked ? (
        <VaultGate />
      ) : isOverviewWindow() ? (
        <div className="h-full"><Overview dark={theme === "dark"} /></div>
      ) : hasAccounts || pimMode !== "mail" ? (
        <MailShell theme={theme} />
      ) : (
        <Welcome />
      )}
      <ComposerDock />
      <CommandPalette />
      <AddAccountDialog />
      <ImportDialog />
      <SettingsDialog />
      <IwConnectDialog />
      <OutboxDialog />
      <SnoozeHost />
      <OAuthHost />
      <EmptyProgress />
      {ready && !bootError && !vaultLocked && !isOverviewWindow() && <PimLayer />}
      {IW_ENABLED && ready && !bootError && !vaultLocked && !isOverviewWindow() && hasAccounts && pimMode === "mail" && <Assistant />}
      {IW_ENABLED && <UpdateContactHost />}
      {IW_ENABLED && ready && !bootError && !vaultLocked && !isOverviewWindow() && <AskPanel />}
      <Toaster theme={theme} />
    </TooltipPrimitive.Provider>
  );
}

function MailShell({ theme }: { theme: "light" | "dark" }) {
  const readingPane = useStore((s) => s.settings?.readingPane ?? "right");
  const iwConnected = useStore((s) => !!s.iw?.connected);
  const iwShown = useStore((s) => s.ui.iwPanel);
  const iwOn = iwConnected && iwShown;
  const sidebarOn = useStore((s) => s.ui.sidebar);
  const openId = useStore((s) => s.openId);
  const isOverview = useStore((s) => s.view.kind === "overview");
  const pimMode = usePim((s) => s.mode);
  const [maxReader, setMaxReader] = React.useState(false);
  // Layout presets (features/layout/LayoutMenu) set the list's share of the space after the new layout has rendered.
  const listRef = React.useRef<ImperativePanelHandle>(null);
  React.useEffect(() => {
    const h = (e: Event) => {
      const size = (e as CustomEvent<number>).detail;
      setTimeout(() => listRef.current?.resize(size), 60);
    };
    window.addEventListener("wm:list-size", h);
    return () => window.removeEventListener("wm:list-size", h);
  }, []);

  React.useEffect(() => {
    const max = () => setMaxReader(true);
    const restore = () => setMaxReader(false);
    window.addEventListener("wm:maximize-reader", max);
    window.addEventListener("wm:restore-reader", restore);
    return () => { window.removeEventListener("wm:maximize-reader", max); window.removeEventListener("wm:restore-reader", restore); };
  }, []);
  // A new reading-pane position brings the list back (full-size reading belongs to the layout it was opened in).
  React.useEffect(() => { setMaxReader(false); }, [readingPane]);
  React.useEffect(() => {
    if (!maxReader) return;
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !modalOpen()) setMaxReader(false);
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [maxReader]);

  // Calendar and Contacts take the whole window (their left column carries the same switcher).
  if (pimMode === "calendar") return <div className="h-full"><CalendarView /></div>;
  if (pimMode === "contacts") return <div className="h-full"><ContactsView /></div>;

  // "Reading pane off": the list fills the window; Enter or double-click shows the reader full-size until Escape.
  const showReaderOnly = maxReader && !!openId;
  const reader = <Reader dark={theme === "dark"} maximized={maxReader} onToggleMax={() => setMaxReader((m) => !m)} />;
  const handle = "w-px bg-border transition-colors hover:bg-[var(--iw-red)] data-[resize-handle-state=drag]:bg-[var(--iw-red)]";

  return (
    <div className="flex h-full bg-surface-inset">
      {!sidebarOn && <PanelRail side="left" label="Mailboxes" shortcut="[" onShow={() => togglePanel("sidebar")} />}
      <div className="h-full min-w-0 flex-1">
      <PanelGroup direction="horizontal" autoSaveId="wm.layout.h">
        {sidebarOn && (
          <>
            <Panel id="sidebar" order={1} defaultSize={17} minSize={12} maxSize={30} className="min-w-[210px]">
              <Sidebar />
            </Panel>
            <PanelResizeHandle className={handle} />
          </>
        )}
        {isOverview ? (
          <Panel id="overview" order={2} minSize={40}><Overview dark={theme === "dark"} /></Panel>
        ) : showReaderOnly ? (
          <Panel id="reader-max" order={2} minSize={40}>{reader}</Panel>
        ) : readingPane === "bottom" ? (
          <Panel id="main" order={2} minSize={40}>
            <PanelGroup direction="vertical" autoSaveId="wm.layout.v">
              <Panel id="list" order={1} defaultSize={42} minSize={15} ref={listRef}><MessageList /></Panel>
              <ResizeGrip between="rows" />
              <Panel id="reader" order={2} minSize={20}>{reader}</Panel>
            </PanelGroup>
          </Panel>
        ) : readingPane === "off" ? (
          <Panel id="list-only" order={2} minSize={40}><MessageList /></Panel>
        ) : (
          <>
            <Panel id="list" order={2} defaultSize={30} minSize={15} className="min-w-[280px]" ref={listRef}><MessageList /></Panel>
            <ResizeGrip between="columns" />
            <Panel id="reader" order={3} minSize={25}>{reader}</Panel>
          </>
        )}
        {iwOn && !isOverview && (
          <>
            <PanelResizeHandle className={handle} />
            <Panel id="iw" order={4} defaultSize={20} minSize={15} maxSize={32} className="min-w-[260px]"><IwPanel /></Panel>
          </>
        )}
      </PanelGroup>
      </div>
      {iwConnected && !iwShown && !isOverview && <PanelRail side="right" label="IntelligentWolf" shortcut="I" onShow={() => togglePanel("iwPanel")} />}
    </div>
  );
}

function BootSkeleton() {
  return (
    <div aria-busy="true" aria-label="Starting WolfMail" className="flex h-full bg-surface-inset">
      <div className="w-[17%] min-w-[210px] space-y-3 bg-surface p-4">
        <Skeleton className="h-7 w-32" /><Skeleton className="h-10 w-full rounded-xl" />
        {Array.from({ length: 9 }).map((_, i) => <Skeleton key={i} className="h-6 w-full" />)}
      </div>
      <div className="w-[30%] space-y-4 border-l border-border bg-surface p-4">
        <Skeleton className="h-6 w-40" /><Skeleton className="h-8 w-full" />
        {Array.from({ length: 8 }).map((_, i) => <Skeleton key={i} className="h-14 w-full" />)}
      </div>
      <div className="flex-1 border-l border-border bg-surface" />
    </div>
  );
}

/** Desktop glue: OS file drops into the composer, mailto: links, new-mail notifications. */
function useDesktopIntegration() {
  React.useEffect(() => {
    const cleanups: (() => void)[] = [];
    let disposed = false;
    const keep = (p: Promise<() => void>) => p.then((u) => { if (disposed) u(); else cleanups.push(u); }).catch(() => { /* not running inside Tauri */ });

    keep(getCurrentWebview().onDragDropEvent((ev) => {
      if (ev.payload.type !== "drop") return;
      if (!getState().composers.some((c) => !c.minimized)) {
        if (ev.payload.paths.length) notify.info("Open a new message first, then drop files on it to attach them.");
        return;
      }
      window.dispatchEvent(new CustomEvent("wm:files-dropped", { detail: ev.payload.paths }));
    }));

    keep(events.mailNew(async (e) => {
      const s = getState();
      if (!s.settings?.notifications || e.messages.length === 0) return;
      try {
        if (await getCurrentWindow().isFocused()) return;
        let ok = await isPermissionGranted();
        if (!ok) ok = (await requestPermission()) === "granted";
        if (!ok) return;
        const acct = s.accounts.find((a) => a.id === e.accountId);
        if (e.messages.length === 1) {
          const m = e.messages[0];
          sendNotification({ title: displayName(m.from), body: `${m.subject || "(no subject)"}${acct ? ` · ${acct.name}` : ""}` });
        } else {
          sendNotification({ title: `${e.messages.length} new messages`, body: acct?.name ?? "WolfMail" });
        }
      } catch { /* notifications unavailable on this desktop; the unread counts still update */ }
    }));

    const onMailto = (e: Event) => {
      const href = (e as CustomEvent<string>).detail;
      try {
        const u = new URL(href);
        const to = parseAddresses(decodeURIComponent(u.pathname)).ok;
        const d = newDraftFor(to);
        d.subject = u.searchParams.get("subject") ?? "";
        const cc = u.searchParams.get("cc");
        if (cc) d.cc = parseAddresses(cc).ok;
        const body = u.searchParams.get("body");
        if (body) d.html = `<p>${body.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/\n/g, "<br>")}</p>`;
        openComposer({ mode: "new", draft: d, quoteOnTop: true });
      } catch (err) { notify.error(err, "Couldn't open that email link"); }
    };
    window.addEventListener("wm:mailto", onMailto);
    cleanups.push(() => window.removeEventListener("wm:mailto", onMailto));
    // Links clicked inside a message reach the app as new-window requests (src-tauri lib.rs external_links()).
    const here = getCurrentWebviewWindow();
    keep(here.listen<string>("link://mailto", (e) => window.dispatchEvent(new CustomEvent("wm:mailto", { detail: e.payload }))));
    keep(here.listen<string>("link://failed", (e) => notify.error({ kind: "internal", message: e.payload }, "Couldn't open the link")));

    return () => { disposed = true; cleanups.forEach((c) => c()); };
  }, []);
}

