// Runs before the app bundle, as a file because the CSP allows no inline scripts. It gives the splash screen and the
// first paint the theme WolfMail last used (App.tsx saves it as "wm.theme"), falling back to the system's, and drops
// the splash from the small all-inboxes overview window.
(function () {
  var stored = null;
  try { stored = localStorage.getItem("wm.theme"); } catch (e) { /* storage blocked: use the system theme */ }
  var dark = stored ? stored === "dark" : window.matchMedia("(prefers-color-scheme: dark)").matches;
  if (dark) document.documentElement.classList.add("dark");
  if (location.hash === "#overview") {
    var s = document.getElementById("wm-splash");
    if (s) s.remove();
  }
})();
