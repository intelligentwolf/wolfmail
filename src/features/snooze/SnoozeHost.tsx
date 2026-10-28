import * as React from "react";
import { SnoozeDialog } from "./SnoozeMenu";

/** One "Snooze until…" dialog for the whole app. Opened by the B shortcut and the "Pick a date and time…" items via
 *  a `wm:snooze-pick` event (detail = message ids, or none for the current selection). A CustomEvent created without
 *  a detail carries `null`, so "open" is tracked separately from the ids. */
export function SnoozeHost() {
  const [state, setState] = React.useState<{ open: boolean; ids?: string[] }>({ open: false });
  React.useEffect(() => {
    const h = (e: Event) => setState({ open: true, ids: (e as CustomEvent<string[] | null>).detail ?? undefined });
    window.addEventListener("wm:snooze-pick", h);
    return () => window.removeEventListener("wm:snooze-pick", h);
  }, []);
  return <SnoozeDialog open={state.open} ids={state.ids} onClose={() => setState({ open: false })} />;
}
