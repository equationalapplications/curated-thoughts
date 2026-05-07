//! Curated-style **code** bench: alternate TypeScript React chunks and small Rust modules.
//! Writes `tests/fixtures/code-bench-curated/`.

use serde_json::{json, Map};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("tests/fixtures/code-bench-curated");
    fs::create_dir_all(&dir).expect("mkdir");

    let mut corpus = BufWriter::new(File::create(dir.join("corpus.jsonl")).expect("corpus"));

    for i in 0..52 {
        let id = format!("code-cur-{i:03}");
        let text = if i % 2 == 0 {
            let stream_tag = format!(
                "{}{}",
                (b'a' + (i % 26) as u8) as char,
                i % 997
            );
            format!(
                "/** CuratedThoughts bench — ledger surface */\n\
                 export async function LedgerWindow{i}(ctx: LedgerCtx{i}) {{\n\
                   await ctx.redis.xadd(\"ledger:{stream_tag}\", '*', 'seq', '{i}');\n\
                   return <article className=\"ledger-{i}\">{{ctx.tenant}}</article>;\n\
                 }}\n\
                 interface LedgerCtx{i} {{ redis: RedisLike; tenant: string }}\n\
                 type RedisLike = {{ xadd: (s: string, ...a: unknown[]) => Promise<number> }};\n",
            )
        } else {
            format!(
                "//! ingestion worker {i}\n\
                 pub struct IngestPump{i} {{\n\
                     shard: usize,\n\
                     label: &'static str,\n\
                 }}\n\
                 impl IngestPump{i} {{\n\
                     pub const TOKEN: &'static str = \"INGEST-CUR-{i}\";\n\
                     pub fn new() -> Self {{\n\
                         Self {{ shard: {shard}, label: \"pump-{i}\" }}\n\
                     }}\n\
                     pub fn ingest(&self, bytes: &[u8]) -> usize {{\n\
                         bytes.len().wrapping_add(self.shard)\n\
                     }}\n\
                 }}\n",
                shard = i * 97 % 65536,
                i = i,
            )
        };

        let row = json!({"_id": id, "title": "", "text": text});
        writeln!(corpus, "{}", serde_json::to_string(&row).unwrap()).unwrap();
    }
    corpus.flush().unwrap();

    let mut queries = Map::new();
    let mut qrels = Map::new();

    for q in 0..72 {
        let doc_ix = (q * 19 + 29) % 52;
        let qid = format!("c-cur-{q:03}");

        let query_text = if doc_ix % 2 == 0 {
            let stream_tag = format!(
                "{}{}",
                (b'a' + (doc_ix % 26) as u8) as char,
                doc_ix % 997
            );
            format!(
                "TypeScript LedgerWindow{doc_ix} redis xadd ledger:{stream_tag} article ledger-{doc_ix}",
            )
        } else {
            format!(
                "Rust IngestPump{doc_ix} TOKEN INGEST-CUR-{doc_ix} pump shard",
                doc_ix = doc_ix,
            )
        };

        queries.insert(qid.clone(), json!(query_text));
        qrels.insert(qid, json!([format!("code-cur-{doc_ix:03}")]));
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

    println!("Wrote code curated bench under {}", dir.display());
}
