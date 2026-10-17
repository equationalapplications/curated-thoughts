fn main() {
    // pr283-review finding 4: `option_env!("CT_BUILD_SHA")` is COMPILE-time;
    // without this directive a cached binary built without (or with a stale)
    // CT_BUILD_SHA would silently keep the old provenance and `flip` would
    // refuse (or worse, accept a stale artifact) even though the runbook
    // exported the current sha. Rebuild whenever it changes.
    println!("cargo:rerun-if-env-changed=CT_BUILD_SHA");
}
