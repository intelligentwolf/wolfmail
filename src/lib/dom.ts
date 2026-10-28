/** True while a modal dialog (Radix Dialog/AlertDialog, which mark themselves data-state="open") is on screen.
 *  The composer is a non-modal window (aria-modal="false") and is deliberately not counted. Radix 1.1.6 does not set
 *  aria-modal itself (checked in node_modules/@radix-ui/react-dialog/dist/index.mjs), so that attribute alone is not
 *  a reliable signal — this is the one place the app asks the question. */
export function modalOpen(): boolean {
  return !!document.querySelector('[role="dialog"][data-state="open"]:not([aria-modal="false"]), [role="alertdialog"][data-state="open"]');
}
