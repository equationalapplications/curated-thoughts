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
        /// Confirm the bulk write (`--all` with pending items refuses without it).
        #[arg(long)]
        yes: bool,
        proposal_id: Option<String>,
    },
    /// Ingest the vault into the brain database (write; requires --yes).
    Ingest {
        /// Confirm the write.
        #[arg(long)]
        yes: bool,
    },
    /// Librarian operations.
    Librarian {
        #[command(subcommand)]
        cmd: LibrarianCmd,
    },
    /// Run the headless vault watcher (foreground daemon).
    Watch {
        /// Run in bounded watchdog mode (exit after --once-timeout; default
        /// 60s). The runtime exits on timeout alone — there is no idle
        /// early-exit; without events the watcher idles for the full
        /// timeout window. CodeRabbit review on PR #96.
        #[arg(long)]
        once: bool,
        /// Emit structured JSON event lines to stdout (one per event). Use
        /// 2>/dev/null or `--stderr` redirection only for human-readable
        /// mode. Schema: {"kind": "<start|added|modified|removed|error|shutdown>",
        /// "path": "<absolute>", "ts_ms": <i64 unix millis>}.
        #[arg(long)]
        json: bool,
        /// Maximum time to wait in --once mode (default 60s). Format: e.g. "60s", "5m", "500ms".
        #[arg(long, value_parser = parse_secs)]
        once_timeout: Option<std::time::Duration>,
        /// Run as a foreground daemon (the only mode in v1; flag exists for spec parity + future systemd use).
        // TODO(phase3): `foreground` is a no-op in v1 — daemon is always foreground.
        // Future: --background spawns a detached systemd-style service.
        #[arg(long)]
        foreground: bool,
    },
}

/// Parse a human-friendly duration string ("60s", "5m", "500ms", "2h") into a
/// `std::time::Duration`. Used by the `watch --once-timeout` flag so we don't
/// pull in the `humantime` crate just for one flag.
fn parse_secs(s: &str) -> Result<std::time::Duration, String> {
    let s = s.trim();
    if let Some(num) = s.strip_suffix("ms") {
        num.parse::<u64>()
            .map(std::time::Duration::from_millis)
            .map_err(|e| format!("invalid ms: {e}"))
    } else if let Some(num) = s.strip_suffix('s') {
        num.parse::<u64>()
            .map(std::time::Duration::from_secs)
            .map_err(|e| format!("invalid s: {e}"))
    } else if let Some(num) = s.strip_suffix('m') {
        num.parse::<u64>()
            .map(|n| std::time::Duration::from_secs(n * 60))
            .map_err(|e| format!("invalid m: {e}"))
    } else if let Some(num) = s.strip_suffix('h') {
        num.parse::<u64>()
            .map(|n| std::time::Duration::from_secs(n * 3600))
            .map_err(|e| format!("invalid h: {e}"))
    } else {
        Err(format!(
            "unrecognized duration format: {s:?} (use 60s, 5m, 500ms, 2h)"
        ))
    }
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
    /// Run the Active Librarian over indexed documents (write; requires --yes).
    Run {
        /// Confirm the write.
        #[arg(long)]
        yes: bool,
        /// Re-run every document, bypassing the synthesis watermark gate.
        #[arg(long)]
        force: bool,
    },
}

fn main() {
    let cmd = match Ct::try_parse() {
        Ok(ct) => ct.cmd,
        Err(e) => {
            // --help is a successful invocation: print help, exit 0.
            if e.kind() == clap::error::ErrorKind::DisplayHelp {
                let _ = e.print();
                std::process::exit(0);
            }
            // Parse errors go through here; clap exits 2 on usage errors by
            // default, but our contract wants 1.
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
            ProposalsCmd::Show { id, json } => proposals_show(&id, json),
        },
        Cmd::Approve {
            all,
            yes,
            proposal_id,
        } => approve_cmd(all, yes, proposal_id),
        Cmd::Ingest { yes } => {
            if !yes {
                // Path-only resolution so a fresh brain (no brain.db yet)
                // can still print the refusal with the planned db path.
                let db_path = tauri_app_lib::retrieval::resolve_brain_paths().db_path;
                eprintln!(
                    "refusing: `ct ingest` would ingest the configured vault into {} (a write). Pass --yes to proceed.",
                    db_path.display()
                );
                return Ok(1);
            }
            cli_common::ingest_run()?;
            Ok(0)
        }
        Cmd::Librarian { cmd } => match cmd {
            LibrarianCmd::Run { yes, force } => librarian_run_cmd(yes, force),
        },
        Cmd::Watch {
            once,
            json,
            once_timeout,
            foreground: _,
        } => {
            use curated_thoughts_tools::cli_common::WatchOpts;
            let opts = WatchOpts {
                once,
                json_mode: json,
                background: false,
                once_timeout,
            };
            // For `--json` mode, the spec §6 wire format covers
            // `{kind, path, ts_ms}` events. The shutdown event is emitted
            // from inside `watch_run` (line ~780) on EVERY outcome —
            // clean, classified, and unclassified. Here we just emit
            // an `error` line so consumers see the reason first; the
            // shutdown follows immediately. CodeRabbit review on PR #96
            // (pass 3): the previous comment promised a "paired" event
            // but the shutdown never fired for classified exits.
            match cli_common::watch_run(opts) {
                Ok(0) => Ok(0),
                Ok(code) => {
                    if json {
                        // Classified exit (lock conflict → 2,
                        // DB → 3, notify-init → 4). The shutdown event
                        // for this run has already been emitted by
                        // `watch_run`'s wrapper (line ~780), so we
                        // only need the error line here. Consumers see
                        // error → shutdown in stdout.
                        println!(
                            "{}",
                            cli_common::format_event(
                                "error",
                                &format!("classified exit code {code}"),
                                cli_common::now_ms()
                            )
                        );
                    }
                    Ok(code)
                }
                Err(e) => {
                    if json {
                        // Emit a structured error line so log
                        // scrapers see the failure reason.
                        println!(
                            "{}",
                            cli_common::format_event(
                                "error",
                                &format!("{e}"),
                                cli_common::now_ms()
                            )
                        );
                    }
                    Err(e)
                }
            }
        }
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

/// `ct proposals show <id>` — full proposal detail. `--json` prints the
/// ProposalDetail JSON verbatim; default renders a compact text summary.
/// Unknown id exits 2 per the no-results contract.
fn proposals_show(id: &str, json_mode: bool) -> Result<i32> {
    let brain = cli_common::resolve()?;
    let conn = cli_common::open_ro(&brain)?;
    match cli_common::show_proposal(&conn, id)? {
        None => Ok(cli_common::EXIT_NO_RESULTS),
        Some(detail) => {
            if json_mode {
                print_json(&detail);
            } else {
                println!("{}\t{}", detail.id, detail.created_at);
                for p in &detail.source_doc_paths {
                    println!("source: {p}");
                }
                println!("{} item(s)", detail.items.len());
                for item in &detail.items {
                    println!(
                        "  {}\t{}",
                        item.id,
                        serde_json::to_string(&item.payload).unwrap_or_else(|_| "<payload>".into())
                    );
                }
            }
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

/// `ct approve` — write command with the SDD confirmation rules:
/// - `<id>`: approve that proposal (exit 0), or exit 1 if not pending/unknown.
/// - `--all`: empty pending set exits 0 printing `approved: 0`; with pending
///   items, refuses (exit 1, listing what would be accepted) unless `--yes`.
fn approve_cmd(all: bool, yes: bool, proposal_id: Option<String>) -> Result<i32> {
    if all {
        let brain = cli_common::resolve()?;
        let conn = cli_common::open_ro(&brain)?;
        let pending = cli_common::list_pending_proposals(&conn)?;
        if pending.is_empty() {
            println!("approved: 0");
            return Ok(0);
        }
        if !yes {
            eprintln!(
                "refusing: --all would approve {} pending proposal(s); pass --yes to proceed:",
                pending.len()
            );
            for p in &pending {
                eprintln!(
                    "  {}\t{} items\t{}",
                    p.id,
                    p.item_count,
                    p.source_doc_path.as_deref().unwrap_or("-")
                );
            }
            return Ok(1);
        }
        drop(conn);
        cli_common::approve_all()?;
        return Ok(0);
    }
    match proposal_id {
        Some(id) => {
            cli_common::approve_one(&id)?;
            Ok(0)
        }
        None => bail!("specify a proposal id or --all"),
    }
}

/// `ct librarian run` — requires --yes; prints the planned action otherwise.
fn librarian_run_cmd(yes: bool, force: bool) -> Result<i32> {
    if !yes {
        let brain = cli_common::resolve()?;
        let conn = cli_common::open_ro(&brain)?;
        let docs: i64 = conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))?;
        eprintln!(
            "refusing: `ct librarian run` would run the Active Librarian over {docs} indexed document(s) in {} (a write). Pass --yes to proceed.",
            brain.paths.db_path.display()
        );
        return Ok(1);
    }
    cli_common::librarian_run("llama3.2:3b", force)?;
    Ok(0)
}
