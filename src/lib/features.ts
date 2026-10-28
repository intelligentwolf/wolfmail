/** IntelligentWolf integration is built but switched off until the server routes in docs/IW-INTEGRATION.md exist
 *  (Paul, 28 Oct 2026: "we won't integrate intelligentwolf yet so that i can test it standalone first").
 *  Build with VITE_WOLFMAIL_IW=1 to show every IW entry point again. Vite inlines the value, so the hidden UI is
 *  dropped from a standalone build rather than merely not rendered. */
export const IW_ENABLED = import.meta.env.VITE_WOLFMAIL_IW === "1";

/** Where saved passwords go on this computer, in words for hints and confirmations. */
export function secretsPlace(backend: "keychain" | "file" | undefined): string {
  return backend === "file" ? "WolfMail's encrypted password vault" : "your system keychain";
}
