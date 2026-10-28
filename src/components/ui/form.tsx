import * as React from "react";
import * as SwitchPrimitive from "@radix-ui/react-switch";
import { ChevronDown } from "lucide-react";
import { cn } from "../../lib/utils";

const fieldBase =
  "w-full rounded-control bg-input-background px-3 text-[13px] text-foreground ring-1 ring-inset ring-transparent placeholder:text-muted-foreground transition-shadow focus:outline-none focus:ring-[var(--iw-red)] focus:shadow-focus-ring aria-[invalid=true]:ring-[var(--iw-red)] disabled:opacity-60";

export const Input = React.forwardRef<HTMLInputElement, React.ComponentProps<"input">>(function Input({ className, ...p }, ref) {
  return <input ref={ref} className={cn(fieldBase, "h-9", className)} {...p} />;
});

export const Textarea = React.forwardRef<HTMLTextAreaElement, React.ComponentProps<"textarea">>(function Textarea({ className, ...p }, ref) {
  return <textarea ref={ref} className={cn(fieldBase, "py-2 leading-relaxed", className)} {...p} />;
});

export function NativeSelect({ className, children, ...p }: React.ComponentProps<"select">) {
  return (
    <div className={cn("relative", className)}>
      <select className={cn(fieldBase, "h-9 appearance-none pr-8")} {...p}>{children}</select>
      <ChevronDown className="pointer-events-none absolute right-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
    </div>
  );
}

/** Label + control + hint + inline error, wired with aria-describedby / aria-invalid (GOV.UK error message). */
export function Field({
  label, hint, error, children, id, className,
}: { label: string; hint?: React.ReactNode; error?: string; children: React.ReactElement; id: string; className?: string }) {
  const hintId = hint ? `${id}-hint` : undefined;
  const errId = error ? `${id}-error` : undefined;
  const describedBy = [hintId, errId].filter(Boolean).join(" ") || undefined;
  return (
    <div className={cn("space-y-1.5", className)}>
      <label htmlFor={id} className="block text-[13px] font-medium">{label}</label>
      {hint && <p id={hintId} className="text-xs text-muted-foreground">{hint}</p>}
      {error && <p id={errId} className="text-xs font-medium text-[var(--iw-red-text)]">{error}</p>}
      {React.cloneElement(children as React.ReactElement<Record<string, unknown>>, { id, "aria-invalid": error ? true : undefined, "aria-describedby": describedBy })}
    </div>
  );
}

export function Switch({ checked, onCheckedChange, label, description, id }: {
  checked: boolean; onCheckedChange: (v: boolean) => void; label: string; description?: string; id: string;
}) {
  return (
    <div className="flex items-start justify-between gap-6 py-2.5">
      <div className="min-w-0">
        <label htmlFor={id} className="block text-[13px] font-medium">{label}</label>
        {description && <p className="mt-0.5 text-xs text-muted-foreground">{description}</p>}
      </div>
      <SwitchPrimitive.Root
        id={id}
        checked={checked}
        onCheckedChange={onCheckedChange}
        className="relative mt-0.5 h-5 w-9 shrink-0 rounded-full bg-[var(--switch-background)] transition-colors data-[state=checked]:bg-primary"
      >
        <SwitchPrimitive.Thumb className="block size-4 translate-x-0.5 rounded-full bg-white shadow transition-transform data-[state=checked]:translate-x-[18px]" />
      </SwitchPrimitive.Root>
    </div>
  );
}

export function Kbd({ children, className }: { children: React.ReactNode; className?: string }) {
  return <kbd className={cn("inline-flex h-5 min-w-5 items-center justify-center rounded border border-border bg-surface-raised px-1 font-sans text-[11px] text-muted-foreground", className)}>{children}</kbd>;
}
