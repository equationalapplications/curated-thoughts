//! recall-eligibility-provenance (PR-1): the CLI/sidecar copy of
//! `rank_wiki_entries` skips superseded/expired/forgotten/deleted facts,
//! labels items with provenance/verified/kind, ranks by the shared confidence
//! ordinal with a deterministic id tie-break, and degrades on pre-migration
//! shapes. Seeds through the real migration path (`with_seeded_brain`).

mod common;

use common::{brain_db, seed_fact, with_seeded_brain};
use curated_thoughts_tools::queries::rank_wiki_entries_at;
use rusqlite::Connection;

const NOW: i64 = 1_000_000;

fn ids_at(conn: &Connection) -> Vec<String> {
    rank_wiki_entries_at(conn, "Body", 20, NOW)
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap().to_string())
        .collect()
}

fn set(conn: &Connection, id: &str, assignments: &str) {
    conn.execute(&format!("UPDATE llm_wiki_entries SET {assignments} WHERE id = ?1"), [id])
        .unwrap();
}

#[test]
fn superseded_with_past_superseded_at_is_excluded() {
    with_seeded_brain(|| {
        seed_fact("live");
        seed_fact("old");
        let conn = brain_db();
        set(&conn, "old", "superseded_by = 'live', superseded_at = 500");
        assert_eq!(ids_at(&conn), vec!["live"]);
    });
}

#[test]
fn future_superseded_at_is_still_included() {
    with_seeded_brain(|| {
        seed_fact("soon");
        let conn = brain_db();
        set(&conn, "soon", "superseded_by = 'x', superseded_at = 5000000");
        assert_eq!(ids_at(&conn), vec!["soon"]);
    });
}

#[test]
fn superseded_by_with_null_superseded_at_is_excluded() {
    // Matches the gate: `NULL > :now` is NULL, i.e. not eligible.
    with_seeded_brain(|| {
        seed_fact("live");
        seed_fact("nullat");
        let conn = brain_db();
        set(&conn, "nullat", "superseded_by = 'live'");
        assert_eq!(ids_at(&conn), vec!["live"]);
    });
}

#[test]
fn expired_valid_to_excluded_future_valid_to_included() {
    with_seeded_brain(|| {
        seed_fact("expired");
        seed_fact("fresh");
        let conn = brain_db();
        set(&conn, "expired", "valid_to = 500");
        set(&conn, "fresh", "valid_to = 5000000");
        assert_eq!(ids_at(&conn), vec!["fresh"]);
    });
}

#[test]
fn forgotten_and_deleted_are_excluded() {
    with_seeded_brain(|| {
        seed_fact("kept");
        seed_fact("gone");
        seed_fact("deleted");
        let conn = brain_db();
        set(&conn, "deleted", "deleted_at = 1");
        conn.execute(
            "INSERT INTO llm_wiki_events
               (id, entity_id, event_type, summary, related_entry_id, created_at, forgotten_at)
             VALUES ('e-gone', 'ent_t', 'synthesized', 's', 'gone', 100, 5)",
            [],
        )
        .unwrap();
        assert_eq!(ids_at(&conn), vec!["kept"]);
    });
}

#[test]
fn items_carry_source_type_and_verified() {
    with_seeded_brain(|| {
        seed_fact("conf");
        seed_fact("inf");
        seed_fact("junk");
        let conn = brain_db();
        set(
            &conn,
            "conf",
            r#"confidence = 'confirmed', source_type = 'user_confirmed',
               okf_verified = '[{"by":"human:t","at":"2026-11-27T00:00:00Z"}]'"#,
        );
        set(&conn, "junk", "source_type = 'bogus'");
        let hits = rank_wiki_entries_at(&conn, "Body", 20, NOW).unwrap();
        let by = |id: &str| hits.iter().find(|h| h["id"] == id).unwrap().clone();
        let conf = by("conf");
        assert_eq!(conf["source_type"], "user_confirmed");
        assert_eq!(conf["verified"]["by"], "human:t");
        assert_eq!(conf["verified"]["at"], "2026-11-27T00:00:00Z");
        let inf = by("inf");
        assert_eq!(inf["source_type"], "librarian_inferred");
        assert!(inf.get("verified").is_none());
        assert!(by("junk").get("source_type").is_none(), "out-of-vocabulary omitted");
    });
}

#[test]
fn kind_is_normalized() {
    with_seeded_brain(|| {
        for (id, t) in [("r", "rule"), ("f", "fact"), ("b", "bogus"), ("c", "Rule")] {
            seed_fact(id);
            set(&brain_db(), id, &format!("okf_type = '{t}'"));
        }
        seed_fact("n"); // NULL okf_type
        let conn = brain_db();
        let hits = rank_wiki_entries_at(&conn, "Body", 20, NOW).unwrap();
        for h in &hits {
            if h["id"] == "r" {
                assert_eq!(h["kind"], "rule");
            } else {
                assert!(h.get("kind").is_none(), "{}", h["id"]);
            }
        }
        assert_eq!(hits.len(), 5);
    });
}

#[test]
fn confidence_ordinal_orders_confirmed_before_inferred() {
    // Distinct updated_at run the OPPOSITE way (older = higher ordinal), so
    // only the ordinal can produce this order (spec 3.4: confirmed, certain,
    // verified = 3 > inferred = 2 > other = 0).
    with_seeded_brain(|| {
        for (id, conf, at) in [
            ("tent", "tentative", 500),
            ("inf", "inferred", 400),
            ("cer", "certain", 300),
            ("ver", "verified", 200),
            ("con", "confirmed", 100),
        ] {
            seed_fact(id);
            set(&brain_db(), id, &format!("confidence = '{conf}', updated_at = {at}"));
        }
        assert_eq!(ids_at(&brain_db()), vec!["cer", "ver", "con", "inf", "tent"]);
    });
}

#[test]
fn ties_break_by_id() {
    with_seeded_brain(|| {
        // >= 8 rows identical on overlap/confidence/updated_at, inserted in a
        // scrambled id order: an accidental HashMap-order pass is negligible.
        for id in ["h", "c", "f", "a", "j", "d", "i", "b", "g", "e"] {
            seed_fact(id);
        }
        assert_eq!(
            ids_at(&brain_db()),
            vec!["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]
        );
    });
}

/// Hand-built pre-migration table: every column in `ALL` except those in
/// `missing`; one matching row. Each shape must still return it.
fn degraded(missing: &[&str]) -> Connection {
    const ALL: &[(&str, &str)] = &[
        ("id", "TEXT PRIMARY KEY"),
        ("entity_id", "TEXT"),
        ("title", "TEXT"),
        ("body", "TEXT"),
        ("source_ref", "TEXT"),
        ("confidence", "TEXT"),
        ("updated_at", "INTEGER"),
        ("deleted_at", "INTEGER"),
        ("superseded_by", "TEXT"),
        ("superseded_at", "INTEGER"),
        ("valid_to", "INTEGER"),
        ("source_type", "TEXT"),
        ("okf_type", "TEXT"),
        ("okf_verified", "TEXT"),
    ];
    let cols: Vec<String> = ALL
        .iter()
        .filter(|(n, _)| !missing.contains(n))
        .map(|(n, t)| format!("{n} {t}"))
        .collect();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&format!("CREATE TABLE llm_wiki_entries ({});", cols.join(", ")))
        .unwrap();
    let events = if missing.contains(&"forgotten_at") {
        "CREATE TABLE llm_wiki_events (id TEXT, related_entry_id TEXT);"
    } else {
        "CREATE TABLE llm_wiki_events (id TEXT, related_entry_id TEXT, forgotten_at INTEGER);"
    };
    conn.execute_batch(events).unwrap();
    conn.execute(
        "INSERT INTO llm_wiki_entries (id, entity_id, title, body, confidence, updated_at)
         VALUES ('d1', 'ent', 'Title', 'Body text', 'inferred', 100)",
        [],
    )
    .unwrap();
    // Where the shape still has the columns, add a superseded and an expired
    // row: the arm that exists must keep filtering while the missing arm is
    // tolerated.
    if !missing.contains(&"superseded_by") && !missing.contains(&"superseded_at") {
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, confidence, updated_at, superseded_by, superseded_at)
             VALUES ('sup', 'ent', 'Title', 'Body text', 'inferred', 100, 'd1', 500)",
            [],
        )
        .unwrap();
    }
    if !missing.contains(&"valid_to") {
        conn.execute(
            "INSERT INTO llm_wiki_entries (id, entity_id, title, body, confidence, updated_at, valid_to)
             VALUES ('exp', 'ent', 'Title', 'Body text', 'inferred', 100, 500)",
            [],
        )
        .unwrap();
    }
    conn
}

#[test]
fn degraded_shapes_still_return_rows_without_sql_errors() {
    let shapes: &[&[&str]] = &[
        &[],
        &["superseded_by", "superseded_at"],
        // Half-migrated supersession (V24 adds columns one ALTER at a time):
        // the arm needs both, so it is omitted and recall still succeeds.
        &["superseded_at"],
        &["superseded_by"],
        &["valid_to"],
        &["superseded_by", "superseded_at", "valid_to", "forgotten_at"],
        &["forgotten_at"],
        &["okf_type"],
        &["okf_verified"],
        // `source_type` predates the events table's `forgotten_at` (the forgotten
        // arm reads `e.source_type`), so a brain without it has no such arm.
        &["source_type", "forgotten_at"],
        &["source_type", "forgotten_at", "okf_type", "okf_verified"],
    ];
    for missing in shapes {
        let conn = degraded(missing);
        let hits = rank_wiki_entries_at(&conn, "Body", 5, NOW)
            .unwrap_or_else(|e| panic!("shape missing {missing:?}: {e}"));
        assert_eq!(hits.len(), 1, "shape missing {missing:?}");
        assert_eq!(hits[0]["id"], "d1");
    }
}

/// Pinned fail-closed behavior: no `source_type` column but the events table
/// has `forgotten_at` + `related_entry_id` makes the forgotten arm reference a
/// missing column. Unreachable via real migrations; recall must error.
#[test]
fn forgotten_arm_without_source_type_fails_closed() {
    let conn = degraded(&["source_type"]);
    assert!(rank_wiki_entries_at(&conn, "Body", 5, NOW).is_err());
}

#[test]
fn blob_okf_type_row_is_still_returned_without_kind() {
    let conn = degraded(&[]);
    conn.execute("UPDATE llm_wiki_entries SET okf_type = x'0102' WHERE id = 'd1'", [])
        .unwrap();
    let hits = rank_wiki_entries_at(&conn, "Body", 5, NOW).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].get("kind").is_none());
}
