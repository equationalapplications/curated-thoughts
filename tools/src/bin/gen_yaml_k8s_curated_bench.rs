//! Kubernetes-style **curated** bench (richer than synthetic): Services, Ingress rules, ConfigMap data.
//! Writes `tests/fixtures/yaml-bench-k8s-curated/`.
//! Run: cargo run --bin gen_yaml_k8s_curated_bench

use serde_json::{json, Map};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("tests/fixtures/yaml-bench-k8s-curated");
    fs::create_dir_all(&dir).expect("mkdir");

    let mut corpus = BufWriter::new(File::create(dir.join("corpus.jsonl")).expect("corpus"));

    let kinds: [&str; 5] = ["Deployment", "Service", "ConfigMap", "Secret", "Ingress"];

    for i in 0..48 {
        let id = format!("k8s-cur-{i:03}");
        let kind = kinds[i % kinds.len()];
        let name = format!("workload-cur-{i}");
        let ns = format!("team-{}", (i % 6) + 1);

        let text = match kind {
            "Deployment" => format!(
                "apiVersion: apps/v1\n\
                 kind: Deployment\n\
                 metadata:\n\
                   name: {name}\n\
                   namespace: {ns}\n\
                 spec:\n\
                   replicas: {}\n\
                   selector:\n\
                     matchLabels:\n\
                       workload: {name}\n\
                   template:\n\
                     metadata:\n\
                       labels:\n\
                         workload: {name}\n\
                     spec:\n\
                       containers:\n\
                       - name: api\n\
                         image: harbor.curatedthoughts.invalid/libs/bench:{i}\n\
                         resources:\n\
                           requests:\n\
                             memory: \"{}Mi\"\n\
                             cpu: \"{}m\"\n",
                if i % 4 == 0 { 1 } else { 2 },
                128 + (i % 200),
                50 + (i % 400),
            ),
            "Service" => format!(
                "apiVersion: v1\n\
                 kind: Service\n\
                 metadata:\n\
                   name: svc-{name}\n\
                   namespace: {ns}\n\
                 spec:\n\
                   type: ClusterIP\n\
                   selector:\n\
                     workload: workload-cur-{i}\n\
                   ports:\n\
                   - name: http-cur-{i}\n\
                     port: {}\n\
                     targetPort: {}\n",
                3000 + (i % 5000),
                8080 + (i % 100),
            ),
            "ConfigMap" => format!(
                "apiVersion: v1\n\
                 kind: ConfigMap\n\
                 metadata:\n\
                   name: cfg-{name}\n\
                   namespace: {ns}\n\
                 data:\n\
                   app.ini: |\n\
                     [runtime]\n\
                     queue=cur-{i}\n\
                     flush_interval_ms={}\n\
                   policy.json: |\n\
                     {{\"version\":1,\"shard\":{}}}\n",
                250 + i,
                i,
            ),
            "Secret" => format!(
                "apiVersion: v1\n\
                 kind: Secret\n\
                 metadata:\n\
                   name: sec-{name}\n\
                   namespace: {ns}\n\
                 type: Opaque\n\
                 stringData:\n\
                   SESSION_KEY: \"CUR-BENCH-{i}-KEY\"\n\
                   db_url: postgres://cur/{i}\n",
            ),
            "Ingress" => {
                let svc_port = 3000 + ((i * 3) % 5000);
                format!(
                    "apiVersion: networking.k8s.io/v1\n\
                     kind: Ingress\n\
                     metadata:\n\
                       name: ing-{name}\n\
                       namespace: {ns}\n\
                     spec:\n\
                       rules:\n\
                       - host: cur-bench-{i}.example.invalid\n\
                         http:\n\
                           paths:\n\
                           - path: /api/v{i}/stream\n\
                             pathType: Prefix\n\
                             backend:\n\
                               service:\n\
                                 name: svc-workload-cur-{i}\n\
                                 port:\n\
                                   number: {svc_port}\n",
                )
            }
            _ => unreachable!(),
        };

        let row = json!({"_id": id, "title": "", "text": text});
        writeln!(corpus, "{}", serde_json::to_string(&row).unwrap()).unwrap();
    }
    corpus.flush().unwrap();

    let mut queries = Map::new();
    let mut qrels = Map::new();

    for q in 0..72 {
        let doc_ix = (q * 17 + 23) % 48;
        let qid = format!("y-k8s-{q:03}");
        let kind = kinds[doc_ix % kinds.len()];
        let ns = format!("team-{}", (doc_ix % 6) + 1);

        let query_text = match kind {
            "Deployment" => format!(
                "Deployment workload-cur-{doc_ix} namespace {ns} harbor.curatedthoughts.invalid/libs/bench:{doc_ix}"
            ),
            "Service" => format!(
                "Service svc-workload-cur-{doc_ix} namespace {ns} http-cur-{doc_ix} port {}",
                3000 + (doc_ix % 5000),
            ),
            "ConfigMap" => format!(
                "ConfigMap cfg-workload-cur-{doc_ix} queue=cur-{doc_ix} namespace {ns}"
            ),
            "Secret" => format!(
                "Secret sec-workload-cur-{doc_ix} SESSION_KEY CUR-BENCH-{doc_ix} namespace {ns}"
            ),
            "Ingress" => format!(
                "Ingress cur-bench-{doc_ix}.example.invalid path /api/v{doc_ix}/stream namespace {ns}"
            ),
            _ => format!("{kind} workload-cur-{doc_ix} namespace {ns}"),
        };

        queries.insert(qid.clone(), json!(query_text));
        qrels.insert(qid, json!([format!("k8s-cur-{doc_ix:03}")]));
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

    println!("Wrote YAML K8s curated bench under {}", dir.display());
}
