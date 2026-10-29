import { PanelResizeHandle } from "react-resizable-panels";
import { cn } from "../lib/utils";

/** The divider between the message list and the reader, with a visible grip so it reads as something to drag (Paul,
 *  29 Oct 2026: "allow me to drag the email/preview"). The library's handle is a focusable separator: arrow keys
 *  resize it too. `between` = how the two panels sit: side by side ("columns") or stacked ("rows"). */
export function ResizeGrip({ between }: { between: "columns" | "rows" }) {
  const cols = between === "columns";
  return (
    <PanelResizeHandle
      hitAreaMargins={{ coarse: 15, fine: 6 }}
      className={cn(
        "group relative flex items-center justify-center bg-border outline-none transition-colors",
        // The library also sets this cursor on the page while the pointer is inside its wider hit area; WebKitGTK
        // didn't show it over the handle itself (Paul, 29 Oct 2026), so the handle and its grip carry it too.
        cols ? "cursor-col-resize" : "cursor-row-resize",
        "data-[resize-handle-state=hover]:bg-[var(--iw-red)] data-[resize-handle-state=drag]:bg-[var(--iw-red)] focus-visible:bg-[var(--iw-red)]",
        cols ? "w-px" : "h-px",
      )}
    >
      <span
        aria-hidden
        className={cn(
          "z-10 rounded-full border border-border bg-surface-raised shadow-theme-sm transition-colors",
          cols ? "cursor-col-resize" : "cursor-row-resize",
          "group-data-[resize-handle-state=hover]:border-[var(--iw-red)] group-data-[resize-handle-state=drag]:border-[var(--iw-red)]",
          cols ? "h-9 w-1.5" : "h-1.5 w-9",
        )}
      />
    </PanelResizeHandle>
  );
}
