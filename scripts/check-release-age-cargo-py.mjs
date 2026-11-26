#!/usr/bin/env node
// Supply-chain policy (docs/superpowers/specs/2026-08-27-supply-chain-version-policy.md):
// every registry dependency version must have been published at least MIN_DAYS
// days ago (default 14), so a poisoned upstream release has time to be caught
// and unpublished before it is installed. FAIL-CLOSED: a version whose registry
// timestamp cannot be determined is a failure, not a skip.
//
// Covers Rust (Cargo.lock) and Python (requirements*.txt, uv.lock,
// poetry.lock) lockfiles. curated-thoughts currently has NO Python lockfile;
// the Python parsers exist so the same gate covers one if one ever lands.
//
// Modes:
//   --base <git-ref>  PR check: age-check only the lockfile DELTA vs <ref>
//                     (entries that are new, changed version, OR changed
//                     source — a same-version source swap is a delta change).
//                     A PR whose delta is empty passes with no cooldown. A
//                     new or source-changed entry that is neither a crates.io
//                     registry entry nor a plain path/workspace entry fails
//                     the gate (git deps have no registry timestamp; foreign
//                     registries are unvetted). Pre-existing path/local
//                     workspace entries stay skipped — they are this repo's
//                     own code, not a supply-chain input.
//   (no --base)       Whole-lockfile audit of every registry entry — the
//                     manual `workflow_dispatch` sweep.
//
// USAGE: node scripts/check-release-age-cargo-py.mjs [--base origin/main] [--days 14]
//        [--lockfile Cargo.lock] [--first-party-prefix <prefix>]...
//
// First-party exemption: entries whose package name starts with a
// --first-party-prefix are exempt. EA publishes no crates to crates.io and no
// packages to PyPI today, so curated-thoughts CI passes no prefix — the
// exemption exists only so a future first-party publish can be wired in
// without weakening the default.
//
// Uses only Node built-ins so it can run before any `cargo`/`pip` install.
// crates.io requires a User-Agent header; requests are globally spaced and
// Retry-After-aware to respect registry rate limits.
//
// Known parser limitation (fails toward MORE checking, never less):
// - poetry.lock per-package git/URL sources live in a nested [package.source]
//   table this parser does not read; any poetry.lock containing such tables
//   is rejected outright (fail-closed) rather than misread as pypi.

import fs from "node:fs"
import { execFileSync } from "node:child_process"
import path from "node:path"
import { pathToFileURL } from "node:url"

// ---------------------------------------------------------------------------
// Lockfile parsing -> Map(name -> Set(version)), plus skipped entries.
// Each entry: { name, version, registry: "crates"|"pypi"|null, git?, reason? }
// registry === null means not registry-checkable (path/git dep, etc.); `git`
// marks git-source entries, which are a fail-closed violation when newly
// added in delta mode.
// ---------------------------------------------------------------------------

// Minimal hand-rolled parse of the Cargo.lock / poetry.lock TOML subset:
// [[package]] array-of-table sections with string-valued keys. Handles quoted
// strings and inline arrays like `dependencies = ["a", "b"]` (ignored anyway).
// Comment stripping is quote-aware so values like git URLs carrying a
// `#revision` fragment survive intact.
export function stripTomlComment(line) {
  let inStr = false
  for (let i = 0; i < line.length; i++) {
    const ch = line[i]
    if (ch === '"' && line[i - 1] !== "\\") inStr = !inStr
    else if (ch === "#" && !inStr) return line.slice(0, i)
  }
  return line
}

export function parseTomlPackageSections(raw) {
  const sections = []
  let current = null
  for (const lineRaw of raw.split(/\r?\n/)) {
    const line = stripTomlComment(lineRaw).trim()
    if (!line) continue
    const section = line.match(/^\[\[([^\]]+)\]\]$/)
    if (section) {
      current = { section: section[1], fields: {} }
      sections.push(current)
      continue
    }
    const table = line.match(/^\[([^\]]+)\]$/)
    if (table) {
      current = { section: null, fields: {} } // non-package table; skip fields
      continue
    }
    if (!current) continue
    const kv = line.match(/^([A-Za-z0-9_.-]+)\s*=\s*(.+)$/)
    if (!kv) continue
    const value = kv[2].trim()
    const str = value.match(/^"(.*)"$/)
    current.fields[kv[1]] = str ? str[1] : value
  }
  return sections
}

export function collectEntries(entries, skipped, prefixes = []) {
  const wanted = new Map() // registry -> Map(name -> Set(version))
  const isFirstParty = (name) => prefixes.some((p) => name.startsWith(p))
  for (const e of entries) {
    if (isFirstParty(e.name)) {
      skipped.push({ name: e.name, version: e.version, reason: "first-party exemption" })
      continue
    }
    if (!e.registry) {
      skipped.push({ name: e.name, version: e.version, reason: e.reason })
      continue
    }
    if (!wanted.has(e.registry)) wanted.set(e.registry, new Map())
    const byName = wanted.get(e.registry)
    if (!byName.has(e.name)) byName.set(e.name, new Set())
    byName.get(e.name).add(e.version)
  }
  return wanted
}

export function entriesFromCargoLock(raw) {
  const entries = []
  for (const s of parseTomlPackageSections(raw)) {
    if (s.section !== "package") continue
    const name = s.fields.name
    const version = s.fields.version
    if (!name || !version) continue
    const source = s.fields.source ?? ""
    if (source.startsWith("registry+https://github.com/rust-lang/crates.io-index")) {
      entries.push({ name, version, registry: "crates", src: "crates.io" })
    } else {
      entries.push({
        name,
        version,
        registry: null,
        git: source.startsWith("git+"),
        src: source,
        reason: source ? `non-registry source ${source}` : "no source (path/git/local)",
      })
    }
  }
  return entries
}

export function entriesFromUvLock(raw) {
  // uv.lock: [[package]] name/version, source = { registry = "..." } (inline
  // table) for registry packages; otherwise path/git/virtual.
  const entries = []
  for (const s of parseTomlPackageSections(raw)) {
    if (s.section !== "package") continue
    const name = s.fields.name
    const version = s.fields.version
    if (!name || !version) continue
    const source = s.fields.source ?? ""
    const m = source.match(/registry\s*=\s*"([^"]+)"/)
    if (m && m[1].startsWith("https://pypi.org/simple")) {
      entries.push({ name, version, registry: "pypi", src: "pypi" })
    } else if (/\b(virtual|editable|path)\s*=\s*"/.test(source)) {
      // uv workspace members and local paths: the project's own code, not a
      // supply-chain input — treated like Cargo path entries (skipped).
      entries.push({
        name,
        version,
        registry: null,
        git: false,
        src: "",
        reason: `local source ${source}`,
      })
    } else {
      entries.push({
        name,
        version,
        registry: null,
        git: /git\s*=\s*"/.test(source),
        src: source,
        reason: source ? `non-registry source ${source}` : "no registry source",
      })
    }
  }
  return entries
}

export function entriesFromPoetryLock(raw, problems = []) {
  // poetry.lock [[package]] with source section or name = "pkg (==1.2.3)".
  // FAIL-CLOSED on [package.source] tables: poetry stores git/URL sources in
  // that NESTED table, which this minimal parser does not read — a git fork
  // pinned at a version that exists on PyPI would otherwise pass the PyPI
  // lookup. Any lockfile using [package.source] is rejected outright.
  const sourceTables = new Map()
  {
    // One problem PER TABLE. The identity is package name@version plus the
    // table's own contents (type/url/reference) — NOT a mutable count — so
    // removing a table passes, adding one fails, and a CHANGED url/revision
    // is a new identity and fails.
    let pkg = null
    let inPkg = false
    let src = null
    const flush = () => {
      if (src) {
        if (pkg) sourceTables.set(`${pkg.name}@${pkg.version}`, [...src].sort().join("; "))
        problems.push(
          `poetry.lock package ${pkg?.name ?? "?"}@${pkg?.version ?? "?"} has a [package.source] table (git/URL/direct source) which this parser cannot verify (fail-closed): ${src.sort().join("; ")}`,
        )
      }
      src = null
    }
    for (const lineRaw of raw.split(/\r?\n/)) {
      const line = stripTomlComment(lineRaw).trim()
      if (!line) continue
      if (/^\[[^\]]+\]\]?$/.test(line)) {
        flush()
        if (line === "[[package]]") {
          pkg = { name: "", version: "" }
          inPkg = true
        } else if (line === "[package.source]") {
          src = []
          inPkg = false
        } else {
          inPkg = false
        }
        continue
      }
      if (src) src.push(line)
      else if (inPkg && pkg) {
        const kv = line.match(/^(name|version)\s*=\s*"(.*)"$/)
        if (kv) pkg[kv[1]] = kv[2]
      }
    }
    flush()
  }
  const entries = []
  for (const s of parseTomlPackageSections(raw)) {
    if (s.section !== "package") continue
    const name = (s.fields.name ?? "").split(" ")[0]
    const version = (s.fields.version ?? "").replace(/^[=<>!~^ ]+/, "")
    if (!name || !version) continue
    // A package with a nested [package.source] table is NOT a plain PyPI
    // entry: its entry identity carries the table contents, so removing the
    // table at the same version changes the delta key and the now-PyPI
    // version is age-checked. (The per-table problem identity is unchanged.)
    const table = sourceTables.get(`${name}@${version}`)
    if (table !== undefined) {
      entries.push({
        name,
        version,
        registry: null,
        git: /type\s*=\s*"git"/.test(table),
        src: `table:${table}`,
        reason: `nested [package.source] table: ${table}`,
      })
    } else if ((s.fields.source ?? "pypi") === "pypi") {
      entries.push({ name, version, registry: "pypi", src: "pypi" })
    } else {
      entries.push({
        name,
        version,
        registry: null,
        git: (s.fields.source ?? "").startsWith("git"),
        src: s.fields.source ?? "",
        reason: `source ${s.fields.source}`,
      })
    }
  }
  return entries
}

// requirements*.txt: pinned lines `name==version`. PEP 508 extras are stripped
// ("flask[async]" -> "flask", the PyPI project name) and names are lowercased.
// Unpinned or otherwise unparseable non-option lines are FAIL-CLOSED problems:
// they cannot be age-checked, so they fail the gate instead of passing it.
// Backslash line continuations are joined before parsing.
//
// pip's requirement-file long options (pip 25.1.1 SUPPORTED_OPTIONS +
// SUPPORTED_OPTIONS_REQ). optparse accepts an unambiguous prefix of any of
// these (`--index-u` == `--index-url`), so tokens are canonicalized before
// classification. This set is a superset of the fail-closed options below,
// which keeps the mapping exact: a token pip accepts resolves to the same
// option here, and a token ambiguous in pip stays unmapped (pip would reject
// the line anyway). Keep in sync with the installed pip's option universe.
const PIP_REQ_FILE_LONG_OPTS = new Set([
  "config-settings", "constraint", "editable", "extra-index-url", "find-links",
  "global-option", "hash", "index-url", "no-binary", "no-index", "only-binary",
  "pre", "prefer-binary", "pypi-url", "require-hashes", "requirement",
  "trusted-host", "use-feature",
])

export function canonicalLongOption(tok) {
  const m = tok.match(/^(--[^\s=]+)(=.*)?$/)
  if (!m) return tok
  const name = m[1]
  if (PIP_REQ_FILE_LONG_OPTS.has(name)) return tok
  const bare = name.slice(2)
  const cands = [...PIP_REQ_FILE_LONG_OPTS].filter((o) => o.startsWith(bare))
  return cands.length === 1 ? `--${cands[0]}${m[2] ?? ""}` : tok
}

const UNRESOLVED_INCLUDE_PREFIX = "unresolved include directive (fail-closed; pins inside are not age-checked): "

function unresolvedIncludeProblem(lineRaw) {
  return `${UNRESOLVED_INCLUDE_PREFIX}${lineRaw.trim()}`
}

export function entriesFromRequirements(raw, problems = [], opts = {}) {
  const entries = []
  // Join continuations exactly like pip's join_lines: fragments are
  // concatenated with NO inserted separator (pip builds the joined line with
  // "".join), so a backslash-split option name (`--index-\` + `url ...`)
  // rejoins and is still scanned; the whitespace before the backslash is
  // preserved verbatim, which keeps ordinary continuations token-identical.
  // A comment line is never continued (pip prepends the space itself) and
  // never swallows the following line.
  const lines = []
  let buf = ""
  for (const l of raw.split(/\r?\n/)) {
    const isComment = l.trimStart().startsWith("#")
    if (isComment) {
      lines.push(buf ? `${buf} ${l}` : l)
      buf = ""
    } else if (l.endsWith("\\")) {
      buf += l.slice(0, -1)
    } else {
      lines.push(buf ? buf + l : l)
      buf = ""
    }
  }
  if (buf) lines.push(buf)
  for (const lineRaw of lines) {
    const line = stripTomlComment(lineRaw).trim()
    if (!line) continue
    if (line.startsWith("-")) {
      // FAIL-CLOSED on supply-chain-relevant option lines: a foreign index
      // (-i/--index-url/--extra-index-url), an editable/URL/git install
      // (-e/--editable, git+), a find-links source, --trusted-host (bypasses
      // TLS verification), or --no-index (local/unverifiable sources only)
      // cannot be age-checked from a version pin. Other option lines are
      // skipped. Tokens are canonicalized first, because pip's optparse
      // accepts unambiguous long-option abbreviations (--index-u is
      // --index-url). Short options also match attached arguments
      // (-ihttps://..., pip's optparse treats -iX as -i X).
      // The line is tokenized on whitespace and EVERY token is tested, so a
      // benign leading option (`--pre --index-url ...`) cannot mask a later
      // one. Matched options are recorded by canonical name in a
      // machine-readable slot: `fail-closed option line (matched: -i,--index-url)`.
      const matched = new Set()
      let includeArg // undefined: no directive; null: arg missing; string: arg
      const toks = line.split(/\s+/)
      for (let i = 0; i < toks.length; i++) {
        const tok = canonicalLongOption(toks[i])
        if (/^(--requirement|--constraint)(=|$)/.test(tok) || /^-[rc]/.test(tok)) {
          if (includeArg === undefined) {
            const eq = tok.indexOf("=")
            if (eq >= 0) includeArg = tok.slice(eq + 1)
            else if (/^(--requirement|--constraint|-r|-c)$/.test(tok)) includeArg = toks[i + 1] ?? null
            else includeArg = tok.slice(2) // attached: -rpath / -cpath
          }
        }
        else if (/^(--index-url|--extra-index-url|--find-links|--trusted-host)(=|$)/.test(tok)) matched.add(tok.split("=")[0])
        else if (tok === "--no-index") matched.add("--no-index")
        else if (/^--editable(=|$)/.test(tok)) matched.add("--editable")
        else if (/^-[ife]/.test(tok)) matched.add(tok.slice(0, 2))
      }
      if (/git\+/.test(line)) matched.add("git+")
      if (includeArg !== undefined) {
        // Include/constraint directives are normally fail-closed: the pins
        // inside are not read here. The CLI's include-graph walk passes
        // `opts.resolveInclude`, which takes over entirely: it either resolves
        // the target (and audits its pins itself) or pushes the unresolved
        // problem itself — so the parser never double-pushes when a resolver
        // is present. Without a resolver, every directive fails closed.
        const handled = opts.resolveInclude ? opts.resolveInclude(includeArg, lineRaw.trim()) : false
        if (!handled) problems.push(unresolvedIncludeProblem(lineRaw))
      }
      if (matched.size > 0) {
        problems.push(
          `fail-closed option line (matched: ${[...matched].sort().join(",")}) (foreign index or editable/git/URL install): ${lineRaw.trim()}`,
        )
      }
      continue
    }
    const m = line.match(/^([A-Za-z0-9_.[\]-]+)==([^;\s]+)/)
    if (m) {
      entries.push({ name: m[1].toLowerCase().split("[")[0], version: m[2], registry: "pypi", src: "pypi" })
    } else {
      problems.push(`unpinned or unparseable requirement line (fail-closed): ${lineRaw.trim()}`)
    }
  }
  return entries
}

export function entriesForFile(path, raw, problems = [], opts = {}) {
  const base = path.split("/").pop()
  if (base === "Cargo.lock") return entriesFromCargoLock(raw)
  if (base === "uv.lock") return entriesFromUvLock(raw)
  if (base === "poetry.lock") return entriesFromPoetryLock(raw, problems)
  if (/^requirements.*\.txt$/.test(base)) return entriesFromRequirements(raw, problems, opts)
  throw new Error(`Unsupported lockfile format: ${path}`)
}

// Walk a requirements-file include graph: parse the root lockfile, then each
// `-r`/`-c` target (resolved relative to the INCLUDING file's directory,
// recursively and cycle-safe), so a pin change inside an included file is
// delta-tracked and age-checked exactly like a direct pin. `rawAt(absPath)`
// returns the target's content or null when it cannot be read on this side
// (absent, unreadable). Directive args that are not plain relative/absolute
// paths (URLs, env-var expressions, `~`) are unresolvable and fail closed.
//
// When a resolver callback is present the parser delegates directive handling
// entirely (see entriesFromRequirements): this walker pushes the unresolved
// problem itself for unresolvable args, and for a target ABSENT on this side
// it records the directive line instead — the delta mode mirrors those into
// the base problem multiset so an inherited-but-unreadable directive counts
// as inherited, not new (and a target present at head is simply audited).
// `absentPushes` is true for the head side (an absent target is a new,
// genuinely broken state there) and false for base.
function graphEntries(rootAbs, rootRaw, rawAt, problems, absentPushes) {
  const entries = []
  const absentDirectiveLines = []
  const visited = new Set()
  let currentAbs = ""
  const opts = {
    resolveInclude: (arg, lineRaw) => {
      const stripped = typeof arg === "string" ? arg.replace(/^['"]|['"]$/g, "") : ""
      if (!stripped || /^(~|[a-z][a-z0-9+.-]*:)/i.test(stripped) || /[$%]/.test(stripped)) {
        problems.push(unresolvedIncludeProblem(lineRaw))
        return true
      }
      // Includes resolve relative to the INCLUDING file's directory (pip's
      // rule); parsing is synchronous, so currentAbs is the file being read.
      const target = path.resolve(path.dirname(currentAbs), stripped)
      const tRaw = rawAt(target)
      if (tRaw === null) {
        absentDirectiveLines.push(lineRaw)
        if (absentPushes) problems.push(unresolvedIncludeProblem(lineRaw))
        return true
      }
      queue.push({ abs: target, raw: tRaw })
      return true
    },
  }
  const queue = [{ abs: path.resolve(rootAbs), raw: rootRaw }]
  while (queue.length > 0) {
    const { abs, raw } = queue.shift()
    if (visited.has(abs)) continue
    visited.add(abs)
    if (visited.size === 1 && !/^requirements.*\.txt$/.test(path.basename(abs))) {
      // Non-requirements root (Cargo.lock, uv.lock, poetry.lock): the format
      // dispatcher decides; these carry no include directives. An unsupported
      // format still throws (runCheck turns that into exit 2).
      entries.push(...entriesForFile(abs, raw, problems))
      continue
    }
    // The root requirements file and every include target parse as
    // requirements regardless of filename (pip allows any name after -r/-c).
    currentAbs = abs
    entries.push(...entriesFromRequirements(raw, problems, opts))
  }
  return { entries, absentDirectiveLines }
}

// Delta = entries in `currentEntries` whose name@version@source is not
// present in `baseEntries` — source is part of the key so a same-version
// source swap (e.g. a [patch] rewriting a crate from crates.io to a git or
// mirror source) is a delta change, not an invisible pass.
export function deltaEntries(currentEntries, baseEntries) {
  const key = (e) => `${e.name}@${e.version}@${e.src ?? ""}`
  const baseSet = new Set(baseEntries.map(key))
  return currentEntries.filter((e) => !baseSet.has(key(e)))
}

// ---------------------------------------------------------------------------
// Check runner. Exported (with the parsers above) so the test suite can
// exercise every path network-free via the `fetchImpl` and `spacingMs`
// injections; the CLI guard at the bottom preserves the original behavior.
// ---------------------------------------------------------------------------

export async function runCheck(
  argv = process.argv.slice(2),
  { fetchImpl = fetch, spacingMs = 1000 } = {},
) {
  const arg = (name, fallback) => {
    const i = argv.indexOf(name)
    return i >= 0 ? argv[i + 1] : fallback
  }
  const minDays = Number(arg("--days", process.env.MIN_RELEASE_AGE_DAYS || 14))
  if (!Number.isFinite(minDays) || minDays <= 0) {
    console.error(`Invalid minimum release age: ${minDays}`)
    return 2
  }
  const cutoff = new Date(Date.now() - minDays * 24 * 60 * 60 * 1000)
  const firstPartyPrefixes = []
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--first-party-prefix") {
      if (!argv[i + 1]) {
        console.error("--first-party-prefix requires a value")
        return 2
      }
      firstPartyPrefixes.push(argv[i + 1])
    }
  }
  const lockfile = arg("--lockfile", "Cargo.lock")
  const baseFlag = argv.indexOf("--base")
  const baseRef = baseFlag >= 0 ? argv[baseFlag + 1] : undefined
  if (baseFlag >= 0 && !baseRef) {
    console.error("--base requires a git ref (e.g. --base origin/main)")
    return 2
  }

  if (!fs.existsSync(lockfile)) {
    console.error(`Lockfile not found: ${lockfile}`)
    return 2
  }

  // Parse-level fail-closed problems (e.g. unpinned requirement lines), over
  // the full requirements include closure: include targets are parsed and
  // their pins age-checked exactly like direct pins.
  const problems = []
  const absLock = path.resolve(lockfile)
  const currentRaw = fs.readFileSync(lockfile, "utf8")
  let currentEntries
  try {
    currentEntries = graphEntries(absLock, currentRaw, (target) => {
      try {
        return fs.readFileSync(target, "utf8")
      } catch {
        return null
      }
    }, problems, true).entries
  } catch (err) {
    console.error(String(err.message))
    return 2
  }

  let scope
  let delta
  let newProblems = []
  let baseProblemsAll = []
  const skipped = []
  if (baseRef) {
    let baseRaw
    let repoRoot
    try {
      // Run git relative to the lockfile's directory so the check works from
      // any process cwd (CI runs at the repo root; tests use temp repos), and
      // address the blob repo-relatively — `git show ref:<path>` rejects
      // absolute paths.
      repoRoot = execFileSync("git", ["rev-parse", "--show-toplevel"], {
        cwd: path.dirname(absLock),
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
      }).trim()
      const relLock = path.relative(repoRoot, absLock)
      let baseMissing = false
      try {
        execFileSync("git", ["cat-file", "-e", `${baseRef}:${relLock}`], {
          cwd: repoRoot,
          stdio: ["ignore", "ignore", "ignore"],
        })
      } catch {
        // Brand-new lockfile (not on base): the whole file is the delta.
        baseMissing = true
      }
      baseRaw = baseMissing
        ? ""
        : execFileSync("git", ["show", `${baseRef}:${relLock}`], {
            cwd: repoRoot,
            encoding: "utf8",
            stdio: ["ignore", "pipe", "pipe"],
          })
    } catch (err) {
      console.error(`Could not read ${lockfile} from ${baseRef}: ${String(err.stderr ?? err.message).trim()}`)
      return 2
    }
    // Base-side rawAt for include-graph targets: absent on base -> null (the
    // walker records the directive instead of a problem; see below); present
    // but unreadable -> null too (head's resolved pins then count as new and
    // are age-checked — the conservative direction).
    const baseRawAt = (target) => {
      const relTarget = path.relative(repoRoot, target)
      try {
        return execFileSync("git", ["show", `${baseRef}:${relTarget}`], {
          cwd: repoRoot,
          encoding: "utf8",
          stdio: ["ignore", "pipe", "pipe"],
        })
      } catch {
        return null
      }
    }
    const baseProblems = []
    baseProblemsAll = baseProblems
    let baseGraph = null
    try {
      baseGraph = graphEntries(absLock, baseRaw, baseRawAt, baseProblems, false)
    } catch (err) {
      console.error(String(err.message))
      return 2
    }
    // An include directive whose target base cannot see (absent on base —
    // including the both-sides-absent case) is INHERITED state, not a new
    // problem: mirror head-side unresolved-include problems whose directive
    // line base recorded as absent into the base multiset. A directive whose
    // target base CAN read never produces a phantom problem, so includes
    // resolved at head are audited on their merits instead of tripping the
    // unresolved-include guard.
    const baseAbsent = new Set(baseGraph.absentDirectiveLines)
    for (const p of problems) {
      if (p.startsWith(UNRESOLVED_INCLUDE_PREFIX) && baseAbsent.has(p.slice(UNRESOLVED_INCLUDE_PREFIX.length))) {
        baseProblems.push(p)
      }
    }
    delta = deltaEntries(currentEntries, baseGraph.entries)
    // Only NEW parse problems fail a PR (count-based multiset diff, so
    // duplicating an inherited bad line still counts as new); problems
    // already present on base are inherited state (the whole-lockfile
    // audit's job). An ADDED unpinned or foreign-index line is a new problem
    // and fails even when the entry delta is empty.
    const baseCounts = new Map()
    for (const p of baseProblems) baseCounts.set(p, (baseCounts.get(p) ?? 0) + 1)
    const curCounts = new Map()
    for (const p of problems) curCounts.set(p, (curCounts.get(p) ?? 0) + 1)
    newProblems = []
    for (const [p, n] of curCounts) {
      for (let i = 0; i < n - (baseCounts.get(p) ?? 0); i++) newProblems.push(p)
    }
    const inherited = currentEntries.length - delta.length
    if (delta.length === 0) {
      if (newProblems.length > 0) {
        console.error(
          `New unage-checkable lines vs ${baseRef} (fail-closed):\n  ${newProblems.sort().join("\n  ")}`,
        )
        return 1
      }
      // Inherited-pins semantics: nothing NEW means nothing to age-check.
      console.log(`No dependency changes vs ${baseRef}; nothing to age-check.`)
      console.log(
        `${inherited} entries inherited unchanged from ${baseRef} are not re-audited in delta mode (workflow_dispatch runs the whole-lockfile audit).`,
      )
      return 0
    }
    const versionCount = delta.length
    scope = `delta vs ${baseRef} (${versionCount} changed entr${versionCount === 1 ? "y" : "ies"}; ${inherited} inherited unchanged)`
  } else {
    delta = currentEntries
    newProblems = problems
    scope = `whole lockfile (${currentEntries.length} entries)`
  }

  const violations = []
  const failures = []

  // FAIL-CLOSED (delta mode only): a new or source-changed entry whose source
  // is neither the crates.io registry nor a plain path/workspace entry has no
  // registry timestamp to age-check (git deps) or comes from an unvetted
  // registry, and git deps run arbitrary build scripts in CI. Whole-lockfile
  // audits report such entries as skipped, not failing — otherwise the manual
  // sweep can never pass a repo that carries one deliberately.
  if (baseRef) {
    // An unsupported index/source option applying in EITHER base or head means
    // a public-PyPI timestamp for a new pin verifies nothing (the pin may
    // resolve from the foreign index, or did before the PR removed it).
    const indexTokens = new Set(["--index-url", "--extra-index-url", "-i", "--find-links", "-f", "--no-index", "--trusted-host"])
    const indexApplies = [...baseProblemsAll, ...problems].some((p) => {
      const m = p.match(/^fail-closed option line \(matched: ([^)]*)\)/)
      return m !== null && m[1].split(",").some((t) => indexTokens.has(t))
    })
    // Same fail-closed rule for an unresolved include (-r/-c): the included
    // file is never read, so a pin change there is unaudited even when the
    // directive itself is inherited unchanged from base.
    const includeApplies = [...baseProblemsAll, ...problems].some((p) => p.startsWith("unresolved include directive"))
    if (includeApplies) {
      for (const e of delta) {
        if (e.registry === "pypi") {
          failures.push(`${e.name}@${e.version}: new pin while an unresolved include directive applies in base or head (included file contents are not age-checked) (fail-closed)`)
        }
      }
    }
    if (indexApplies) {
      for (const e of delta) {
        if (e.registry === "pypi") {
          failures.push(`${e.name}@${e.version}: new pin while an unsupported index/source option applies in base or head (fail-closed)`)
        }
      }
    }
    for (const e of delta) {
      if (
        e.registry === null &&
        (e.src ?? "") !== "" &&
        !firstPartyPrefixes.some((p) => e.name.startsWith(p))
      ) {
        failures.push(`${e.name}@${e.version}: new or changed non-registry source (${e.src ?? "unknown"}) (fail-closed)`)
      }
    }
  }

  const wanted = collectEntries(delta, skipped, firstPartyPrefixes)

  // ---------------------------------------------------------------------------
  // Registry queries
  // ---------------------------------------------------------------------------

  const UA = "equationalapplications-release-age-check/1.0 (github.com/equationalapplications)"

  const lastRequestAt = { t: 0 }

  async function fetchJson(url, attemptMax = 5) {
    // attemptMax 5: worst case per request is 5 fetch timeouts (30s each)
    // plus 4 Retry-After waits (60s cap) — about 6.5 minutes, which can
    // approach the 10-minute PR-path job timeout alongside a second worker.
    // The job still fails closed either way.
    for (let attempt = 1; attempt <= attemptMax; attempt++) {
      let res
      try {
        // Reserve the spacing slot synchronously BEFORE awaiting, so concurrent
        // workers cannot compute the same slot and fire together.
        const slot = Math.max(Date.now(), lastRequestAt.t + spacingMs)
        lastRequestAt.t = slot
        if (slot > Date.now()) await new Promise((r) => setTimeout(r, slot - Date.now()))
        res = await fetchImpl(url, {
          headers: { "User-Agent": UA, Accept: "application/json" },
          signal: AbortSignal.timeout(30_000),
        })
      } catch (err) {
        if (attempt === attemptMax) throw err
        await new Promise((r) => setTimeout(r, 1000 * attempt))
        continue
      }
      if (res.status === 404) return null // unknown package/version: fail closed upstream
      if (res.status === 429 || res.status >= 500) {
        const retryAfter = Math.min(Number(res.headers.get("retry-after")) || Math.min(2 ** attempt, 30), 60)
        if (attempt === attemptMax) throw new Error(`HTTP ${res.status} after ${attemptMax} attempts`)
        await new Promise((r) => setTimeout(r, retryAfter * 1000))
        continue
      }
      if (!res.ok) throw new Error(`HTTP ${res.status}`)
      return await res.json()
    }
  }

  // One bulk request per crate name returns every version's created_at, so a
  // lockfile with many versions of one crate costs a single lookup.
  async function cratesVersions(name) {
    const doc = await fetchJson(`https://crates.io/api/v1/crates/${encodeURIComponent(name)}`)
    if (!doc || !Array.isArray(doc.versions)) return null
    const byNum = new Map()
    for (const v of doc.versions) if (v.num && v.created_at) byNum.set(v.num, v.created_at)
    return byNum
  }

  async function pypiCreated(name, version) {
    const doc = await fetchJson(`https://pypi.org/pypi/${encodeURIComponent(name)}/${encodeURIComponent(version)}/json`)
    if (!doc || !doc.urls || !Array.isArray(doc.urls)) return null
    // Youngest file wins: releases can gain files after the first upload, so
    // taking the latest upload time is the fail-closed direction (a new file
    // on an old release is still too new). Prefer the timezone-explicit
    // upload_time_iso_8601 field over the naive upload_time string.
    const times = doc.urls
      .map((u) => u.upload_time_iso_8601 ?? u.upload_time)
      .filter(Boolean)
      .map((t) => new Date(t))
      .filter((d) => !Number.isNaN(d.getTime()))
    if (times.length === 0) return null
    return new Date(Math.max(...times.map((d) => d.getTime()))).toISOString()
  }

  const registryLookup = { crates: cratesVersions, pypi: pypiCreated }

  // Build a flat job list [{registry, name, versions:Set}]; 2 workers plus the
  // global request spacing (1s per request, slot reserved synchronously) keeps
  // us under crates.io rate limits.
  const jobs = []
  for (const [registry, byName] of wanted) {
    for (const [name, versions] of byName) jobs.push({ registry, name, versions })
  }
  let next = 0
  async function worker() {
    while (next < jobs.length) {
      const { registry, name, versions } = jobs[next++]
      if (registry === "crates") {
        let byNum
        try {
          byNum = await cratesVersions(name)
        } catch (err) {
          for (const version of versions)
            failures.push(`${name}@${version} (${registry}): registry query failed after retries: ${err.message}`)
          continue
        }
        for (const version of versions) {
          const created = byNum ? byNum.get(version) : undefined
          if (created === undefined) {
            failures.push(`${name}@${version} (${registry}): no publish timestamp in registry (fail-closed)`)
            continue
          }
          const publishedAt = new Date(created)
          if (Number.isNaN(publishedAt.getTime()))
            failures.push(`${name}@${version} (${registry}): unparsable publish timestamp: ${created}`)
          else if (publishedAt > cutoff)
            violations.push(`${name}@${version} (${registry}) published ${created.slice(0, 10)}`)
        }
      } else {
        for (const version of versions) {
          let created
          try {
            created = await registryLookup[registry](name, version)
          } catch (err) {
            failures.push(`${name}@${version} (${registry}): registry query failed after retries: ${err.message}`)
            continue
          }
          if (!created) {
            failures.push(`${name}@${version} (${registry}): no publish timestamp in registry (fail-closed)`)
            continue
          }
          const publishedAt = new Date(created)
          if (Number.isNaN(publishedAt.getTime()))
            failures.push(`${name}@${version} (${registry}): unparsable publish timestamp: ${created}`)
          else if (publishedAt > cutoff)
            violations.push(`${name}@${version} (${registry}) published ${created.slice(0, 10)}`)
        }
      }
    }
  }
  await Promise.all(Array.from({ length: 2 }, worker))

  // Whole-lockfile sweep: all parse problems; delta mode: only NEW ones.
  for (const p of newProblems) failures.push(p)

  const checkedCount = [...wanted.values()].reduce(
    (n, byName) => n + [...byName.values()].reduce((m, s) => m + s.size, 0),
    0,
  )
  console.log(
    `Checked ${checkedCount} package version(s) across ${jobs.length} packages from ${lockfile} (${scope}) against a ${minDays}-day minimum release age (cutoff ${cutoff.toISOString().slice(0, 10)}).`,
  )
  if (skipped.length)
    console.log(
      `Not age-checkable / exempt, skipped: ${
        [...new Set(skipped.map((s) => `${s.name}@${s.version} (${s.reason})`))].sort().join(", ")
      }`,
    )
  if (failures.length)
    console.error(`Could not verify (fail-closed):\n  ${failures.sort().join("\n  ")}`)
  if (violations.length) {
    console.error(`Too new for policy:\n  ${violations.sort().join("\n  ")}`)
    console.error(
      `Resolve by waiting until these publish dates are ${minDays} days old, or pinning releases published before ${cutoff.toISOString().slice(0, 10)}.`,
    )
  }
  return violations.length || failures.length ? 1 : 0
}

// CLI guard: run only when executed directly, so test files can import the
// exported helpers without triggering a check.
const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (isMain) {
  process.exit(await runCheck())
}
