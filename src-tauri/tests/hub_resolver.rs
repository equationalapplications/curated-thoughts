//! Integration tests for the shared hub resolver (spec 2026-11-14-areas
//! §9.3 core + §6.8.2, task-3 brief). Fixtures build a migrated in-memory
//! brain, seed entities/redirects, and exercise longest-prefix hub
//! resolution, redirect-chain + live resolution, merge-group expansion,
//! globset area matching, and the mtime-gated areas-file cache.

use rusqlite::Connection;
use tauri_app_lib::config::WisdomConfig;
use tauri_app_lib::config::hub_resolver::{
    AreasFile, DepositHub, ScopedMatch, ScopedResolution, load_areas_file, parse_areas_file,
    resolve_deposit_hub, resolve_hub_id, resolve_scoped,
};
use tauri_app_lib::db::connection::open_in_memory;
use tempfile::TempDir;

fn db() -> Connection {
    open_in_memory().unwrap()
}

fn add_entity(conn: &Connection, id: &str) {
    conn.execute(
        "INSERT INTO curated_entities (id, name, entity_type, summary, created_at, updated_at)
         VALUES (?1, ?1, 'concept', 'Summary', 100, 100)",
        [id],
    )
    .unwrap();
}

fn add_redirect(conn: &Connection, from: &str, to: &str) {
    conn.execute(
        "INSERT INTO entity_redirects (entity_id, merged_into, created_at) VALUES (?1, ?2, 100)",
        [from, to],
    )
    .unwrap();
}

/// Config with one repo hub (checkout root = `root`) and org hub `org-1`.
fn cfg_one(root: &std::path::Path, hub: &str) -> WisdomConfig {
    WisdomConfig {
        areas_paths: vec![(hub.to_string(), root.join(".ct-areas.yaml"))],
        org_entity_id: Some("org-1".to_string()),
    }
}

fn write_areas_file(root: &std::path::Path, yaml: &str) {
    std::fs::write(root.join(".ct-areas.yaml"), yaml).unwrap();
}

// ---- resolve_deposit_hub -------------------------------------------------

#[test]
fn no_config_is_unconfigured() {
    let conn = db();
    assert_eq!(
        resolve_deposit_hub(&conn, None, "/anywhere/repo"),
        DepositHub::Unconfigured
    );
}

#[test]
fn path_outside_every_root_is_no_prefix() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let conn = db();
    add_entity(&conn, "hub-a");
    let cfg = cfg_one(&root, "hub-a");
    assert_eq!(
        resolve_deposit_hub(
            &conn,
            Some(&cfg),
            &tmp.path().join("other").join("f.rs").to_string_lossy()
        ),
        DepositHub::NoPrefix
    );
}

#[test]
fn innermost_checkout_nesting_wins() {
    let tmp = TempDir::new().unwrap();
    let outer = tmp.path().join("outer");
    let inner = outer.join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    let conn = db();
    add_entity(&conn, "hub-outer");
    add_entity(&conn, "hub-inner");
    let cfg = WisdomConfig {
        areas_paths: vec![
            ("hub-outer".to_string(), outer.join(".ct-areas.yaml")),
            ("hub-inner".to_string(), inner.join(".ct-areas.yaml")),
        ],
        org_entity_id: Some("org-1".to_string()),
    };
    let path = inner
        .join("src")
        .join("f.rs")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        resolve_deposit_hub(&conn, Some(&cfg), &path),
        DepositHub::Hub("hub-inner".to_string())
    );
}

#[test]
fn symlinked_root_canonicalizes_to_the_same_hub() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, tmp.path().join("link")).unwrap();
    let conn = db();
    add_entity(&conn, "hub-a");
    let cfg = cfg_one(&real, "hub-a");
    #[cfg(unix)]
    {
        let path = tmp
            .path()
            .join("link")
            .join("f.rs")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            resolve_deposit_hub(&conn, Some(&cfg), &path),
            DepositHub::Hub("hub-a".to_string())
        );
    }
}

/// FIX 1 (MAJOR-1): two configured keys whose canonical roots are equal —
/// one behind a symlink — is a TIE at scoped/deposit time; it must never
/// silently pick the first. Unresolved, naming BOTH ids.
#[test]
#[cfg(unix)]
fn symlink_tie_between_two_configured_roots_is_unresolved_naming_both() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, tmp.path().join("link")).unwrap();
    let conn = db();
    add_entity(&conn, "hub-a");
    add_entity(&conn, "hub-b");
    let cfg = WisdomConfig {
        areas_paths: vec![
            ("hub-a".to_string(), real.join(".ct-areas.yaml")),
            (
                "hub-b".to_string(),
                tmp.path().join("link").join(".ct-areas.yaml"),
            ),
        ],
        org_entity_id: Some("org-1".to_string()),
    };
    let path = real.join("f.rs").to_string_lossy().into_owned();
    match resolve_deposit_hub(&conn, Some(&cfg), &path) {
        DepositHub::Unresolved(reason) => {
            assert!(
                reason.contains("hub-a") && reason.contains("hub-b"),
                "tie reason must name BOTH ids: {reason}"
            );
        }
        other => panic!("expected Unresolved tie, got {other:?}"),
    }
    match resolve_scoped(&conn, &cfg, &path) {
        ScopedResolution::Unresolved(reason) => {
            assert!(
                reason.contains("hub-a") && reason.contains("hub-b"),
                "tie reason must name BOTH ids: {reason}"
            );
        }
        other => panic!("expected Unresolved tie, got {other:?}"),
    }
}

#[test]
fn non_existent_path_under_root_still_resolves_ancestor_plus_remainder() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(root.join("planned").join("deep")).unwrap();
    let conn = db();
    add_entity(&conn, "hub-a");
    let cfg = cfg_one(&root, "hub-a");
    let path = root.join("planned").join("deep").join("new.md");
    assert_eq!(
        resolve_deposit_hub(&conn, Some(&cfg), &path.to_string_lossy()),
        DepositHub::Hub("hub-a".to_string())
    );
}

#[test]
fn redirect_chain_merged_away_configured_key_still_binds() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let conn = db();
    add_entity(&conn, "hub-a");
    add_entity(&conn, "hub-b");
    add_redirect(&conn, "hub-a", "hub-b");
    let cfg = cfg_one(&root, "hub-a");
    let path = root.join("f.rs").to_string_lossy().into_owned();
    assert_eq!(
        resolve_deposit_hub(&conn, Some(&cfg), &path),
        DepositHub::Hub("hub-b".to_string())
    );
}

#[test]
fn redirect_cycle_is_unresolved() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let conn = db();
    add_entity(&conn, "hub-a");
    add_entity(&conn, "hub-b");
    add_redirect(&conn, "hub-a", "hub-b");
    add_redirect(&conn, "hub-b", "hub-a");
    let cfg = cfg_one(&root, "hub-a");
    let path = root.join("f.rs").to_string_lossy().into_owned();
    match resolve_deposit_hub(&conn, Some(&cfg), &path) {
        DepositHub::Unresolved(reason) => assert!(reason.contains("redirect cycle"), "{reason}"),
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

#[test]
fn soft_deleted_configured_hub_is_unresolved() {
    // Rung-3 (tier-1.5) MAJOR-2: liveness is ONE rule — a SOFT-DELETED
    // configured hub (`deleted_at` set, no redirect row) is NOT live. The
    // resolver must refuse it at the door (`hub_reason: not live`) exactly
    // as the synthesis force-include already does.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let conn = db();
    add_entity(&conn, "hub-a");
    conn.execute(
        "UPDATE curated_entities SET deleted_at = 1 WHERE id = 'hub-a'",
        [],
    )
    .unwrap();
    let cfg = cfg_one(&root, "hub-a");
    let path = root.join("f.rs").to_string_lossy().into_owned();
    match resolve_deposit_hub(&conn, Some(&cfg), &path) {
        DepositHub::Unresolved(reason) => assert!(reason.contains("not live"), "{reason}"),
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

#[test]
fn dead_survivor_is_unresolved() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let conn = db();
    add_entity(&conn, "hub-a");
    add_redirect(&conn, "hub-a", "hub-gone");
    let cfg = cfg_one(&root, "hub-a");
    let path = root.join("f.rs").to_string_lossy().into_owned();
    match resolve_deposit_hub(&conn, Some(&cfg), &path) {
        DepositHub::Unresolved(reason) => assert!(reason.contains("not live"), "{reason}"),
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

#[test]
fn same_survivor_collision_is_unresolved_with_carried_reason() {
    let tmp = TempDir::new().unwrap();
    let r1 = tmp.path().join("repo1");
    let r2 = tmp.path().join("repo2");
    std::fs::create_dir_all(&r1).unwrap();
    std::fs::create_dir_all(&r2).unwrap();
    let conn = db();
    add_entity(&conn, "key-1");
    add_entity(&conn, "key-2");
    add_entity(&conn, "survivor");
    add_redirect(&conn, "key-1", "survivor");
    add_redirect(&conn, "key-2", "survivor");
    let cfg = WisdomConfig {
        areas_paths: vec![
            ("key-1".to_string(), r1.join(".ct-areas.yaml")),
            ("key-2".to_string(), r2.join(".ct-areas.yaml")),
        ],
        org_entity_id: Some("org-1".to_string()),
    };
    let path = r1.join("f.rs").to_string_lossy().into_owned();
    match resolve_deposit_hub(&conn, Some(&cfg), &path) {
        DepositHub::Unresolved(reason) => assert!(reason.contains("collision"), "{reason}"),
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

// ---- resolve_scoped ------------------------------------------------------

#[test]
fn scoped_no_prefix_when_no_root_prefixes() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let conn = db();
    add_entity(&conn, "hub-a");
    add_entity(&conn, "org-1");
    let cfg = cfg_one(&root, "hub-a");
    assert_eq!(
        resolve_scoped(&conn, &cfg, &tmp.path().join("elsewhere").to_string_lossy()),
        ScopedResolution::NoPrefix
    );
}

/// FIX 2 (MAJOR-2): a redirect cycle at the matched hub is NOT a silent
/// `NoPrefix` — it is a loud `Unresolved` carrying a descriptive reason.
#[test]
fn scoped_redirect_cycle_is_unresolved_not_no_prefix() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    write_areas_file(&root, "areas:\n  deploy:\n    - infra/**\n");
    let conn = db();
    add_entity(&conn, "hub-a");
    add_entity(&conn, "hub-b");
    add_redirect(&conn, "hub-a", "hub-b");
    add_redirect(&conn, "hub-b", "hub-a");
    let cfg = cfg_one(&root, "hub-a");
    match resolve_scoped(
        &conn,
        &cfg,
        &root.join("infra").join("x.yml").to_string_lossy(),
    ) {
        ScopedResolution::Unresolved(reason) => {
            assert!(reason.contains("redirect cycle"), "{reason}");
        }
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

/// FIX 3b (MAJOR-3): the matched hub's areas file cannot be LOADED at
/// scoped time → loud `Unresolved`, never a silent area-less match.
#[test]
fn scoped_unloadable_areas_file_is_unresolved() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    // No .ct-areas.yaml written: the matched hub's areas file is missing.
    let conn = db();
    add_entity(&conn, "hub-a");
    add_entity(&conn, "org-1");
    let cfg = cfg_one(&root, "hub-a");
    match resolve_scoped(
        &conn,
        &cfg,
        &root.join("infra").join("x.yml").to_string_lossy(),
    ) {
        ScopedResolution::Unresolved(reason) => {
            assert!(reason.contains("areas file"), "{reason}");
        }
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

/// FIX 3a (MAJOR-3a): a bad glob pattern is a PARSE error of the areas
/// file — malformed file, no partial state (loader rule).
#[test]
fn bad_glob_pattern_is_a_parse_error() {
    let err = parse_areas_file("areas:\n  deploy:\n    - \"[\"\n")
        .expect_err("bad glob pattern must be a parse error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("invalid glob pattern"),
        "error must name the bad pattern as a glob problem: {msg}"
    );
}

#[test]
fn scoped_glob_matches_only_relative_to_root() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(root.join("infra")).unwrap();
    write_areas_file(&root, "areas:\n  deploy:\n    - infra/**\n");
    let conn = db();
    add_entity(&conn, "hub-a");
    add_entity(&conn, "org-1");
    let cfg = cfg_one(&root, "hub-a");

    // Inside the repo, infra/** matches.
    let m = match resolve_scoped(
        &conn,
        &cfg,
        &root.join("infra").join("x.yml").to_string_lossy(),
    ) {
        ScopedResolution::Bound(m) => m,
        other => panic!("expected Bound, got {other:?}"),
    };
    assert_eq!(m.area.as_deref(), Some("deploy"));
    assert_eq!(m.repo_group, vec!["hub-a".to_string()]);
    assert_eq!(m.org_group, Some(vec!["org-1".to_string()]));

    // Same shape outside the checkout root: NoPrefix (UNSCOPED fence, §9.8).
    let r = resolve_scoped(
        &conn,
        &cfg,
        &tmp.path()
            .join("other")
            .join("infra")
            .join("x.yml")
            .to_string_lossy(),
    );
    assert_eq!(r, ScopedResolution::NoPrefix);

    // Inside the repo but outside the pattern: repo band only, no area.
    let m = match resolve_scoped(
        &conn,
        &cfg,
        &root.join("src").join("a.rs").to_string_lossy(),
    ) {
        ScopedResolution::Bound(m) => m,
        other => panic!("expected Bound, got {other:?}"),
    };
    assert_eq!(m.area, None);
}

#[test]
fn scoped_resolve_then_expand_includes_survivor_and_losers() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    write_areas_file(&root, "areas:\n  deploy:\n    - infra/**\n");
    let conn = db();
    for id in ["hub-a", "hub-b", "org-1"] {
        add_entity(&conn, id);
    }
    add_redirect(&conn, "hub-a", "hub-b");
    let cfg = cfg_one(&root, "hub-a");
    let m = match resolve_scoped(
        &conn,
        &cfg,
        &root.join("infra").join("x.yml").to_string_lossy(),
    ) {
        ScopedResolution::Bound(m) => m,
        other => panic!("expected Bound, got {other:?}"),
    };
    let mut group = m.repo_group.clone();
    group.sort();
    assert_eq!(group, vec!["hub-a".to_string(), "hub-b".to_string()]);
}

#[test]
fn scoped_org_unresolvable_omits_org_arm_with_diagnostic() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    write_areas_file(&root, "areas:\n  deploy:\n    - infra/**\n");
    let conn = db();
    add_entity(&conn, "hub-a");
    // org-1 configured but NOT present as a live entity.
    let cfg = cfg_one(&root, "hub-a");
    let m = match resolve_scoped(
        &conn,
        &cfg,
        &root.join("infra").join("x.yml").to_string_lossy(),
    ) {
        ScopedResolution::Bound(m) => m,
        other => panic!("expected Bound, got {other:?}"),
    };
    assert_eq!(m.org_group, None);
    assert!(
        m.diagnostics.iter().any(|d| d.contains("org-1")),
        "diagnostics must name the omitted org hub: {:?}",
        m.diagnostics
    );
}

#[test]
fn scoped_org_redirect_resolves_to_survivor() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    write_areas_file(&root, "areas:\n  deploy:\n    - infra/**\n");
    let conn = db();
    for id in ["hub-a", "org-old", "org-new"] {
        add_entity(&conn, id);
    }
    add_redirect(&conn, "org-old", "org-new");
    let cfg = cfg_one(&root, "hub-a");
    let cfg = WisdomConfig {
        org_entity_id: Some("org-old".to_string()),
        ..cfg
    };
    let m = match resolve_scoped(
        &conn,
        &cfg,
        &root.join("infra").join("x.yml").to_string_lossy(),
    ) {
        ScopedResolution::Bound(m) => m,
        other => panic!("expected Bound, got {other:?}"),
    };
    let mut org = m.org_group.unwrap();
    org.sort();
    assert_eq!(org, vec!["org-new".to_string(), "org-old".to_string()]);
}

// ---- resolve_hub_id (§6.8.4, Librarian read side) ------------------------

#[test]
fn hub_id_accepts_a_configured_key_itself() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let conn = db();
    add_entity(&conn, "hub-a");
    add_entity(&conn, "org-1");
    let cfg = cfg_one(&root, "hub-a");
    assert_eq!(
        resolve_hub_id(&conn, &cfg, "hub-a"),
        DepositHub::Hub("hub-a".to_string())
    );
}

#[test]
fn hub_id_accepts_only_configured_survivors() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let conn = db();
    for id in ["hub-a", "hub-b", "stranger"] {
        add_entity(&conn, id);
    }
    add_redirect(&conn, "hub-a", "hub-b");
    let cfg = cfg_one(&root, "hub-a");
    // The merged-away key's survivor IS a configured key's survivor.
    assert_eq!(
        resolve_hub_id(&conn, &cfg, "hub-a"),
        DepositHub::Hub("hub-b".to_string())
    );
    // A live non-hub entity never binds.
    match resolve_hub_id(&conn, &cfg, "stranger") {
        DepositHub::Unresolved(reason) => {
            assert!(reason.contains("not a configured"), "{reason}")
        }
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

#[test]
fn hub_id_cycle_and_dead_are_unresolved() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let conn = db();
    for id in ["hub-a", "hub-b", "c1", "c2"] {
        add_entity(&conn, id);
    }
    add_redirect(&conn, "hub-a", "hub-b");
    add_redirect(&conn, "hub-b", "hub-gone");
    add_redirect(&conn, "c1", "c2");
    add_redirect(&conn, "c2", "c1");
    let cfg = cfg_one(&root, "hub-a");
    match resolve_hub_id(&conn, &cfg, "hub-a") {
        DepositHub::Unresolved(reason) => assert!(reason.contains("not live"), "{reason}"),
        other => panic!("expected Unresolved, got {other:?}"),
    }
    match resolve_hub_id(&conn, &cfg, "c1") {
        DepositHub::Unresolved(reason) => assert!(reason.contains("redirect cycle"), "{reason}"),
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

// ---- areas-file loader + mtime cache (§3.1–§3.5) -------------------------

#[test]
fn loader_parses_valid_file_in_order() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".ct-areas.yaml");
    std::fs::write(
        &path,
        "areas:\n  deploy:\n    - infra/**\n    - amplify.yml\n  keys:\n    - src/kms/**\n",
    )
    .unwrap();
    let f = load_areas_file(&path).unwrap();
    assert_eq!(
        f,
        AreasFile {
            areas: vec![
                (
                    "deploy".to_string(),
                    vec!["infra/**".to_string(), "amplify.yml".to_string()]
                ),
                ("keys".to_string(), vec!["src/kms/**".to_string()]),
            ],
        }
    );
}

#[test]
fn loader_malformed_file_is_an_error_with_no_partial_state() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".ct-areas.yaml");
    std::fs::write(&path, "areas: [broken").unwrap();
    assert!(load_areas_file(&path).is_err());
    // A second read of the SAME malformed file must still be an error —
    // the failed parse must not poison or cache partial state.
    assert!(load_areas_file(&path).is_err());
}

#[test]
fn loader_rereads_when_mtime_changes() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".ct-areas.yaml");
    std::fs::write(&path, "areas:\n  deploy:\n    - infra/**\n").unwrap();
    let f1 = load_areas_file(&path).unwrap();
    assert_eq!(f1.areas.len(), 1);
    // Rewrite with different content and force a DISTINCT mtime explicitly
    // (File::set_modified) — no wall-clock sleep flakiness.
    std::fs::write(
        &path,
        "areas:\n  deploy:\n    - infra/**\n  keys:\n    - src/kms/**\n",
    )
    .unwrap();
    let new_mtime = std::fs::metadata(&path).unwrap().modified().unwrap()
        + std::time::Duration::from_secs(3600);
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.set_modified(new_mtime).unwrap();
    drop(f);
    let f2 = load_areas_file(&path).unwrap();
    assert_eq!(f2.areas.len(), 2, "mtime change must force a re-read");
}

// ---- shape pins ----------------------------------------------------------

#[test]
fn scoped_match_diagnostics_default_empty_when_healthy() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    write_areas_file(&root, "areas:\n  deploy:\n    - infra/**\n");
    let conn = db();
    add_entity(&conn, "hub-a");
    add_entity(&conn, "org-1");
    let cfg = cfg_one(&root, "hub-a");
    let m: ScopedMatch = match resolve_scoped(
        &conn,
        &cfg,
        &root.join("infra").join("y.yml").to_string_lossy(),
    ) {
        ScopedResolution::Bound(m) => m,
        other => panic!("expected Bound, got {other:?}"),
    };
    assert!(m.diagnostics.is_empty(), "{:?}", m.diagnostics);
}

/// Gauntlet fix 1 (CodeRabbit Major): an EMPTY area name (`"": [...]`) is a
/// PARSE error — it would otherwise flow exact-match and the commit path
/// maps `""` to NULL, silently degrading scoped guidance to whole-repo.
#[test]
fn empty_area_name_is_a_parse_error() {
    let err = parse_areas_file("areas:\n  \"\":\n    - \"**\"\n")
        .expect_err("empty area name must be a parse error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("empty"),
        "error must name the empty area name: {msg}"
    );
}

/// Gauntlet fix 1: a whitespace-only area name is equally refused.
#[test]
fn whitespace_only_area_name_is_a_parse_error() {
    let err = parse_areas_file("areas:\n  \"  \":\n    - \"**\"\n")
        .expect_err("whitespace-only area name must be a parse error");
    assert!(format!("{err:#}").contains("empty"));
}

// ---- resolve_brief_scope (spec 2026-11-28-wisdom-brief 3.2) ---------------

use tauri_app_lib::config::hub_resolver::resolve_brief_scope;

fn sorted(g: &Option<Vec<String>>) -> Vec<String> {
    let mut v = g.clone().expect("group present");
    v.sort();
    v
}

fn brief_fixture() -> (TempDir, std::path::PathBuf, Connection) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let conn = db();
    (tmp, root, conn)
}

#[test]
fn brief_scope_bound_repo_and_org() {
    let (_t, root, conn) = brief_fixture();
    add_entity(&conn, "hub-a");
    add_entity(&conn, "org-1");
    let cfg = cfg_one(&root, "hub-a");
    let s = resolve_brief_scope(&conn, Some(&cfg), &root.join("src").join("x.rs").to_string_lossy());
    assert_eq!(sorted(&s.repo_group), vec!["hub-a".to_string()]);
    assert_eq!(sorted(&s.org_group), vec!["org-1".to_string()]);
    assert_eq!(s.reason, None);
    assert!(s.diagnostics.is_empty());
}

#[test]
fn brief_scope_no_prefix_gives_org_only() {
    let (t, root, conn) = brief_fixture();
    add_entity(&conn, "hub-a");
    add_entity(&conn, "org-1");
    let cfg = cfg_one(&root, "hub-a");
    let s = resolve_brief_scope(&conn, Some(&cfg), &t.path().join("elsewhere").to_string_lossy());
    assert_eq!(s.repo_group, None);
    assert_eq!(sorted(&s.org_group), vec!["org-1".to_string()]);
    assert_eq!(s.reason, Some("no_prefix"));
}

#[test]
fn brief_scope_no_prefix_no_org_is_none() {
    let (t, root, conn) = brief_fixture();
    add_entity(&conn, "hub-a");
    let cfg = WisdomConfig {
        org_entity_id: None,
        ..cfg_one(&root, "hub-a")
    };
    let s = resolve_brief_scope(&conn, Some(&cfg), &t.path().join("elsewhere").to_string_lossy());
    assert_eq!((s.repo_group, s.org_group), (None, None));
    assert_eq!(s.reason, Some("no_prefix"));
}

#[test]
fn brief_scope_unconfigured() {
    let (_t, root, conn) = brief_fixture();
    add_entity(&conn, "org-1");
    let s = resolve_brief_scope(&conn, None, &root.to_string_lossy());
    assert_eq!((s.repo_group.clone(), s.org_group.clone()), (None, None));
    assert_eq!(s.reason, Some("unconfigured"));
    // Config with an org but no areas_paths: org still served.
    let cfg = WisdomConfig {
        areas_paths: vec![],
        org_entity_id: Some("org-1".to_string()),
    };
    let s = resolve_brief_scope(&conn, Some(&cfg), &root.to_string_lossy());
    assert_eq!(s.repo_group, None);
    assert_eq!(sorted(&s.org_group), vec!["org-1".to_string()]);
    assert_eq!(s.reason, Some("unconfigured"));
}

#[test]
fn brief_scope_unresolved_collision_gives_org_only() {
    let tmp = TempDir::new().unwrap();
    let r1 = tmp.path().join("repo1");
    let r2 = tmp.path().join("repo2");
    std::fs::create_dir_all(&r1).unwrap();
    std::fs::create_dir_all(&r2).unwrap();
    let conn = db();
    for id in ["key-1", "key-2", "survivor", "org-1"] {
        add_entity(&conn, id);
    }
    add_redirect(&conn, "key-1", "survivor");
    add_redirect(&conn, "key-2", "survivor");
    let cfg = WisdomConfig {
        areas_paths: vec![
            ("key-1".to_string(), r1.join(".ct-areas.yaml")),
            ("key-2".to_string(), r2.join(".ct-areas.yaml")),
        ],
        org_entity_id: Some("org-1".to_string()),
    };
    let s = resolve_brief_scope(&conn, Some(&cfg), &r1.to_string_lossy());
    assert_eq!(s.repo_group, None);
    assert_eq!(sorted(&s.org_group), vec!["org-1".to_string()]);
    assert_eq!(s.reason, Some("unresolved"));
    assert!(s.diagnostics.iter().any(|d| d.contains("collision")), "{:?}", s.diagnostics);
    // Diagnostics hold ids, never the checkout path.
    let root_str = tmp.path().to_string_lossy().into_owned();
    assert!(s.diagnostics.iter().all(|d| !d.contains(&root_str)), "{:?}", s.diagnostics);
}

#[test]
fn brief_scope_org_unresolvable_sets_reason() {
    let (_t, root, conn) = brief_fixture();
    add_entity(&conn, "hub-a");
    let cfg = cfg_one(&root, "hub-a"); // org-1 not a live entity
    let s = resolve_brief_scope(&conn, Some(&cfg), &root.join("f.rs").to_string_lossy());
    assert_eq!(sorted(&s.repo_group), vec!["hub-a".to_string()]);
    assert_eq!(s.org_group, None);
    assert_eq!(s.reason, Some("org_unresolved"));
    assert!(s.diagnostics.iter().any(|d| d.contains("org-1")));
}

#[test]
fn brief_scope_ignores_unusable_areas_file() {
    let (_t, root, conn) = brief_fixture();
    add_entity(&conn, "hub-a");
    add_entity(&conn, "org-1");
    write_areas_file(&root, "{{ this is not yaml");
    let cfg = cfg_one(&root, "hub-a");
    let s = resolve_brief_scope(&conn, Some(&cfg), &root.join("f.rs").to_string_lossy());
    assert_eq!(sorted(&s.repo_group), vec!["hub-a".to_string()]);
    assert_eq!(s.reason, None);
}

#[test]
fn brief_scope_group_includes_merged_losers() {
    let (_t, root, conn) = brief_fixture();
    for id in ["hub-a", "hub-b", "org-1"] {
        add_entity(&conn, id);
    }
    add_redirect(&conn, "hub-a", "hub-b");
    let cfg = cfg_one(&root, "hub-a");
    let s = resolve_brief_scope(&conn, Some(&cfg), &root.join("f.rs").to_string_lossy());
    assert_eq!(
        sorted(&s.repo_group),
        vec!["hub-a".to_string(), "hub-b".to_string()]
    );
}

#[test]
fn brief_scope_repo_wins_when_ids_in_both_groups() {
    // The resolver reports both groups as they are; "repo wins" labelling is
    // applied by the brief builder. Here the org hub was merged into the repo
    // hub, so both groups hold the same ids.
    let (_t, root, conn) = brief_fixture();
    for id in ["hub-a", "org-1"] {
        add_entity(&conn, id);
    }
    add_redirect(&conn, "org-1", "hub-a");
    let cfg = cfg_one(&root, "hub-a");
    let s = resolve_brief_scope(&conn, Some(&cfg), &root.join("f.rs").to_string_lossy());
    assert_eq!(sorted(&s.repo_group), sorted(&s.org_group));
    assert_eq!(
        sorted(&s.repo_group),
        vec!["hub-a".to_string(), "org-1".to_string()]
    );
    assert_eq!(s.reason, None);
}

#[test]
fn brief_scope_reason_precedence() {
    // unconfigured beats org_unresolved.
    let (t, root, conn) = brief_fixture();
    add_entity(&conn, "hub-a");
    let none_cfg = WisdomConfig {
        areas_paths: vec![],
        org_entity_id: Some("org-missing".to_string()),
    };
    let s = resolve_brief_scope(&conn, Some(&none_cfg), &root.to_string_lossy());
    assert_eq!(s.reason, Some("unconfigured"));
    // no_prefix beats org_unresolved.
    let cfg = cfg_one(&root, "hub-a"); // org-1 missing
    let s = resolve_brief_scope(&conn, Some(&cfg), &t.path().join("x").to_string_lossy());
    assert_eq!(s.reason, Some("no_prefix"));
    // unresolved beats org_unresolved: dead hub.
    let cfg2 = cfg_one(&root, "dead-hub");
    let s = resolve_brief_scope(&conn, Some(&cfg2), &root.join("f.rs").to_string_lossy());
    assert_eq!(s.reason, Some("unresolved"));
    // no_prefix beats unresolved: a config collision, but the path is under
    // no checkout.
    let tmp = TempDir::new().unwrap();
    let r1 = tmp.path().join("repo1");
    let r2 = tmp.path().join("repo2");
    std::fs::create_dir_all(&r1).unwrap();
    std::fs::create_dir_all(&r2).unwrap();
    let conn2 = db();
    for id in ["key-1", "key-2", "survivor", "org-1"] {
        add_entity(&conn2, id);
    }
    add_redirect(&conn2, "key-1", "survivor");
    add_redirect(&conn2, "key-2", "survivor");
    let cfg3 = WisdomConfig {
        areas_paths: vec![
            ("key-1".to_string(), r1.join(".ct-areas.yaml")),
            ("key-2".to_string(), r2.join(".ct-areas.yaml")),
        ],
        org_entity_id: Some("org-1".to_string()),
    };
    let s = resolve_brief_scope(&conn2, Some(&cfg3), &tmp.path().join("elsewhere").to_string_lossy());
    assert_eq!(s.reason, Some("no_prefix"));
    assert_eq!(sorted(&s.org_group), vec!["org-1".to_string()]);
    // ...and under a checkout the same config is still `unresolved`.
    let s = resolve_brief_scope(&conn2, Some(&cfg3), &r1.to_string_lossy());
    assert_eq!(s.reason, Some("unresolved"));
}
