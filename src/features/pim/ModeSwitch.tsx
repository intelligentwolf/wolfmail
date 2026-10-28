import * as React from "react";
import { Mail, CalendarDays, Users } from "lucide-react";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { bootPim, setMode, usePim, type Mode } from "../../lib/pim";
import { cn, modKey } from "../../lib/utils";
import { EventEditor } from "../calendar/EventEditor";
import { ScopePrompt } from "../calendar/actions";
import { PimAccountDialog, PimImportDialog } from "./PimDialogs";

const MODES: { m: Mode; label: string; icon: React.ComponentType<{ className?: string }>; key: string }[] = [
  { m: "mail", label: "Mail", icon: Mail, key: "1" },
  { m: "calendar", label: "Calendar", icon: CalendarDays, key: "2" },
  { m: "contacts", label: "Contacts", icon: Users, key: "3" },
];

/** Mail / Calendar / Contacts — a segmented control at the top of the sidebar (Ctrl/⌘+1/2/3). */
export function ModeSwitch() {
  const mode = usePim((s) => s.mode);
  return (
    <div role="tablist" aria-label="Switch between mail, calendar and contacts" className="@container mx-3 mb-3 grid grid-cols-3 rounded-control bg-surface-inset p-0.5 ring-1 ring-border">
      {MODES.map(({ m, label, icon: Icon, key }) => (
        <TooltipPrimitive.Root key={m} delayDuration={350}>
          <TooltipPrimitive.Trigger asChild>
            <button
              type="button"
              role="tab"
              aria-selected={mode === m}
              aria-keyshortcuts={`Control+${key}`}
              onClick={() => setMode(m)}
              className={cn("flex h-8 min-w-0 items-center justify-center gap-1.5 rounded-[6px] px-1 text-[12.5px] font-medium transition-colors",
                mode === m ? "bg-surface text-foreground shadow-theme-xs" : "text-muted-foreground hover:text-foreground")}
            >
              <Icon className={cn("size-4", mode === m && "text-[var(--iw-red-text)]")} />
              {/* Labels follow the sidebar's own width (it is resizable), not the window's. */}
              <span className="hidden truncate @[18.5rem]:inline">{label}</span>
              <span className="sr-only @[18.5rem]:hidden">{label}</span>
            </button>
          </TooltipPrimitive.Trigger>
          <TooltipPrimitive.Portal>
            <TooltipPrimitive.Content side="bottom" sideOffset={6} className="z-[100] rounded-md bg-pill-bg px-2 py-1 text-xs text-pill-fg shadow-theme-md">
              {label}<kbd className="ml-2 rounded bg-white/15 px-1 text-[11px] opacity-80">{modKey}+{key}</kbd>
            </TooltipPrimitive.Content>
          </TooltipPrimitive.Portal>
        </TooltipPrimitive.Root>
      ))}
    </div>
  );
}

/** The top of the left column in Calendar and Contacts: same brand row and switcher as the mail sidebar. */
export function PimHeader() {
  return (
    <>
      <div className="flex items-center gap-2 px-4 pb-3 pt-1">
        <img src="/icon.png" alt="" className="size-7 rounded-lg" />
        <span className="font-title text-[17px] tracking-tight">WolfMail</span>
      </div>
      <ModeSwitch />
    </>
  );
}

/** Calendar & contacts engine wiring + their dialogs; mounted once in the app shell. */
export function PimLayer() {
  React.useEffect(() => { bootPim(); }, []);
  return (
    <>
      <EventEditor />
      <ScopePrompt />
      <PimAccountDialog />
      <PimImportDialog />
    </>
  );
}
