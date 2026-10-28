import * as React from "react";
import * as DM from "@radix-ui/react-dropdown-menu";
import * as CM from "@radix-ui/react-context-menu";
import { Check, ChevronRight } from "lucide-react";
import { cn, hueFor, initials } from "../../lib/utils";
import type { Address } from "../../lib/types";

export function Avatar({ address, size = 36, className }: { address?: Address; size?: number; className?: string }) {
  const key = (address?.email || "?").toLowerCase();
  const h = hueFor(key);
  return (
    <div
      aria-hidden
      className={cn("flex shrink-0 select-none items-center justify-center rounded-full font-semibold text-white", className)}
      style={{ width: size, height: size, fontSize: Math.round(size * 0.36), background: `linear-gradient(135deg, hsl(${h} 55% 48%), hsl(${(h + 40) % 360} 55% 38%))` }}
    >
      {initials(address)}
    </div>
  );
}

export function Skeleton({ className, style }: { className?: string; style?: React.CSSProperties }) {
  return <div aria-hidden className={cn("wm-skeleton", className)} style={style} />;
}

/** Empty state recipe (ux-ui-foundation §6): what's missing, why, one action. */
export function EmptyState({ icon, title, body, action, tone = "default" }: {
  icon: React.ReactNode; title: string; body?: string; action?: React.ReactNode; tone?: "default" | "filtered" | "error";
}) {
  return (
    <div role={tone === "error" ? "alert" : undefined} className="flex h-full flex-col items-center justify-center px-8 py-12 text-center">
      <div className={cn("mb-4 flex size-14 items-center justify-center rounded-2xl", tone === "error" ? "bg-[var(--iw-red-wash)] text-[var(--iw-red-text)]" : "bg-surface-raised text-muted-foreground")}>
        {icon}
      </div>
      <h2 className="font-title text-base">{title}</h2>
      {body && <p className="mt-1.5 max-w-xs text-[13px] text-muted-foreground">{body}</p>}
      {action && <div className="mt-5">{action}</div>}
    </div>
  );
}

const menuPanel = "z-[90] min-w-[200px] overflow-hidden rounded-table bg-popover p-1 text-[13px] text-foreground shadow-[var(--shadow-pop)] ring-1 ring-border";
const menuItem = "relative flex h-8 cursor-default select-none items-center gap-2.5 rounded-md px-2.5 outline-none data-[highlighted]:bg-accent data-[disabled]:opacity-50 [&_svg]:size-4 [&_svg]:text-muted-foreground";

export const Menu = {
  Root: DM.Root,
  Trigger: DM.Trigger,
  Content: React.forwardRef<HTMLDivElement, DM.DropdownMenuContentProps>(function MenuContent({ className, sideOffset = 6, ...p }, ref) {
    return <DM.Portal><DM.Content ref={ref} sideOffset={sideOffset} className={cn(menuPanel, className)} {...p} /></DM.Portal>;
  }),
  Item: React.forwardRef<HTMLDivElement, DM.DropdownMenuItemProps & { shortcut?: string; danger?: boolean }>(function MenuItem({ className, shortcut, danger, children, ...p }, ref) {
    return (
      <DM.Item ref={ref} className={cn(menuItem, danger && "text-[var(--iw-red-text)] [&_svg]:text-[var(--iw-red-text)]", className)} {...p}>
        {children}
        {shortcut && <span className="ml-auto pl-4 text-[11px] text-muted-foreground">{shortcut}</span>}
      </DM.Item>
    );
  }),
  Check: function MenuCheck({ checked, onCheckedChange, children }: { checked: boolean; onCheckedChange: (v: boolean) => void; children: React.ReactNode }) {
    return (
      <DM.CheckboxItem checked={checked} onCheckedChange={onCheckedChange} className={cn(menuItem, "pl-8")}>
        <DM.ItemIndicator className="absolute left-2.5"><Check /></DM.ItemIndicator>
        {children}
      </DM.CheckboxItem>
    );
  },
  Label: ({ children }: { children: React.ReactNode }) => <DM.Label className="px-2.5 pb-1 pt-2 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">{children}</DM.Label>,
  Separator: () => <DM.Separator className="my-1 h-px bg-border" />,
  Sub: function MenuSub({ label, icon, children }: { label: string; icon?: React.ReactNode; children: React.ReactNode }) {
    return (
      <DM.Sub>
        <DM.SubTrigger className={cn(menuItem, "data-[state=open]:bg-accent")}>{icon}{label}<ChevronRight className="ml-auto" /></DM.SubTrigger>
        <DM.Portal><DM.SubContent className={cn(menuPanel, "max-h-[60vh] overflow-y-auto")} sideOffset={4}>{children}</DM.SubContent></DM.Portal>
      </DM.Sub>
    );
  },
};

export const Ctx = {
  Root: CM.Root,
  Trigger: CM.Trigger,
  Content: React.forwardRef<HTMLDivElement, CM.ContextMenuContentProps>(function CtxContent({ className, ...p }, ref) {
    return <CM.Portal><CM.Content ref={ref} className={cn(menuPanel, className)} {...p} /></CM.Portal>;
  }),
  Item: React.forwardRef<HTMLDivElement, CM.ContextMenuItemProps & { shortcut?: string; danger?: boolean }>(function CtxItem({ className, shortcut, danger, children, ...p }, ref) {
    return (
      <CM.Item ref={ref} className={cn(menuItem, danger && "text-[var(--iw-red-text)] [&_svg]:text-[var(--iw-red-text)]", className)} {...p}>
        {children}
        {shortcut && <span className="ml-auto pl-4 text-[11px] text-muted-foreground">{shortcut}</span>}
      </CM.Item>
    );
  }),
  Separator: () => <CM.Separator className="my-1 h-px bg-border" />,
  Sub: function CtxSub({ label, icon, children }: { label: string; icon?: React.ReactNode; children: React.ReactNode }) {
    return (
      <CM.Sub>
        <CM.SubTrigger className={cn(menuItem, "data-[state=open]:bg-accent")}>{icon}{label}<ChevronRight className="ml-auto" /></CM.SubTrigger>
        <CM.Portal><CM.SubContent className={cn(menuPanel, "max-h-[60vh] overflow-y-auto")} sideOffset={4}>{children}</CM.SubContent></CM.Portal>
      </CM.Sub>
    );
  },
};

export function Spinner({ className }: { className?: string }) {
  return <span aria-hidden className={cn("inline-block size-4 rounded-full border-2 border-current border-r-transparent wm-spin", className)} />;
}
