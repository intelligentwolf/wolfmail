import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { cn } from "../../lib/utils";

// Variants follow IntelligentWolf's shared Button (intelligentwolf/src/app/components/ui/button.tsx:7-34), sized
// down for a dense desktop mail client: IW's h-11 is a phone-first target; here the default control is 36px and
// icon buttons 32px, which is what desktop mail clients use and still clears WCAG 2.5.8's 24px minimum.
export const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-control text-[13px] font-medium transition-colors duration-[var(--dur-fast)] disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-4 shrink-0 [&_svg]:shrink-0 focus-visible:shadow-focus-ring",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground shadow-theme-xs hover:bg-[var(--iw-red-hover)]",
        go: "bg-go text-white shadow-theme-xs hover:bg-[var(--iw-go-hover)]",
        destructive: "bg-destructive text-white hover:bg-[var(--iw-red-hover)]",
        outline: "bg-background text-foreground shadow-theme-xs ring-1 ring-inset ring-border hover:bg-accent",
        secondary: "bg-secondary text-foreground hover:bg-[var(--iw-surface-sunk)]",
        ghost: "text-foreground hover:bg-accent",
        link: "text-primary underline-offset-4 hover:underline",
      },
      size: {
        default: "h-9 px-3.5",
        sm: "h-8 gap-1.5 px-2.5",
        lg: "h-11 px-5 text-sm",
        icon: "size-8",
        iconSm: "size-7 [&_svg:not([class*='size-'])]:size-3.5",
      },
    },
    defaultVariants: { variant: "default", size: "default" },
  },
);

export type ButtonProps = React.ComponentProps<"button"> & VariantProps<typeof buttonVariants> & { asChild?: boolean };

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { className, variant, size, asChild = false, type, ...props },
  ref,
) {
  const Comp = asChild ? Slot : "button";
  return <Comp ref={ref} type={asChild ? undefined : (type ?? "button")} className={cn(buttonVariants({ variant, size, className }))} {...props} />;
});

/** Icon-only button: always has an accessible name, and a tooltip that shows it (plus the shortcut). */
export const IconButton = React.forwardRef<HTMLButtonElement, ButtonProps & { label: string; shortcut?: string; side?: "top" | "bottom" | "left" | "right" }>(
  function IconButton({ label, shortcut, side = "bottom", variant = "ghost", size = "icon", ...props }, ref) {
    return (
      <TooltipPrimitive.Root delayDuration={350}>
        <TooltipPrimitive.Trigger asChild>
          <Button ref={ref} variant={variant} size={size} aria-label={shortcut ? `${label} (${shortcut})` : label} {...props} />
        </TooltipPrimitive.Trigger>
        <TooltipPrimitive.Portal>
          <TooltipPrimitive.Content
            side={side}
            sideOffset={6}
            className="z-[100] rounded-md bg-pill-bg px-2 py-1 text-xs text-pill-fg shadow-theme-md"
          >
            {label}
            {shortcut && <kbd className="ml-2 rounded bg-white/15 px-1 font-sans text-[11px] opacity-80">{shortcut}</kbd>}
          </TooltipPrimitive.Content>
        </TooltipPrimitive.Portal>
      </TooltipPrimitive.Root>
    );
  },
);
