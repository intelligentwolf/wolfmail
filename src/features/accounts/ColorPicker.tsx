import { Check } from "lucide-react";
import { cn } from "../../lib/utils";

// Account colours. #c41e1e is IntelligentWolf's brand red (intelligentwolf/src/styles/theme.css:1153 --iw-red); the
// rest are distinct mid-tone hues chosen to stay visible as a small dot on both the light (#ffffff) and dark
// (#2e2f37) surfaces. Colour is never the only identifier — the account name is always shown beside it.
export const ACCOUNT_COLORS: { hex: string; name: string }[] = [
  { hex: "#c41e1e", name: "Wolf red" },
  { hex: "#d97706", name: "Amber" },
  { hex: "#15803d", name: "Green" },
  { hex: "#0d9488", name: "Teal" },
  { hex: "#2563eb", name: "Blue" },
  { hex: "#7c3aed", name: "Violet" },
  { hex: "#db2777", name: "Pink" },
  { hex: "#64748b", name: "Slate" },
];

/** Labelled radio group (W3C APG radio pattern via native inputs, so arrow keys work for free). */
export function ColorPicker({ value, onChange, name, label = "Colour" }: { value: string; onChange: (hex: string) => void; name: string; label?: string }) {
  return (
    <fieldset>
      <legend className="mb-1.5 block text-[13px] font-medium">{label}</legend>
      <div className="flex flex-wrap gap-2">
        {ACCOUNT_COLORS.map((c) => {
          const on = value.toLowerCase() === c.hex;
          return (
            <label key={c.hex} className="relative cursor-pointer" title={c.name}>
              <input type="radio" name={name} value={c.hex} checked={on} onChange={() => onChange(c.hex)} className="peer sr-only" />
              <span className="sr-only">{c.name}</span>
              <span
                aria-hidden
                className={cn(
                  "flex size-8 items-center justify-center rounded-full ring-2 ring-offset-2 ring-offset-[var(--popover)] transition-shadow peer-focus-visible:ring-[var(--iw-red)]",
                  on ? "ring-foreground/70" : "ring-transparent hover:ring-border",
                )}
                style={{ background: c.hex }}
              >
                {on && <Check className="size-4 text-white" />}
              </span>
            </label>
          );
        })}
      </div>
    </fieldset>
  );
}
