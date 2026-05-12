//! Writes `tests/fixtures/yaml-bench-synthetic/{corpus.jsonl,queries.json,qrels.json}`.
//! Run: cargo run --example gen_yaml_synthetic_bench --features dev-tools

use serde_json::{json, Map};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("tests/fixtures/yaml-bench-synthetic");
    fs::create_dir_all(&dir).expect("mkdir");

    let mut corpus = BufWriter::new(File::create(dir.join("corpus.jsonl")).expect("corpus"));

    for i in 0..80 {
        let id = format!("syn-yaml-{i:04}");
        let replicas = if i % 7 == 0 { 1 } else { 3 };
        let port = 8080 + (i % 50);

        let text = format!(
            "apiVersion: apps/v1\n\
             kind: Deployment\n\
             metadata:\n\
               name: bench-app-{i}\n\
               labels:\n\
                 bench.suite: curated-thoughts\n\
                 bench.ordinal: \"{i}\"\n\
             spec:\n\
               replicas: {replicas}\n\
               selector:\n\
                 matchLabels:\n\
                   app: bench-app-{i}\n\
               template:\n\
                 metadata:\n\
                   labels:\n\
                     app: bench-app-{i}\n\
                 spec:\n\
                   containers:\n\
                   - name: workload\n\
                     image: registry.curatedthoughts.invalid/bench:{i}\n\
                     ports:\n\
                     - containerPort: {port}\n\
                     env:\n\
                     - name: BENCH_SEQUENCE\n\
                       value: \"{i}\"\n"
        );

        let row = json!({"_id": id, "title": "", "text": text});
        writeln!(corpus, "{}", serde_json::to_string(&row).unwrap()).unwrap();
    }
    corpus.flush().unwrap();

    let mut queries = Map::new();
    let mut qrels = Map::new();

    for q in 0..72 {
        let doc_ix = (q * 11 + 17) % 80;
        let qid = format!("y-syn-{q:03}");
        let port_want = 8080 + (doc_ix % 50);
        queries.insert(
            qid.clone(),
            json!(format!(
                "Kubernetes Deployment bench-app-{doc_ix} registry.curatedthoughts.invalid/bench:{doc_ix} workload port {port_want}",
                doc_ix = doc_ix,
                port_want = port_want,
            )),
        );
        qrels.insert(
            qid,
            json!([format!("syn-yaml-{doc_ix:04}", doc_ix = doc_ix)]),
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

    println!(
        "Wrote YAML synthetic bench fixtures under {}",
        dir.display()
    );
}
