#!/usr/bin/env node
// Supply-chain policy (dependency-supply-chain-sop.md): every registry
// dependency version must have been published at least MIN_DAYS days ago
// (default 14), so a poisoned upstream release has time to be caught and
// unpublished before it is installed. FAIL-CLOSED: a version whose registry
// timestamp cannot be determined is a failure, not a skip.
//
// Covers Rust (Cargo.lock) and Python (requirements*.txt, uv.lock,
// poetry.lock) lockfiles. curated-thoughts currently has NO Python lockfile;
// the Python parsers exist so the same gate covers one if one ever lands.
//
// Modes:
//   --base <git-ref>  PR check: age-check only the lockfile DELTA vs <ref>
//                     (entries that are new or changed version). A PR whose
//                     delta is empty passes with no cooldown.
//   (no --base)       Whole-lockfile audit of every registry entry — the
//                     manual `workflow_dispatch` sweep.
//
// Usage:
//   node scripts/check-release-age-cargo-py.mjs [--base origin/main] [--days 14]
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

import fs from "node:fs"
import { execFileSync } from "node:child_process"

const args = process.argv.slice(2)
const arg = (name, fallback) => {
  const i = args.indexOf(name)
  return i >= 0 ? args[i + 1] : fallback
}
const minDays = Number(arg("--days", process.env.MIN_RELEASE_AGE_DAYS || 14))
if (!Number.isFinite(minDays) || minDays <= 0) {
  console.error(`Invalid minimum release age: ${minDays}`)
  process.exit(2)
}
const cutoff = new Date(Date.now() - minDays * 24 * 60 * 60 * 1000)
const firstPartyPrefixes = []
for (let i = 0; i < args.length; i++) {
  if (args[i] === "--first-party-prefix") {
    if (!args[i + 1]) {
      console.error("--first-party-prefix requires a value")
      process.exit(2)
    }
    firstPartyPrefixes.push(args[i + 1])
  }
}
const lockfile = arg("--lockfile", "Cargo.lock")
const baseFlag = args.indexOf("--base")
const baseRef = baseFlag >= 0 ? args[baseFlag + 1] : undefined
if (baseFlag >= 0 && !baseRef) {
  console.error("--base requires a git ref (e.g. --base origin/main)")
  process.exit(2)
}

if (!fs.existsSync(lockfile)) {
  console.error(`Lockfile not found: ${lockfile}`)
  process.exit(2)
}

// ---------------------------------------------------------------------------
// Lockfile parsing -> Map(name -> Set(version)), plus skipped entries.
// Each entry: { name, version, registry: "crates"|"pypi"|null, reason? }
// registry === null means not registry-checkable (path/git dep, etc.).
// ---------------------------------------------------------------------------

// Minimal hand-rolled parse of the Cargo.lock / poetry.lock TOML subset:
// [[package]] array-of-table sections with string-valued keys. Handles quoted
// strings and inline arrays like `dependencies = ["a", "b"]` (ignored anyway).
function parseTomlPackageSections(raw) {
  const sections = []
  let current = null
  for (const lineRaw of raw.split(/\r?\n/)) {
    const line = lineRaw.replace(/#.*$/, "").trim()
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

function isFirstParty(name) {
  return firstPartyPrefixes.some((p) => name.startsWith(p))
}

function collectEntries(entries, skipped) {
  const wanted = new Map() // registry -> Map(name -> Set(version))
  for (const e of entries) {
    if (isFirstParty(e.name)) {
      skipped.push(`${e.name}@${e.version} (first-party exemption)`)
      continue
    }
    if (!e.registry) {
      skipped.push(`${e.name}@${e.version} (${e.reason})`)
      continue
    }
    if (!wanted.has(e.registry)) wanted.set(e.registry, new Map())
    const byName = wanted.get(e.registry)
    if (!byName.has(e.name)) byName.set(e.name, new Set())
    byName.get(e.name).add(e.version)
  }
  return wanted
}

function entriesFromCargoLock(raw) {
  const entries = []
  for (const s of parseTomlPackageSections(raw)) {
    if (s.section !== "package") continue
    const name = s.fields.name
    const version = s.fields.version
    if (!name || !version) continue
    const source = s.fields.source ?? ""
    if (source.startsWith("registry+https://github.com/rust-lang/crates.io-index")) {
      entries.push({ name, version, registry: "crates" })
    } else {
      entries.push({
        name,
        version,
        registry: null,
        reason: source ? `non-registry source ${source}` : "no source (path/git/local)",
      })
    }
  }
  return entries
}

function entriesFromUvLock(raw) {
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
      entries.push({ name, version, registry: "pypi" })
    } else {
      entries.push({
        name,
        version,
        registry: null,
        reason: source ? `non-registry source ${source}` : "no registry source",
      })
    }
  }
  return entries
}

function entriesFromPoetryLock(raw) {
  // poetry.lock [[package]] with source section or name = "pkg (==1.2.3)".
  const entries = []
  for (const s of parseTomlPackageSections(raw)) {
    if (s.section !== "package") continue
    const name = (s.fields.name ?? "").split(" ")[0]
    const version = (s.fields.version ?? "").replace(/^[=<>!~^ ]+/, "")
    if (!name || !version) continue
    if ((s.fields.source ?? "pypi") === "pypi") {
      entries.push({ name, version, registry: "pypi" })
    } else {
      entries.push({ name, version, registry: null, reason: `source ${s.fields.source}` })
    }
  }
  return entries
}

function entriesFromRequirements(raw) {
  // requirements*.txt: pinned lines `name==version`. Unpinned lines cannot be
  // age-checked -> fail-closed violation via the unpinned list.
  const entries = []
  for (const lineRaw of raw.split(/\r?\n/)) {
    const line = lineRaw.replace(/#.*$/, "").trim()
    if (!line || line.startsWith("-")) continue // -r/-e/--flags not handled
    const m = line.match(/^([A-Za-z0-9_.\[\]-]+)==([^;\s]+)/)
    if (!m) continue
    entries.push({ name: m[1].toLowerCase(), version: m[2], registry: "pypi" })
  }
  return entries
}

function entriesForFile(path, raw) {
  const base = path.split("/").pop()
  if (base === "Cargo.lock") return entriesFromCargoLock(raw)
  if (base === "uv.lock") return entriesFromUvLock(raw)
  if (base === "poetry.lock") return entriesFromPoetryLock(raw)
  if (/^requirements.*\.txt$/.test(base)) return entriesFromRequirements(raw)
  console.error(`Unsupported lockfile format: ${path}`)
  process.exit(2)
}

const currentRaw = fs.readFileSync(lockfile, "utf8")
const currentEntries = entriesForFile(lockfile, currentRaw)

let scope
let wanted
const skipped = []
if (baseRef) {
  let baseRaw
  try {
    const repoRoot = execFileSync("git", ["rev-parse", "--show-toplevel"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    }).trim()
    baseRaw = execFileSync("git", ["show", `${baseRef}:${lockfile}`], {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    })
  } catch (err) {
    console.error(`Could not read ${lockfile} from ${baseRef}: ${String(err.stderr ?? err.message).trim()}`)
    process.exit(2)
  }
  const baseSet = new Set()
  for (const e of entriesForFile(lockfile, baseRaw)) baseSet.add(`${e.name}@${e.version}`)
  const delta = currentEntries.filter((e) => !baseSet.has(`${e.name}@${e.version}`))
  const inherited = currentEntries.length - delta.length
  if (delta.length === 0) {
    console.log(`No dependency changes vs ${baseRef}; nothing to age-check.`)
    console.log(
      `${inherited} entries inherited unchanged from ${baseRef} are not re-audited in delta mode (workflow_dispatch runs the whole-lockfile audit).`,
    )
    process.exit(0)
  }
  wanted = collectEntries(delta, skipped)
  const versionCount = delta.reduce((n, e) => n + 1, 0)
  scope = `delta vs ${baseRef} (${versionCount} changed entr${versionCount === 1 ? "y" : "ies"}; ${inherited} inherited unchanged)`
} else {
  wanted = collectEntries(currentEntries, skipped)
  scope = `whole lockfile (${currentEntries.length} entries)`
}

// ---------------------------------------------------------------------------
// Registry queries
// ---------------------------------------------------------------------------

const violations = []
const failures = []
const UA = "equationalapplications-release-age-check/1.0 (github.com/equationalapplications)"

const lastRequestAt = { t: 0 }
const MIN_REQUEST_INTERVAL_MS = 1000 // global spacing across all workers

async function fetchJson(url, attemptMax = 10) {
  for (let attempt = 1; attempt <= attemptMax; attempt++) {
    let res
    try {
      const wait = lastRequestAt.t + MIN_REQUEST_INTERVAL_MS - Date.now()
      if (wait > 0) await new Promise((r) => setTimeout(r, wait))
      lastRequestAt.t = Date.now()
      res = await fetch(url, {
        headers: { "User-Agent": UA, Accept: "application/json" },
        signal: AbortSignal.timeout(30_000),
      })
    } catch (err) {
      if (attempt === attemptMax) throw err
      await new Promise((r) => setTimeout(r, 1000 * attempt))
      continue
    }
    if (res.status === 404) return null // unknown package/version: fail closed upstream
    if (res.status === 429 || res.status === 503) {
      const retryAfter = Number(res.headers.get("retry-after")) || Math.min(2 ** attempt, 30)
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
  const upload = doc.urls.find((u) => u.upload_time)
  return upload ? upload.upload_time : null
}

const registryLookup = { crates: cratesVersions, pypi: pypiCreated }

// Build a flat job list [{registry, name, versions:Set}]; 2 workers plus the
// global 300ms request spacing keeps us well under crates.io rate limits.
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

const checkedCount = [...wanted.values()].reduce(
  (n, byName) => n + [...byName.values()].reduce((m, s) => m + s.size, 0),
  0,
)
console.log(
  `Checked ${checkedCount} package version(s) across ${jobs.length} packages from ${lockfile} (${scope}) against a ${minDays}-day minimum release age (cutoff ${cutoff.toISOString().slice(0, 10)}).`,
)
if (skipped.length)
  console.log(`Not age-checkable / exempt, skipped: ${[...new Set(skipped)].sort().join(", ")}`)
if (failures.length)
  console.error(`Could not verify (fail-closed):\n  ${failures.sort().join("\n  ")}`)
if (violations.length) {
  console.error(`Too new for policy:\n  ${violations.sort().join("\n  ")}`)
  console.error(
    `Resolve by waiting until these publish dates are ${minDays} days old, or pinning releases published before ${cutoff.toISOString().slice(0, 10)}.`,
  )
}
process.exit(violations.length || failures.length ? 1 : 0)
