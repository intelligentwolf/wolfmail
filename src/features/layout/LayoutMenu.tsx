import { BookOpen, Columns3, LayoutTemplate, List, PanelBottom } from "lucide-react";
import { setPanel, updateSettings, useStore } from "../../lib/store";
import { IconButton } from "../../components/ui/button";
import { Menu } from "../../components/ui/misc";
import type { Settings } from "../../lib/types";

type Pane = Settings["readingPane"];
type Preset = { id: string; label: string; hint: string; icon: typeof Columns3; sidebar: boolean; iw: boolean; pane: Pane; listSize?: number };

// Ready-made layouts (Paul, 29 Oct 2026: "add different layout setups that i can get from an icon on the top").
// listSize = the message list's share of the space it shares with the reader, in percent.
const PRESETS: Preset[] = [
  { id: "classic", label: "Classic", hint: "Mailboxes, list, email on the right", icon: Columns3, sidebar: true, iw: true, pane: "right", listSize: 32 },
  { id: "focus", label: "Focus on reading", hint: "Side panels hidden, big email", icon: BookOpen, sidebar: false, iw: false, pane: "right", listSize: 26 },
  { id: "below", label: "Email below the list", hint: "List on top, email underneath", icon: PanelBottom, sidebar: true, iw: false, pane: "bottom", listSize: 40 },
  { id: "list", label: "List only", hint: "Enter or double-click opens the email full-size", icon: List, sidebar: true, iw: false, pane: "off" },
];

/** Applies a preset: panels and reading pane now, the list's size once the new layout has rendered (App.tsx
 *  MailShell listens for wm:list-size). */
function apply(p: Preset, iwConnected: boolean) {
  window.dispatchEvent(new Event("wm:restore-reader")); // a layout always shows the list again
  setPanel("sidebar", p.sidebar);
  if (iwConnected) setPanel("iwPanel", p.iw);
  void updateSettings({ readingPane: p.pane });
  if (p.listSize != null) window.dispatchEvent(new CustomEvent("wm:list-size", { detail: p.listSize }));
}

export function LayoutMenu() {
  const sidebar = useStore((s) => s.ui.sidebar);
  const iwPanel = useStore((s) => s.ui.iwPanel);
  const iwConnected = useStore((s) => !!s.iw?.connected);
  const pane = useStore((s) => s.settings?.readingPane ?? "right");
  const current = PRESETS.find((p) => p.sidebar === sidebar && p.pane === pane && (!iwConnected || p.iw === iwPanel))?.id;

  return (
    <Menu.Root>
      <Menu.Trigger asChild>
        <IconButton label="Layout" size="iconSm"><LayoutTemplate /></IconButton>
      </Menu.Trigger>
      <Menu.Content align="end" className="w-72">
        <Menu.Label>Layouts</Menu.Label>
        {PRESETS.map((p) => (
          // Two-line rows: the menu's one-line h-8 made the hints overlap the next row (Paul, 29 Oct 2026).
          <Menu.Check key={p.id} checked={current === p.id} onCheckedChange={() => apply(p, iwConnected)} className="h-auto items-start py-1.5">
            <p.icon className="mt-0.5 size-4 shrink-0" />
            <span className="flex min-w-0 flex-col">
              <span>{p.label}</span>
              <span className="text-[11px] leading-snug text-muted-foreground">{p.hint}</span>
            </span>
          </Menu.Check>
        ))}
        <Menu.Separator />
        <Menu.Label>Show</Menu.Label>
        <Menu.Check checked={sidebar} onCheckedChange={(v) => setPanel("sidebar", v)}>Mailboxes <Kbd k="[" /></Menu.Check>
        {iwConnected && <Menu.Check checked={iwPanel} onCheckedChange={(v) => setPanel("iwPanel", v)}>IntelligentWolf panel <Kbd k="I" /></Menu.Check>}
        <Menu.Separator />
        <Menu.Label>Reading pane</Menu.Label>
        {([["right", "On the right"], ["bottom", "Below the list"], ["off", "Off"]] as const).map(([v, label]) => (
          <Menu.Check key={v} checked={pane === v} onCheckedChange={() => void updateSettings({ readingPane: v })}>{label}</Menu.Check>
        ))}
        <p className="px-2.5 pb-1.5 pt-2 text-[11px] text-muted-foreground">Drag the line between the list and the email to resize them.</p>
      </Menu.Content>
    </Menu.Root>
  );
}

function Kbd({ k }: { k: string }) {
  return <span className="ml-auto pl-4 text-[11px] text-muted-foreground">{k}</span>;
}
