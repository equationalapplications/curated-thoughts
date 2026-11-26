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
// - poetry.lock stores per-package source in a nested [package.source] table
//   this parser does not read; poetry entries without an inline
//   `source = "pypi"` field default to pypi, and a git-sourced package then
//   404s on the PyPI lookup, which fails closed.

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

export function entriesFromPoetryLock(raw) {
  // poetry.lock [[package]] with source section or name = "pkg (==1.2.3)".
  // See the header note: inline source fields only; missing source defaults
  // to pypi and git-sourced packages fail closed at the PyPI 404.
  const entries = []
  for (const s of parseTomlPackageSections(raw)) {
    if (s.section !== "package") continue
    const name = (s.fields.name ?? "").split(" ")[0]
    const version = (s.fields.version ?? "").replace(/^[=<>!~^ ]+/, "")
    if (!name || !version) continue
    if ((s.fields.source ?? "pypi") === "pypi") {
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
export function entriesFromRequirements(raw, problems = []) {
  const entries = []
  const lines = raw.replace(/\\\r?\n/g, " ").split(/\r?\n/)
  for (const lineRaw of lines) {
    const line = lineRaw.replace(/#.*$/, "").trim()
    if (!line) continue
    if (line.startsWith("-")) {
      // FAIL-CLOSED on supply-chain-relevant option lines: a foreign index
      // (-i/--index-url/--extra-index-url), an editable/URL/git install
      // (-e/--editable, git+), or a find-links source cannot be age-checked
      // from a version pin. Other option lines (e.g. -r includes) are skipped.
      if (
        /^(--index-url|--extra-index-url|-i\b|-e\b|--editable|--find-links|-f\b)/.test(line) ||
        /git\+/.test(line)
      ) {
        problems.push(`fail-closed option line (foreign index or editable/git/URL install): ${lineRaw.trim()}`)
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

export function entriesForFile(path, raw, problems = []) {
  const base = path.split("/").pop()
  if (base === "Cargo.lock") return entriesFromCargoLock(raw)
  if (base === "uv.lock") return entriesFromUvLock(raw)
  if (base === "poetry.lock") return entriesFromPoetryLock(raw)
  if (/^requirements.*\.txt$/.test(base)) return entriesFromRequirements(raw, problems)
  throw new Error(`Unsupported lockfile format: ${path}`)
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

  // Parse-level fail-closed problems (e.g. unpinned requirement lines).
  const problems = []
  const currentRaw = fs.readFileSync(lockfile, "utf8")
  let currentEntries
  try {
    currentEntries = entriesForFile(lockfile, currentRaw, problems)
  } catch (err) {
    console.error(String(err.message))
    return 2
  }

  let scope
  let delta
  let newProblems = []
  const skipped = []
  if (baseRef) {
    let baseRaw
    try {
      // Run git relative to the lockfile's directory so the check works from
      // any process cwd (CI runs at the repo root; tests use temp repos), and
      // address the blob repo-relatively — `git show ref:<path>` rejects
      // absolute paths.
      const absLock = path.resolve(lockfile)
      const repoRoot = execFileSync("git", ["rev-parse", "--show-toplevel"], {
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
    const baseProblems = []
    delta = deltaEntries(currentEntries, (() => {
      try {
        return entriesForFile(lockfile, baseRaw, baseProblems)
      } catch (err) {
        console.error(String(err.message))
        return null
      }
    })())
    if (delta === null) return 2
    // Only NEW parse problems fail a PR; problems already present on base are
    // inherited state (the whole-lockfile audit's job). An ADDED unpinned or
    // foreign-index line is a new problem and fails even when the entry delta
    // is empty.
    newProblems = problems.filter((p) => !baseProblems.includes(p))
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
    for (const e of delta) {
      if (
        e.registry === null &&
        (e.src ?? "") !== "" &&
        !firstPartyPrefixes.some((p) => e.name.startsWith(p))
      ) {
        failures.push(`${e.name}@${e.version}: new or changed non-crates.io source (${e.src ?? "unknown"}) (fail-closed)`)
      }
    }
  }

  const wanted = collectEntries(delta, skipped, firstPartyPrefixes)

  // ---------------------------------------------------------------------------
  // Registry queries
  // ---------------------------------------------------------------------------

  const UA = "equationalapplications-release-age-check/1.0 (github.com/equationalapplications)"

  const lastRequestAt = { t: 0 }

  async function fetchJson(url, attemptMax = 10) {
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
      if (res.status === 429 || res.status === 503 || res.status >= 500) {
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
