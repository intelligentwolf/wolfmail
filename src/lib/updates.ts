import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { notify } from "./notify";

// Signed updates (Tauri updater): the bundle's signature is checked against the public key in tauri.conf.json
// before anything installs, so a tampered download is refused (OWASP A08).

const LAST_CHECK = "wm.updates.lastCheck";
const DAY = 24 * 60 * 60 * 1000;

export type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "none" }
  | { kind: "available"; version: string; notes?: string; update: Update }
  | { kind: "downloading"; version: string; done: number; total: number }
  | { kind: "ready"; version: string }
  | { kind: "error"; message: string };

export async function checkForUpdate(): Promise<UpdateState> {
  try {
    const u = await check();
    try { localStorage.setItem(LAST_CHECK, String(Date.now())); } catch { /* convenience only */ }
    if (!u) return { kind: "none" };
    return { kind: "available", version: u.version, notes: u.body ?? undefined, update: u };
  } catch (e) {
    return { kind: "error", message: e instanceof Error ? e.message : String(e) };
  }
}

export async function installUpdate(u: Update, onProgress: (s: UpdateState) => void): Promise<void> {
  let total = 0;
  let done = 0;
  await u.downloadAndInstall((ev) => {
    if (ev.event === "Started") total = ev.data.contentLength ?? 0;
    if (ev.event === "Progress") done += ev.data.chunkLength;
    onProgress({ kind: "downloading", version: u.version, done, total });
  });
  onProgress({ kind: "ready", version: u.version });
}

export function restartApp() {
  relaunch().catch((e) => notify.error(e, "Couldn't restart WolfMail — close and reopen it to finish updating"));
}

/** Quiet daily check at startup: only says something when an update exists. Failures stay silent here (the user
 *  didn't ask), and are shown when they press "Check for updates" in Settings → About. */
export async function dailyUpdateCheck() {
  let last = 0;
  try { last = Number(localStorage.getItem(LAST_CHECK) || 0); } catch { /* ignore */ }
  if (Date.now() - last < DAY) return;
  const st = await checkForUpdate();
  if (st.kind !== "available") return;
  notify.info(`WolfMail ${st.version} is available`, {
    duration: 20000,
    action: {
      label: "Update",
      onClick: () => {
        notify.info(`Downloading WolfMail ${st.version}…`, { id: "wm-update" });
        installUpdate(st.update, () => {})
          .then(() => notify.info(`WolfMail ${st.version} is ready`, { id: "wm-update", duration: Infinity, action: { label: "Restart now", onClick: restartApp } }))
          .catch((e) => { notify.dismiss("wm-update"); notify.error(e, "The update didn't install"); });
      },
    },
  });
}
