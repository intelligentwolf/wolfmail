import * as React from "react";
import * as D from "@radix-ui/react-dialog";
import * as AD from "@radix-ui/react-alert-dialog";
import { X } from "lucide-react";
import { cn } from "../../lib/utils";
import { Button } from "./button";

// Radix Dialog/AlertDialog give role, labelledby, focus trap, Escape and focus return (W3C APG dialog + alertdialog).
// Radix 1.1.6 does NOT set aria-modal (checked in its dist), so it is set explicitly on every Content below.
// Every dialog here MUST pass a title.

const overlay = "fixed inset-0 z-50 bg-black/40 backdrop-blur-[2px] data-[state=open]:animate-in data-[state=open]:fade-in-0";
const panel =
  "fixed left-1/2 top-1/2 z-50 -translate-x-1/2 -translate-y-1/2 bg-popover text-foreground rounded-card shadow-[var(--shadow-pop)] ring-1 ring-border focus:outline-none max-h-[calc(100vh-48px)] flex flex-col";

/** `keepOnOutside`: a click (or focus) outside the panel does NOT close it — only its own buttons, the X and Escape do. For a dialog
 *  whose closing stops something the user is in the middle of elsewhere (a browser sign-in, OAuthHost). */
export function Dialog({
  open, onOpenChange, title, description, children, footer, className, hideClose, keepOnOutside,
}: {
  open: boolean; onOpenChange: (o: boolean) => void; title: React.ReactNode; description?: React.ReactNode;
  children?: React.ReactNode; footer?: React.ReactNode; className?: string; hideClose?: boolean; keepOnOutside?: boolean;
}) {
  return (
    <D.Root open={open} onOpenChange={onOpenChange}>
      <D.Portal>
        <D.Overlay className={overlay} />
        <D.Content aria-modal="true" className={cn(panel, "w-[min(560px,calc(100vw-32px))]", className)}
          onInteractOutside={keepOnOutside ? (e) => e.preventDefault() : undefined}>
          <div className="flex items-start gap-3 px-6 pt-5 pb-3">
            <div className="min-w-0 flex-1">
              <D.Title className="font-title text-lg">{title}</D.Title>
              {description ? <D.Description className="mt-1 text-sm text-muted-foreground">{description}</D.Description> : <D.Description className="sr-only">{typeof title === "string" ? title : "Dialog"}</D.Description>}
            </div>
            {!hideClose && (
              <D.Close asChild>
                <Button variant="ghost" size="icon" aria-label="Close"><X /></Button>
              </D.Close>
            )}
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-5">{children}</div>
          {footer && <div className="flex items-center justify-end gap-2 border-t border-border px-6 py-3">{footer}</div>}
        </D.Content>
      </D.Portal>
    </D.Root>
  );
}

/** Yes/no confirmation for medium-risk actions (ux-ui-foundation §1). Focus starts on Cancel for destructive ones,
 *  and the destructive button sits apart from Cancel (NN/G proximity of consequential options). */
export function ConfirmDialog({
  open, onOpenChange, title, body, confirmLabel, destructive, onConfirm,
}: {
  open: boolean; onOpenChange: (o: boolean) => void; title: string; body: React.ReactNode;
  confirmLabel: string; destructive?: boolean; onConfirm: () => void | Promise<void>;
}) {
  const [busy, setBusy] = React.useState(false);
  return (
    <AD.Root open={open} onOpenChange={onOpenChange}>
      <AD.Portal>
        <AD.Overlay className={overlay} />
        <AD.Content aria-modal="true" className={cn(panel, "w-[min(440px,calc(100vw-32px))] p-6")}>
          <AD.Title className="font-title text-lg">{title}</AD.Title>
          <AD.Description asChild>
            <div className="mt-2 text-sm text-muted-foreground">{body}</div>
          </AD.Description>
          <div className="mt-6 flex items-center gap-2">
            <AD.Cancel asChild>
              <Button variant="outline">Cancel</Button>
            </AD.Cancel>
            <div className="flex-1" />
            <Button
              variant={destructive ? "destructive" : "default"}
              disabled={busy}
              onClick={async () => {
                setBusy(true);
                try { await onConfirm(); onOpenChange(false); } finally { setBusy(false); }
              }}
            >
              {confirmLabel}
            </Button>
          </div>
        </AD.Content>
      </AD.Portal>
    </AD.Root>
  );
}
