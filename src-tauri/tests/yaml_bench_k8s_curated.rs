#![cfg(feature = "slow-tests")]

mod helpers;

use helpers::recall_bench::run_recall_at_k;
use tauri_app_lib::recall_bench_fixture::YAML_K8S_CURATED_EMBEDDINGS_GZIP;

#[test]
fn yaml_k8s_curated_benchmark_recall_at_10() {
    run_recall_at_k(
        "yaml-k8s-curated",
        "yaml-bench-k8s-curated",
        YAML_K8S_CURATED_EMBEDDINGS_GZIP,
        10,
        0.90,
    );
}
