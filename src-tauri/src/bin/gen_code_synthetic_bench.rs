//! Writes `tests/fixtures/code-bench-synthetic/{corpus.jsonl,queries.json,qrels.json}`.
//! Run: cargo run --bin gen_code_synthetic_bench

use serde_json::{json, Map};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("tests/fixtures/code-bench-synthetic");
    fs::create_dir_all(&dir).expect("mkdir");

    let mut corpus = BufWriter::new(File::create(dir.join("corpus.jsonl")).expect("corpus"));

    for i in 0..96 {
        let id = format!("syn-code-{i:04}");

        let text = format!(
            "// synthetic bench module\n\
             /** Widget for orbital feature */\n\
             export function FeatureOrbital{i}Pane() {{\n\
               const slot = {i};\n\
               const label = \"orbital-slot-{i}\";\n\
               return <section data-slot={{slot}}>{{label}}</section>;\n\
             }}\n\
             export const ORBIT_CFG = {{ token: \"tok-{i}\", radix: {radix} }};\n",
            radix = 128 + i,
        );

        let row = json!({"_id": id, "title": "", "text": text});
        writeln!(corpus, "{}", serde_json::to_string(&row).unwrap()).unwrap();
    }
    corpus.flush().unwrap();

    let mut queries = Map::new();
    let mut qrels = Map::new();

    for q in 0..72 {
        let doc_ix = (q * 13 + 19) % 96;
        let qid = format!("c-syn-{q:03}");
        queries.insert(
            qid.clone(),
            json!(format!(
                "TypeScript React FeatureOrbital{doc_ix}Pane orbital-slot-{doc_ix} tok-{doc_ix}",
                doc_ix = doc_ix,
            )),
        );
        qrels.insert(
            qid,
            json!([format!("syn-code-{doc_ix:04}", doc_ix = doc_ix)]),
        );
    }

    fs::write(
        dir.join("queries.json"),
        serde_json::to_vec_pretty(&queries).unwrap(),
    )
    .unwrap();
    fs::write(
        dir.join("qrels.json"),
        serde_json::to_vec_pretty(&qrels).unwrap(),
    )
    .unwrap();

    println!("Wrote code synthetic bench fixtures under {}", dir.display());
}
