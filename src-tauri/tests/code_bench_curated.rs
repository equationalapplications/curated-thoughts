#![cfg(feature = "slow-tests")]

mod helpers;

use helpers::recall_bench::run_recall_at_k;
use tauri_app_lib::recall_bench_fixture::CODE_CURATED_EMBEDDINGS_GZIP;

#[test]
fn code_curated_benchmark_recall_at_10() {
    run_recall_at_k(
        "code-curated",
        "code-bench-curated",
        CODE_CURATED_EMBEDDINGS_GZIP,
        10,
        0.90,
    );
}
