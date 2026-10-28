import * as React from "react";
import { toast, Toaster as SonnerToaster } from "sonner";
import type { ApiError } from "./types";
import { toApiError } from "./api";

// Feedback rules (CLAUDE.md "Accessible user feedback"): confirmations go to a role="status" polite region,
// failures to a role="alert" assertive region; error toasts never auto-dismiss so the text can be read and copied.
// Sonner draws the visible toast; the two hidden regions below are what screen readers hear, because Sonner's own
// single polite region would announce failures politely.

type Live = { status: string; alert: string };
let setLive: React.Dispatch<React.SetStateAction<Live>> | null = null;

function announce(kind: keyof Live, text: string) {
  // Clear then set, so the same sentence twice is still announced twice.
  setLive?.((l) => ({ ...l, [kind]: "" }));
  setTimeout(() => setLive?.((l) => ({ ...l, [kind]: text })), 30);
}

const errorTitle: Record<ApiError["kind"], string> = {
  auth: "Sign-in failed",
  network: "Can't reach the server",
  tls: "Secure connection failed",
  server: "The mail server refused",
  notFound: "Not found",
  invalid: "Check the details",
  storage: "Couldn't save on this computer",
  oauthConfig: "Sign-in isn't set up in this build",
  cancelled: "Cancelled",
  internal: "Something went wrong",
};

export const notify = {
  ok(text: string, opts?: { action?: { label: string; onClick: () => void }; duration?: number }) {
    announce("status", text);
    toast.success(text, { action: opts?.action, duration: opts?.duration ?? 4000 });
  },
  info(text: string, opts?: { action?: { label: string; onClick: () => void }; duration?: number; id?: string }) {
    announce("status", text);
    return toast(text, { action: opts?.action, duration: opts?.duration ?? 5000, id: opts?.id });
  },
  error(e: unknown, context?: string) {
    const err: ApiError = toApiError(e);
    if (err.kind === "cancelled") return;
    const title = context ?? errorTitle[err.kind];
    announce("alert", `${title}. ${err.message}`);
    toast.error(title, { description: err.message, duration: Infinity, closeButton: true });
  },
  dismiss: (id?: string | number) => toast.dismiss(id),
};

export function Toaster({ theme }: { theme: "light" | "dark" }) {
  const [live, set] = React.useState<Live>({ status: "", alert: "" });
  React.useEffect(() => {
    setLive = set;
    return () => { setLive = null; };
  }, []);
  return (
    <>
      <div className="sr-only" role="status" aria-live="polite">{live.status}</div>
      <div className="sr-only" role="alert" aria-live="assertive">{live.alert}</div>
      <SonnerToaster
        theme={theme}
        position="bottom-center"
        toastOptions={{
          classNames: {
            toast: "!bg-popover !text-foreground !ring-1 !ring-border !border-0 !shadow-[var(--shadow-pop)] !rounded-table !font-sans !text-[13px]",
            description: "!text-muted-foreground selectable",
            actionButton: "!bg-primary !text-white !font-semibold",
            error: "[&_[data-icon]]:!text-[var(--iw-red-text)]",
            success: "[&_[data-icon]]:!text-[var(--iw-go-text)]",
          },
        }}
      />
    </>
  );
}
