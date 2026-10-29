import { PanelLeftOpen, PanelRightOpen } from "lucide-react";
import { cn } from "../lib/utils";

/** What's left of a hidden side panel: a slim strip at that edge with the panel's name, so it is obvious where the
 *  panel went and one click (anywhere on the strip) brings it back (Paul, 29 Oct 2026: "make it obvious … not just a
 *  keypress"). */
export function PanelRail({ side, label, shortcut, onShow }: { side: "left" | "right"; label: string; shortcut: string; onShow: () => void }) {
  const Icon = side === "left" ? PanelLeftOpen : PanelRightOpen;
  return (
    <button
      type="button"
      onClick={onShow}
      aria-label={`Show ${label} (${shortcut})`}
      title={`Show ${label} (${shortcut})`}
      className={cn(
        "flex h-full w-9 shrink-0 flex-col items-center gap-3 border-border bg-surface pt-4 text-muted-foreground transition-colors",
        "hover:bg-accent/60 hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-[var(--iw-red)]",
        side === "left" ? "border-r" : "border-l",
      )}
    >
      <Icon className="size-4" aria-hidden />
      <span aria-hidden className="text-xs font-medium tracking-wide [writing-mode:vertical-rl]">{label}</span>
    </button>
  );
}
