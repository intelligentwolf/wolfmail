import * as React from "react";
import { pimApi } from "../../lib/pimApi";
import { usePim } from "../../lib/pim";
import type { Occurrence } from "../../lib/pimTypes";

/** Occurrences in [from, to), refetched when calendar data changes (pim version) or the range moves. A failed
 *  fetch keeps the last good data and reports the error so the view can show it. */
export function useOccurrences(from: number, to: number) {
  const version = usePim((s) => s.version);
  const [state, set] = React.useState<{ items: Occurrence[]; loading: boolean; error?: string }>({ items: [], loading: true });
  React.useEffect(() => {
    let live = true;
    set((s) => ({ ...s, loading: true }));
    pimApi
      .eventsRange(from, to)
      .then((items) => live && set({ items, loading: false }))
      .catch((e) => live && set((s) => ({ ...s, loading: false, error: (e as { message?: string }).message ?? String(e) })));
    return () => { live = false; };
  }, [from, to, version]);
  return state;
}
