#![cfg(feature = "slow-tests")]

mod helpers;

use helpers::recall_bench::run_recall_at_k;
use tauri_app_lib::recall_bench_fixture::CODE_SYNTHETIC_EMBEDDINGS_GZIP;

#[test]
fn code_synthetic_benchmark_recall_at_10() {
    run_recall_at_k(
        "code-synthetic",
        "code-bench-synthetic",
        CODE_SYNTHETIC_EMBEDDINGS_GZIP,
        10,
        0.90,
    );
}
