#![cfg(feature = "slow-tests")]

mod helpers;

use helpers::recall_bench::run_recall_at_k;
use tauri_app_lib::recall_bench_fixture::YAML_SYNTHETIC_EMBEDDINGS_GZIP;

#[test]
fn yaml_synthetic_benchmark_recall_at_10() {
    run_recall_at_k(
        "yaml-synthetic",
        "yaml-bench-synthetic",
        YAML_SYNTHETIC_EMBEDDINGS_GZIP,
        10,
        0.90,
    );
}
