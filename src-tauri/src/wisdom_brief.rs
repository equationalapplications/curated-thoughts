//! `ct wisdom brief`: the standing statements for a project (spec
//! 2026-11-28-wisdom-brief). Pure ordering and fitting live here, with no
//! database; the database read and the covered-topics index sit beside them.

use serde_json::Value;

pub const BRIEF_SCHEMA_VERSION: u32 = 1;

/// Documented contract with CTI: "CTI per-item markup plus labels <= 80
/// UTF-16 units + id" (spec 3.4). Raise to 90 if CTI also renders `verified`.
pub const ITEM_OVERHEAD: usize = 80;

/// Hard cap on the covered-topics index text, before the `max_chars / 4` cut.
pub const INDEX_CAP: usize = 400;

/// `kind_tier` value of an untyped statement (Tier 4 in the spec).
pub const UNTYPED_TIER: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Band {
    Repo,
    Org,
}

impl Band {
    pub fn as_str(self) -> &'static str {
        match self {
            Band::Repo => "repo",
            Band::Org => "org",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub band: Band,
    /// `wisdom_match::kind_tier` (0 rule/trap, 1 decision, 2 lesson, 3 untyped).
    pub tier: u8,
    pub created_at: i64,
    pub item: Value,
    pub cost: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Omitted {
    /// Every eligible statement not returned, INCLUDING `oversize`.
    pub statements: usize,
    /// Subset figure: statements whose cost alone exceeds the budget.
    pub oversize: usize,
    pub rule_trap: usize,
    pub decision: usize,
    pub lesson: usize,
    pub untyped: usize,
}

impl Omitted {
    fn count(&mut self, c: &Candidate) {
        self.statements += 1;
        match c.tier {
            0 => self.rule_trap += 1,
            1 => self.decision += 1,
            2 => self.lesson += 1,
            _ => self.untyped += 1,
        }
    }
}

/// UTF-16 code units, to match TypeScript `.length` on the TS hosts.
fn utf16(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Statement cost on the canonical rendering (spec 3.4).
pub fn item_cost(title: &str, text: &str, id: &str) -> usize {
    utf16(title) + utf16(text) + utf16(id) + ITEM_OVERHEAD
}

/// `created_at` cell coerced like `tool_dispatch::coerce_updated_at`: a TEXT
/// or malformed cell never drops the row, it sorts as 0 (oldest).
pub fn coerce_created_at(value: Option<rusqlite::types::Value>) -> i64 {
    crate::tool_dispatch::coerce_updated_at(value)
}

/// Statement budget after the index reserve (spec 3.4: the index is subtracted
/// first so statement text can never crowd it out).
pub fn statement_budget(max_chars: usize, index_cost: usize) -> usize {
    max_chars.saturating_sub(index_cost)
}

/// Spec 3.4 order: tier asc; band asc (repo first) for Tiers 1-3 only (OR-1:
/// the untyped tier interleaves repo and org); `created_at` desc; `id` asc.
pub fn order(c: &mut [Candidate]) {
    c.sort_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then_with(|| {
                if a.tier == UNTYPED_TIER {
                    std::cmp::Ordering::Equal
                } else {
                    a.band.cmp(&b.band)
                }
            })
            .then_with(|| b.created_at.cmp(&a.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Keep the longest ordered prefix that fits `budget`; never cut a statement.
/// A statement whose cost alone exceeds the budget can never fit, so it is
/// skipped (counted as `oversize`) instead of blocking everything below it.
pub fn fit(c: Vec<Candidate>, budget: usize) -> (Vec<Candidate>, Omitted) {
    let mut kept = Vec::new();
    let mut omitted = Omitted::default();
    let mut used = 0usize;
    let mut stopped = false;
    for cand in c {
        if cand.cost > budget {
            omitted.oversize += 1;
            omitted.count(&cand);
        } else if !stopped && used + cand.cost <= budget {
            used += cand.cost;
            kept.push(cand);
        } else {
            stopped = true;
            omitted.count(&cand);
        }
    }
    (kept, omitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, band: Band, tier: u8, created_at: i64, cost: usize) -> Candidate {
        Candidate {
            id: id.to_string(),
            band,
            tier,
            created_at,
            item: serde_json::json!({ "id": id }),
            cost,
        }
    }

    fn ids(c: &[Candidate]) -> Vec<&str> {
        c.iter().map(|x| x.id.as_str()).collect()
    }

    #[test]
    fn order_is_tier_then_scope_then_newest_then_id() {
        let mut v = vec![
            cand("d-org-new", Band::Org, 1, 90, 1),
            cand("d-repo-old", Band::Repo, 1, 10, 1),
            cand("r-org", Band::Org, 0, 50, 1),
            cand("d-repo-new-b", Band::Repo, 1, 80, 1),
            cand("d-repo-new-a", Band::Repo, 1, 80, 1),
            cand("l-repo", Band::Repo, 2, 5, 1),
        ];
        order(&mut v);
        assert_eq!(
            ids(&v),
            vec![
                "r-org",
                "d-repo-new-a",
                "d-repo-new-b",
                "d-repo-old",
                "d-org-new",
                "l-repo"
            ]
        );
    }

    #[test]
    fn tier4_interleaves_repo_and_org_newest_first() {
        let mut v = vec![
            cand("u1", Band::Repo, UNTYPED_TIER, 10, 1),
            cand("u2", Band::Org, UNTYPED_TIER, 30, 1),
            cand("u3", Band::Repo, UNTYPED_TIER, 20, 1),
            cand("u4", Band::Org, UNTYPED_TIER, 5, 1),
        ];
        order(&mut v);
        assert_eq!(ids(&v), vec!["u2", "u3", "u1", "u4"]);
    }

    #[test]
    fn tier4_oldest_dropped_first_globally() {
        let mut v = vec![
            cand("old-repo", Band::Repo, UNTYPED_TIER, 1, 10),
            cand("new-org", Band::Org, UNTYPED_TIER, 9, 10),
            cand("mid-repo", Band::Repo, UNTYPED_TIER, 5, 10),
        ];
        order(&mut v);
        let (kept, om) = fit(v, 20);
        assert_eq!(ids(&kept), vec!["new-org", "mid-repo"]);
        assert_eq!(om.statements, 1);
        assert_eq!(om.untyped, 1);
    }

    #[test]
    fn drop_is_bottom_up_prefix() {
        let mut v = vec![
            cand("rule", Band::Repo, 0, 1, 10),
            cand("dec", Band::Repo, 1, 1, 10),
            cand("les", Band::Repo, 2, 1, 10),
            cand("unt", Band::Repo, 3, 1, 10),
        ];
        order(&mut v);
        let (kept, om) = fit(v, 25);
        assert_eq!(ids(&kept), vec!["rule", "dec"]);
        assert_eq!((om.lesson, om.untyped, om.statements), (1, 1, 2));
    }

    #[test]
    fn lower_tier_never_kept_when_higher_dropped() {
        // The big rule does not fit after the first item; a small lesson that
        // would fit must still be dropped (no skip-and-continue).
        let mut v = vec![
            cand("a", Band::Repo, 0, 9, 10),
            cand("b-big", Band::Repo, 0, 8, 15),
            cand("c-small-lesson", Band::Repo, 2, 7, 3),
        ];
        order(&mut v);
        let (kept, om) = fit(v, 20);
        assert_eq!(ids(&kept), vec!["a"]);
        assert_eq!(om.statements, 2);
        assert_eq!(om.oversize, 0);
    }

    #[test]
    fn oversize_statement_is_skipped_not_blocking() {
        let mut v = vec![
            cand("a", Band::Repo, 0, 9, 10),
            cand("huge", Band::Repo, 0, 8, 500),
            cand("b", Band::Repo, 1, 7, 10),
        ];
        order(&mut v);
        let (kept, om) = fit(v, 30);
        assert_eq!(ids(&kept), vec!["a", "b"]);
        assert_eq!((om.statements, om.oversize, om.rule_trap), (1, 1, 1));
    }

    #[test]
    fn omitted_counts_by_tier() {
        let v = vec![
            cand("a", Band::Repo, 0, 1, 100),
            cand("b", Band::Repo, 1, 1, 100),
            cand("c", Band::Repo, 2, 1, 100),
            cand("d", Band::Repo, 3, 1, 100),
            cand("e", Band::Org, 3, 1, 100),
        ];
        let (kept, om) = fit(v, 50);
        assert!(kept.is_empty());
        assert_eq!(om.statements, 5);
        assert_eq!(om.oversize, 5);
        assert_eq!(
            om.rule_trap + om.decision + om.lesson + om.untyped,
            om.statements
        );
        assert_eq!((om.rule_trap, om.decision, om.lesson, om.untyped), (1, 1, 1, 2));
    }

    #[test]
    fn cost_formula_uses_utf16_units() {
        // 'é' is 1 unit (2 UTF-8 bytes); the astral '𝄞' is 2 units (4 bytes).
        let title = "é𝄞"; // 3 units
        let text = "日本"; // 2 units
        assert_eq!(item_cost(title, text, "id1"), 3 + 2 + 3 + ITEM_OVERHEAD);
        assert_eq!(ITEM_OVERHEAD, 80);
    }

    #[test]
    fn text_created_at_keeps_row() {
        use rusqlite::types::Value as V;
        assert_eq!(coerce_created_at(Some(V::Integer(7))), 7);
        assert_eq!(coerce_created_at(Some(V::Text("12".into()))), 12);
        assert_eq!(coerce_created_at(Some(V::Text("garbage".into()))), 0);
        assert_eq!(coerce_created_at(Some(V::Null)), 0);
        assert_eq!(coerce_created_at(None), 0);
    }

    #[test]
    fn index_reserve_is_subtracted_first() {
        assert_eq!(statement_budget(2200, 100), 2100);
        assert_eq!(statement_budget(600, 700), 0);
    }
}
