use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use curated_thoughts_tools::cli_common::{self, print_json};
use serde_json::json;

/// `ct` — headless CLI for Curated Thoughts brains.
#[derive(Parser)]
struct Ct {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Vault + database summary.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Semantic search over indexed chunks.
    Search {
        query: String,
        #[arg(long, default_value_t = 5)]
        k: usize,
        #[arg(long)]
        json: bool,
    },
    /// Recall context for a prompt (chunks + wiki entries).
    Recall {
        query: String,
        #[arg(long, default_value_t = 5)]
        k: usize,
        #[arg(long)]
        json: bool,
    },
    /// Search code chunks (ast strategies only).
    Code {
        query: String,
        #[arg(long, default_value_t = 5)]
        k: usize,
        #[arg(long)]
        json: bool,
    },
    /// Knowledge-graph lookups around a symbol.
    Graph {
        symbol: String,
        #[arg(long, value_enum, default_value = "both")]
        dir: cli_common::GraphDir,
        #[arg(long, default_value_t = 1)]
        hops: u32,
        #[arg(long)]
        json: bool,
    },
    /// Wiki entry operations.
    Wiki {
        #[command(subcommand)]
        cmd: WikiCmd,
    },
    /// Curated proposal operations (read-only).
    Proposals {
        #[command(subcommand)]
        cmd: ProposalsCmd,
    },
    /// Approve pending proposals (write).
    Approve {
        #[arg(long)]
        all: bool,
        proposal_id: Option<String>,
    },
    /// Ingest the vault into the brain database (write).
    Ingest,
    /// Librarian operations.
    Librarian {
        #[command(subcommand)]
        cmd: LibrarianCmd,
    },
}

#[derive(Subcommand)]
enum WikiCmd {
    List {
        #[arg(long)]
        json: bool,
    },
    /// Print full wiki row(s) for an entity id (body included).
    Get {
        entity_id: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ProposalsCmd {
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum LibrarianCmd {
    Run,
}

fn main() {
    let cmd = match Ct::try_parse() {
        Ok(ct) => ct.cmd,
        Err(e) => {
            // Parse errors and --help both go through here; clap exits 2 on
            // usage errors by default, but our contract wants 1.
            let _ = e.print();
            std::process::exit(1);
        }
    };
    let code = match run(cmd) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

/// One dispatch path; returns the process exit code
/// (0 ok, 1 error, 2 no results).
fn run(cmd: Cmd) -> Result<i32> {
    match cmd {
        Cmd::Status { json } => status(json),
        Cmd::Search { query, k, json } => cli_common::search_cmd(&query, k, json),
        Cmd::Recall { query, k, json } => cli_common::recall_cmd(&query, k, json),
        Cmd::Code { query, k, json } => cli_common::code_cmd(&query, k, json),
        Cmd::Graph {
            symbol,
            dir,
            hops,
            json,
        } => cli_common::graph_cmd(&symbol, dir, hops, json),
        Cmd::Wiki { cmd } => match cmd {
            WikiCmd::List { json } => cli_common::wiki_list_cmd(json),
            WikiCmd::Get { entity_id, json } => cli_common::wiki_get_cmd(&entity_id, json),
        },
        Cmd::Proposals { cmd } => match cmd {
            ProposalsCmd::List { json } => proposals_list(json),
            ProposalsCmd::Show { id, .. } => proposals_show(&id),
        },
        other => {
            bail!("subcommand not implemented yet: {}", describe(&other));
        }
    }
}

fn describe(cmd: &Cmd) -> &'static str {
    match cmd {
        Cmd::Status { .. } => "status",
        Cmd::Search { .. } => "search",
        Cmd::Recall { .. } => "recall",
        Cmd::Code { .. } => "code",
        Cmd::Graph { .. } => "graph",
        Cmd::Wiki { .. } => "wiki",
        Cmd::Proposals { .. } => "proposals",
        Cmd::Approve { .. } => "approve",
        Cmd::Ingest => "ingest",
        Cmd::Librarian { .. } => "librarian",
    }
}

/// `ct proposals list` — pending proposals, oldest first. Empty list is the
/// no-results case (exit 2), matching search/recall.
fn proposals_list(json_mode: bool) -> Result<i32> {
    let brain = cli_common::resolve()?;
    let conn = cli_common::open_ro(&brain)?;
    let proposals = cli_common::list_pending_proposals(&conn)?;
    if proposals.is_empty() {
        return Ok(cli_common::EXIT_NO_RESULTS);
    }
    if json_mode {
        print_json(&proposals);
    } else {
        for p in &proposals {
            println!(
                "{}\t{}\t{} items\t{}",
                p.id,
                p.created_at,
                p.item_count,
                p.source_doc_path.as_deref().unwrap_or("-")
            );
        }
    }
    Ok(0)
}

/// `ct proposals show <id>` — full ProposalDetail JSON verbatim.
/// Unknown id exits 2 per the no-results contract.
fn proposals_show(id: &str) -> Result<i32> {
    let brain = cli_common::resolve()?;
    let conn = cli_common::open_ro(&brain)?;
    match cli_common::show_proposal(&conn, id)? {
        None => Ok(cli_common::EXIT_NO_RESULTS),
        Some(detail) => {
            print_json(&detail);
            Ok(0)
        }
    }
}

fn status(json_mode: bool) -> Result<i32> {
    let brain = cli_common::resolve()?;
    let conn = cli_common::open_ro(&brain)?;
    let count = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };
    let docs = count("SELECT COUNT(*) FROM documents")?;
    let chunks = count("SELECT COUNT(*) FROM chunks")?;
    let wiki_entries = count("SELECT COUNT(*) FROM llm_wiki_entries WHERE deleted_at IS NULL")?;
    let proposals_pending = count("SELECT COUNT(*) FROM curated_proposals WHERE status='pending'")?;
    let schema_version: Option<i64> = conn
        .query_row(
            "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok();
    let last_ingest_run: Option<(i64, i64, String)> = conn
        .query_row(
            "SELECT id, doc_id, outcome FROM ingest_runs \
             WHERE id = (SELECT MAX(id) FROM ingest_runs)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();

    if json_mode {
        print_json(&json!({
            "docs": docs,
            "chunks": chunks,
            "wiki_entries": wiki_entries,
            "proposals_pending": proposals_pending,
            "db_path": brain.paths.db_path.display().to_string(),
            "schema_version": schema_version,
            "last_ingest_run": last_ingest_run
                .map(|(id, doc_id, outcome)| json!({
                    "id": id, "doc_id": doc_id, "outcome": outcome
                })),
        }));
    } else {
        println!("{:<20}{}", "docs", docs);
        println!("{:<20}{}", "chunks", chunks);
        println!("{:<20}{}", "wiki_entries", wiki_entries);
        println!("{:<20}{}", "proposals_pending", proposals_pending);
        println!(
            "{:<20}{}",
            "schema_version",
            schema_version
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into())
        );
        println!("{:<20}{}", "db_path", brain.paths.db_path.display());
        println!(
            "{:<20}{}",
            "last_ingest_run",
            last_ingest_run
                .map(|(id, _, o)| format!("#{id} ({o})"))
                .unwrap_or_else(|| "never".into())
        );
    }
    Ok(0)
}
