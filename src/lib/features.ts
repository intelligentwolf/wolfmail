/** IntelligentWolf integration: on (Paul, 28 Oct 2026: "okay let's connect it to IntelligentWolf"), after an earlier
 *  standalone-testing spell with it off. Build with VITE_WOLFMAIL_IW=0 for a standalone build — Vite inlines the value,
 *  so the hidden UI is then dropped from the bundle rather than merely not rendered. */
export const IW_ENABLED = import.meta.env.VITE_WOLFMAIL_IW !== "0";

/** Where saved passwords go on this computer, in words for hints and confirmations. */
export function secretsPlace(backend: "keychain" | "file" | undefined): string {
  return backend === "file" ? "WolfMail's encrypted password vault" : "your system keychain";
}
