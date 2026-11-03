// A release build that leaves out an OAuth client id runs, installs and then cannot touch that provider's accounts
// ("Google sign-in is not set up in this build of WolfMail (missing WOLFMAIL_GOOGLE_CLIENT_ID)").
// That happened on 2 Nov 2026: 0.1.11 was built at 12:35 without loading ~/.config/wolfmail/google-oauth.env,
// installed over 0.1.10, and Paul could not read his email. oauth.rs reads the ids with option_env!, so the
// compiler never complains. On a machine that HAS the ids file, a release build without them now stops here.
// The Microsoft id joined this on 3 Nov 2026, when oauth.rs stopped carrying a built-in default for it.
// CI has no such file and passes its ids from repo secrets (.github/workflows/release.yml), so it is unaffected;
// debug builds, tests and other people's source builds are unaffected too.
use std::path::PathBuf;

// Source: crates/wolfmail-core/src/oauth.rs cfg() — the option_env! names for Google and Microsoft.
const REQUIRED: [&str; 2] = ["WOLFMAIL_GOOGLE_CLIENT_ID", "WOLFMAIL_MS_CLIENT_ID"];

fn main() {
    for name in REQUIRED {
        println!("cargo:rerun-if-env-changed={name}");
    }
    println!("cargo:rerun-if-env-changed=PROFILE");
    if std::env::var("PROFILE").as_deref() != Ok("release") {
        return;
    }
    let missing: Vec<&str> =
        REQUIRED.into_iter().filter(|n| !std::env::var(n).map(|v| !v.trim().is_empty()).unwrap_or(false)).collect();
    if missing.is_empty() {
        return;
    }
    let Some(home) = std::env::var_os("HOME") else { return };
    let ids = PathBuf::from(home).join(".config/wolfmail/google-oauth.env");
    println!("cargo:rerun-if-changed={}", ids.display());
    if ids.exists() {
        panic!(
            "\n\nThis release build has no {missing}, so those accounts would stop working in it.\n\
             The ids are in {path}. Load them first:\n\n    set -a; . {path}; set +a\n\nthen build again.\n",
            missing = missing.join(" or "),
            path = ids.display()
        );
    }
}
