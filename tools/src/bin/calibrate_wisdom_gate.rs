//! calibrate_wisdom_gate — sets the `ct wisdom match` abstention floor for
//! one embed model (issue #265; CT INTENT rule 7 + workflow 4).
//!
//! Builds a SCRATCH brain in a temp dir from fixture facts, embeds facts and
//! probes with the REAL profile, sweeps floors 0.20..=0.90 through
//! `wisdom_match_with_floor` itself, and picks the floor that maximises
//! hit@2 subject to FP rate <= 0.05 (ties -> higher floor). Never touches the
//! live brain. Refuses to run with CURATED_EMBED_STUB set.
//!
//! `--scheme` selects the cell: BOTH sides are built through the production
//! text functions for that scheme (`embed_scheme::doc_text_for_scheme` for
//! facts, `embed_scheme::query_text_for_scheme` for probes), the scratch rows
//! are stamped with it, and the sweep declares it to `wisdom_match_with_floor`
//! — so a raw re-freeze is genuinely raw on both sides and an instr1 freeze
//! applies the instruction exactly once per side.

use anyhow::{bail, Context, Result};
use clap::Parser;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;

use tauri_app_lib::embed_scheme::{
    doc_text_for_scheme, floor_key_for, query_text_for_scheme, Scheme, QUERY_INSTRUCTION_PREFIX,
};
use tauri_app_lib::embedder::{embed_batch, EmbedProfile};
use tauri_app_lib::wisdom_match::{gate_model_key, wisdom_match_with_floor};

const FP_BOUND: f64 = 0.05;
const BATCH: usize = 32;
const NOW_MS: i64 = 4_102_444_800_000; // 2100-01-01: nothing in the fixture expires

#[derive(Parser)]
struct Args {
    #[arg(long)]
    facts: PathBuf,
    #[arg(long)]
    probes: PathBuf,
    /// EmbedProfile JSON (as in vault config.json `embed_profile`).
    /// Default is the production embedder: OpenRouter `qwen/qwen3-embedding-4b`
    /// (key from `OPENROUTER_API_KEY` or `EMBED_API_KEY`).
    #[arg(
        long,
        default_value = r#"{"type":"external","base_url":"https://openrouter.ai/api/v1","model":"qwen/qwen3-embedding-4b"}"#
    )]
    profile: String,
    /// Write vectors.json.gz + expected.json here (the regression fixture).
    #[arg(long)]
    freeze: Option<PathBuf>,
    /// Embed scheme to calibrate: `raw` (both sides verbatim — the
    /// `tests/fixtures/wisdom_gate` snapshot) or `instr1` (the byte-exact
    /// instruction on BOTH facts and probes, spec-rev2 cell E — the
    /// `tests/fixtures/wisdom_gate_instr1` snapshot). Freeze each into its
    /// own directory; the resulting floor registers under
    /// `floor_key_for(model_key, scheme)`.
    #[arg(long, default_value = "raw", value_parser = parse_scheme)]
    scheme: Scheme,
}

fn parse_scheme(s: &str) -> Result<Scheme, String> {
    Scheme::parse(s).map_err(|e| e.to_string())
}

#[derive(Deserialize, Serialize, Clone)]
struct Fact {
    id: String,
    title: String,
    body: String,
    source_type: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    vector: Vec<f32>,
}

#[derive(Deserialize, Serialize, Clone)]
struct Probe {
    text: String,
    expect: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    vector: Vec<f32>,
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(p: &PathBuf) -> Result<(Vec<T>, String)> {
    let raw = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
    let sha = hex::encode(Sha256::digest(&raw));
    let items = String::from_utf8(raw)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<Vec<T>, _>>()?;
    Ok((items, sha))
}

fn embed_all(profile: &EmbedProfile, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
    let mut out = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(BATCH) {
        let got = embed_batch(profile, chunk.to_vec())?;
        if got.len() != chunk.len() {
            bail!(
                "embed backend returned {} vectors for {}",
                got.len(),
                chunk.len()
            );
        }
        out.extend(got);
    }
    Ok(out)
}

fn main() -> Result<()> {
    let args = Args::parse();
    if std::env::var_os("CURATED_EMBED_STUB").is_some() {
        bail!("refusing to calibrate with CURATED_EMBED_STUB set — real embeddings only");
    }
    let profile: EmbedProfile = serde_json::from_str(&args.profile).context("--profile")?;
    let key = gate_model_key(&profile, None);
    let scheme = args.scheme;
    let (mut facts, facts_sha) = read_jsonl::<Fact>(&args.facts)?;
    let (mut probes, probes_sha) = read_jsonl::<Probe>(&args.probes)?;
    let n_rel = probes.iter().filter(|p| !p.expect.is_empty()).count();
    let n_irr = probes.len() - n_rel;
    if facts.len() < 100 || probes.len() < 200 || n_rel < 80 || n_irr < 80 {
        bail!(
            "fixture too small: facts {}, relevant {n_rel}, irrelevant {n_irr}",
            facts.len()
        );
    }

    let fact_vecs = embed_all(
        &profile,
        facts
            .iter()
            .map(|f| doc_text_for_scheme(&f.title, &f.body, scheme))
            .collect(),
    )?;
    let probe_vecs = embed_all(
        &profile,
        probes
            .iter()
            .map(|p| query_text_for_scheme(&p.text, scheme))
            .collect(),
    )?;
    for (f, v) in facts.iter_mut().zip(fact_vecs) {
        f.vector = v;
    }
    for (p, v) in probes.iter_mut().zip(probe_vecs) {
        p.vector = v;
    }

    let tmp = tempfile::tempdir()?;
    let conn = tauri_app_lib::db::connection::open_app_db(&tmp.path().join("brain.db"), None)?;
    for f in &facts {
        let blob: Vec<u8> = f.vector.iter().flat_map(|x| x.to_le_bytes()).collect();
        conn.execute(
            "INSERT INTO llm_wiki_entries (
                id, entity_id, title, body, tags, confidence, source_type,
                source_hash, source_ref, created_at, updated_at, last_accessed_at,
                access_count, deleted_at, embedding_blob, embed_scheme, embedding
             ) VALUES (?1, 'ent_calibration', ?2, ?3, '[]', 'inferred', ?4,
                       NULL, NULL, 100, 100, NULL, 0, NULL, ?5, ?6, NULL)",
            params![f.id, f.title, f.body, f.source_type, blob, scheme.as_str()],
        )?;
    }

    let mut best: Option<(u32, f64, f64)> = None; // (floor_pct, hit, fp)
    println!("floor  hit@2   fp_rate");
    for pct in 20u32..=90 {
        let floor = pct as f32 / 100.0;
        let (mut hits, mut fps) = (0usize, 0usize);
        for p in &probes {
            let m = wisdom_match_with_floor(
                &conn,
                &p.vector,
                &key,
                scheme,
                Some(floor),
                2,
                &[],
                NOW_MS,
            )?;
            if p.expect.is_empty() {
                fps += usize::from(!m.entries.is_empty());
            } else {
                hits += usize::from(m.entries.iter().any(|e| p.expect.contains(&e.id)));
            }
        }
        let hit = hits as f64 / n_rel as f64;
        let fp = fps as f64 / n_irr as f64;
        println!("{floor:.2}   {hit:.3}   {fp:.3}");
        if fp <= FP_BOUND && best.is_none_or(|(_, h, _)| hit >= h) {
            best = Some((pct, hit, fp)); // >= keeps the HIGHER floor on ties
        }
    }
    let Some((pct, hit, fp)) = best else {
        bail!("no floor meets FP <= {FP_BOUND}; model stays uncalibrated");
    };
    let mut expected = serde_json::json!({
        "model_key": key, "floor": pct as f64 / 100.0, "hit_at_2": hit, "fp_rate": fp,
        "n_relevant": n_rel, "n_irrelevant": n_irr,
        "facts_sha256": facts_sha, "probes_sha256": probes_sha,
    });
    if scheme == Scheme::Instr1 {
        // Recorded so the bench reader and future sessions can tell an
        // instr1 snapshot from a raw one at a glance (same shape the
        // committed `wisdom_gate_instr1/expected.json` carries).
        expected["query_prefix"] = serde_json::json!(QUERY_INSTRUCTION_PREFIX);
        expected["doc_prefix"] = serde_json::json!(QUERY_INSTRUCTION_PREFIX);
    }
    println!("{}", serde_json::to_string_pretty(&expected)?);
    eprintln!(
        "register as WISDOM_GATE_FLOORS[{:?}] (scheme {})",
        floor_key_for(&key, scheme),
        scheme.as_str()
    );

    if let Some(dir) = args.freeze {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("expected.json"),
            serde_json::to_vec_pretty(&expected)?,
        )?;
        let frozen = serde_json::json!({"model_key": key, "facts": facts, "probes": probes});
        let file = std::fs::File::create(dir.join("vectors.json.gz"))?;
        let mut gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        gz.write_all(&serde_json::to_vec(&frozen)?)?;
        gz.finish()?;
    }
    Ok(())
}
