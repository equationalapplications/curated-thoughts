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

// ---- covered-topics index (spec 3.5) ------------------------------------

/// Names (hubs, areas) are cut to this many chars in the index.
const NAME_CUT: usize = 40;
/// At most this many areas are listed.
const MAX_AREAS: usize = 8;
const HINT: &str = "More on demand: ct wisdom match --path=<file>, or wiki_context.";
const AREAS_LEAD: &str = "Areas not listed, surfaced when you work on their files:";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AreaCount {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Counts {
    pub repo: usize,
    pub org: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Index {
    pub text: String,
    pub areas: Vec<AreaCount>,
    pub counts: Counts,
}

/// One band's line: its (optional) hub display name, eligible count and shown.
#[derive(Debug, Clone)]
pub struct BandInfo {
    pub name: Option<String>,
    pub count: usize,
    pub shown: usize,
}

/// Everything the index renders from.
#[derive(Debug, Clone)]
pub struct IndexInput {
    pub repo: Option<BandInfo>,
    pub org: Option<BandInfo>,
    /// Repo-hub area facts as `(area name, eligible count)`, any order.
    pub areas: Vec<(String, usize)>,
}

/// How much of the index is kept. Ladder (spec 3.5): areas shed from the
/// tail, then hub names, then the area clause, then the hint, then everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShedLevel {
    /// 0 full, 1 no hub names, 2 no area clause, 3 no hint, 4 empty.
    stage: u8,
    /// Areas listed (stage 0 only; stage 1 keeps the same value).
    areas_kept: usize,
}

fn cut(s: &str) -> String {
    s.chars().take(NAME_CUT).collect()
}

fn sorted_areas(input: &IndexInput) -> Vec<(String, usize)> {
    let mut a: Vec<(String, usize)> = input.areas.iter().map(|(n, c)| (cut(n), *c)).collect();
    a.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
    a
}

/// Index cost in the budget: text plus the `\n\n` separator, 0 when empty.
pub fn index_cost(index: &Index) -> usize {
    if index.text.is_empty() {
        0
    } else {
        utf16(&index.text) + 2
    }
}

/// Index cap for a given `max_chars` (spec 3.4).
pub fn index_cap(max_chars: usize) -> usize {
    INDEX_CAP.min(max_chars / 4)
}

/// Re-render at a fixed shed level with the input's `shown` values. Never
/// sheds further: a smaller `shown` is never wider than the one the level was
/// chosen for.
pub fn render_index_at(level: ShedLevel, input: &IndexInput) -> Index {
    let all = sorted_areas(input);
    let listed: Vec<AreaCount> = all
        .iter()
        .take(MAX_AREAS)
        .map(|(n, c)| AreaCount { name: n.clone(), count: *c })
        .collect();
    let counts = Counts {
        repo: input.repo.as_ref().map_or(0, |b| b.count),
        org: input.org.as_ref().map_or(0, |b| b.count),
    };
    let nothing = counts.repo + counts.org == 0 && all.is_empty();
    if nothing || level.stage >= 4 {
        return Index { text: String::new(), areas: listed, counts };
    }
    let mut parts: Vec<String> = Vec::new();
    let named = level.stage == 0;
    let band = |label: &str, b: &BandInfo, first: bool| {
        let name = match (&b.name, named) {
            (Some(n), true) => format!(" {}", cut(n)),
            _ => String::new(),
        };
        if first {
            format!("{label}{name} ({} statements, {} shown)", b.count, b.shown)
        } else {
            format!("{label}{name} ({}, {} shown)", b.count, b.shown)
        }
    };
    let mut bands: Vec<String> = Vec::new();
    if let Some(b) = &input.repo {
        bands.push(band("repo", b, true));
    }
    if let Some(b) = &input.org {
        bands.push(band("org", b, input.repo.is_none()));
    }
    if !bands.is_empty() {
        parts.push(format!("Covered: {}.", bands.join("; ")));
    }
    if level.stage <= 1 && !all.is_empty() {
        let kept = level.areas_kept.min(listed.len());
        let mut items: Vec<String> = listed
            .iter()
            .take(kept)
            .map(|a| format!("{} ({})", a.name, a.count))
            .collect();
        let more = all.len() - kept;
        if more > 0 {
            items.push(format!("... +{more} more"));
        }
        parts.push(format!("{AREAS_LEAD} {}.", items.join(", ")));
    }
    if level.stage <= 2 {
        parts.push(HINT.to_string());
    }
    Index { text: parts.join(" "), areas: listed, counts }
}

/// Build the index at the finest level whose text fits `cap` (UTF-16 units).
/// Call with `shown == count` (k := n) to fix the reserve and shed level; the
/// caller then fits statements and re-renders at the SAME level (spec 3.4).
pub fn build_index(input: &IndexInput, cap: usize) -> (Index, ShedLevel) {
    let kmax = input.areas.len().min(MAX_AREAS);
    let mut levels: Vec<ShedLevel> = (0..=kmax)
        .rev()
        .map(|k| ShedLevel { stage: 0, areas_kept: k })
        .collect();
    levels.push(ShedLevel { stage: 1, areas_kept: 0 });
    levels.push(ShedLevel { stage: 2, areas_kept: 0 });
    levels.push(ShedLevel { stage: 3, areas_kept: 0 });
    for level in levels {
        let idx = render_index_at(level, input);
        if utf16(&idx.text) <= cap {
            return (idx, level);
        }
    }
    let level = ShedLevel { stage: 4, areas_kept: 0 };
    (render_index_at(level, input), level)
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

    fn input(repo_n: usize, org_n: usize, areas: &[(&str, usize)]) -> IndexInput {
        IndexInput {
            repo: Some(BandInfo { name: Some("Repo Hub".into()), count: repo_n, shown: repo_n }),
            org: Some(BandInfo { name: Some("Company".into()), count: org_n, shown: org_n }),
            areas: areas.iter().map(|(n, c)| (n.to_string(), *c)).collect(),
        }
    }

    #[test]
    fn index_lists_areas_count_desc_name_asc() {
        let (idx, _) = build_index(&input(3, 1, &[("b", 2), ("a", 2), ("z", 5)]), 400);
        assert!(idx.text.contains("z (5), a (2), b (2)"), "{}", idx.text);
        assert_eq!(idx.areas[0], AreaCount { name: "z".into(), count: 5 });
        assert!(idx.text.starts_with("Covered: repo Repo Hub (3 statements, 3 shown); org Company (1, 1 shown)."));
        assert!(idx.text.ends_with(HINT));
    }

    #[test]
    fn index_caps_areas_at_eight_with_more() {
        let areas: Vec<(String, usize)> = (0..11).map(|i| (format!("area{i:02}"), 20 - i)).collect();
        let mut inp = input(3, 0, &[]);
        inp.areas = areas;
        let (idx, _) = build_index(&inp, 1000);
        assert_eq!(idx.areas.len(), 8);
        assert!(idx.text.contains("... +3 more"), "{}", idx.text);
        assert!(!idx.text.contains("area08"));
    }

    #[test]
    fn index_sheds_areas_then_names_to_fit_cap() {
        let areas: Vec<(String, usize)> = (0..8).map(|i| (format!("area-number-{i}"), 9)).collect();
        let mut inp = input(30, 20, &[]);
        inp.areas = areas;
        let (full, l0) = build_index(&inp, 1000);
        assert_eq!(l0.stage, 0);
        let cap = utf16(&full.text) - 1;
        let (less, l1) = build_index(&inp, cap);
        assert!(utf16(&less.text) <= cap);
        assert!(l1.areas_kept < 8 || l1.stage > 0);
        // Tight cap: names go before the area clause.
        let (tight, l2) = build_index(&inp, 150);
        assert!(utf16(&tight.text) <= 150);
        assert!(l2.stage >= 1, "{:?}", l2);
        assert!(!tight.text.contains("Repo Hub"));
    }

    #[test]
    fn index_empty_when_nothing_to_say() {
        let inp = IndexInput { repo: None, org: None, areas: vec![] };
        let (idx, _) = build_index(&inp, 400);
        assert_eq!(idx.text, "");
        let zero = input(0, 0, &[]);
        assert_eq!(build_index(&zero, 400).0.text, "");
    }

    #[test]
    fn index_text_never_exceeds_cap() {
        for cap in [0usize, 20, 60, 100, 150, 400] {
            let (idx, _) = build_index(&input(12, 5, &[("auth", 3), ("deploy", 2)]), cap);
            assert!(utf16(&idx.text) <= cap, "cap {cap}: {}", idx.text);
        }
    }

    #[test]
    fn index_cap_is_min_400_quarter_of_max() {
        assert_eq!(index_cap(600), 150);
        assert_eq!(index_cap(2200), 400.min(550));
        assert_eq!(index_cap(2200), 400);
        assert_eq!(index_cap(8000), 400);
    }

    #[test]
    fn index_cost_zero_when_text_empty() {
        let e = Index { text: String::new(), areas: vec![], counts: Counts::default() };
        assert_eq!(index_cost(&e), 0);
        let f = Index { text: "ab".into(), areas: vec![], counts: Counts::default() };
        assert_eq!(index_cost(&f), 4);
    }

    #[test]
    fn index_k_crossing_digit_boundary_n10_k9() {
        let mut inp = input(10, 0, &[]);
        inp.org = None;
        let (full, level) = build_index(&inp, 400);
        inp.repo.as_mut().unwrap().shown = 9;
        let real = render_index_at(level, &inp);
        assert!(utf16(&real.text) <= utf16(&full.text));
        assert!(real.text.contains("9 shown"));
        // And the other way: n=100 -> k=99 is shorter.
        let mut big = input(100, 0, &[]);
        big.org = None;
        let (f2, l2) = build_index(&big, 400);
        big.repo.as_mut().unwrap().shown = 99;
        assert!(utf16(&render_index_at(l2, &big).text) < utf16(&f2.text));
    }

    #[test]
    fn index_at_max_chars_600_fits_non_empty() {
        let cap = index_cap(600);
        let long = "n".repeat(60);
        let mut inp = input(9999, 9999, &[("auth", 3)]);
        inp.repo.as_mut().unwrap().name = Some(long.clone());
        inp.org.as_mut().unwrap().name = Some(long);
        let (idx, _) = build_index(&inp, cap);
        assert!(!idx.text.is_empty());
        assert!(utf16(&idx.text) <= cap);
        // Shortest realistic form: repo only, 40-char name, hint kept.
        let mut one = input(9999, 0, &[]);
        one.org = None;
        one.repo.as_mut().unwrap().name = Some("x".repeat(40));
        let (i1, _) = build_index(&one, cap);
        assert!(i1.text.ends_with(HINT), "{}", i1.text);
    }

    #[test]
    fn index_shed_ladder_ends_hint_then_empty() {
        let inp = input(12, 5, &[("auth", 3)]);
        let (a, la) = build_index(&inp, 400);
        assert_eq!(la.stage, 0);
        assert!(a.text.contains(HINT));
        // Just the bare counts line fits, no hint.
        let bare = render_index_at(ShedLevel { stage: 3, areas_kept: 0 }, &inp);
        assert!(!bare.text.contains(HINT) && !bare.text.is_empty());
        let (b, lb) = build_index(&inp, utf16(&bare.text));
        assert_eq!(lb.stage, 3);
        assert_eq!(b.text, bare.text);
        let (c, lc) = build_index(&inp, utf16(&bare.text) - 1);
        assert_eq!(lc.stage, 4);
        assert_eq!(c.text, "");
        assert_eq!(index_cost(&c), 0);
    }
}
