// A release build that leaves out the Google OAuth client id runs, installs and then cannot touch a Gmail
// account ("Google sign-in is not set up in this build of WolfMail (missing WOLFMAIL_GOOGLE_CLIENT_ID)").
// That happened on 2 Nov 2026: 0.1.11 was built at 12:35 without loading ~/.config/wolfmail/google-oauth.env,
// installed over 0.1.10, and Paul could not read his email. oauth.rs reads the id with option_env!, so the
// compiler never complains. On a machine that HAS the ids file, a release build without them now stops here.
// CI has no such file and passes its ids from repo secrets (.github/workflows/release.yml), so it is unaffected;
// debug builds and tests are unaffected too.
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=WOLFMAIL_GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=PROFILE");
    if std::env::var("PROFILE").as_deref() != Ok("release") {
        return;
    }
    if std::env::var("WOLFMAIL_GOOGLE_CLIENT_ID").map(|v| !v.trim().is_empty()).unwrap_or(false) {
        return;
    }
    let Some(home) = std::env::var_os("HOME") else { return };
    let ids = PathBuf::from(home).join(".config/wolfmail/google-oauth.env");
    println!("cargo:rerun-if-changed={}", ids.display());
    if ids.exists() {
        panic!(
            "\n\nThis release build has no WOLFMAIL_GOOGLE_CLIENT_ID, so Gmail accounts would stop working in it.\n\
             The ids are in {path}. Load them first:\n\n    set -a; . {path}; set +a\n\nthen build again.\n",
            path = ids.display()
        );
    }
}
