// Tests for scripts/check-release-age-cargo-py.mjs — run with `node --test`
// (wired as `pnpm run test:release-age`). Network-free: runCheck's registry
// fetch and request spacing are injectable, and the delta path runs against a
// throwaway git repo in the OS temp dir.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { execFileSync } from "node:child_process"
import {
  parseTomlPackageSections,
  entriesFromCargoLock,
  entriesFromUvLock,
  entriesFromPoetryLock,
  entriesFromRequirements,
  entriesForFile,
  deltaEntries,
  collectEntries,
  runCheck,
} from "./check-release-age-cargo-py.mjs"

async function runQuiet(argv, opts) {
  const out = []
  const err = []
  const origLog = console.log
  const origErr = console.error
  console.log = (...a) => out.push(a.join(" "))
  console.error = (...a) => err.push(a.join(" "))
  try {
    const code = await runCheck(argv, opts)
    return { code, out, err }
  } finally {
    console.log = origLog
    console.error = origErr
  }
}

function jsonResponse(body, status = 200) {
  return {
    status,
    ok: status >= 200 && status < 300,
    headers: { get: () => null },
    json: async () => body,
  }
}

function crateDoc(createdAtByNum) {
  return {
    versions: Object.entries(createdAtByNum).map(([num, created_at]) => ({ num, created_at })),
  }
}

const tmpDirs = []
process.on("exit", () => {
  for (const d of tmpDirs) fs.rmSync(d, { recursive: true, force: true })
})

function tmpLockfile(name, content) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "release-age-test-"))
  tmpDirs.push(dir)
  const file = path.join(dir, name)
  fs.writeFileSync(file, content)
  return { dir, file }
}

function initGitRepo(dir, fileName, content) {
  fs.writeFileSync(path.join(dir, fileName), content)
  const run = (args) =>
    execFileSync("git", args, {
      cwd: dir,
      encoding: "utf8",
      env: {
        ...process.env,
        GIT_CONFIG_GLOBAL: "/dev/null",
        GIT_CONFIG_SYSTEM: "/dev/null",
        GIT_AUTHOR_NAME: "t",
        GIT_AUTHOR_EMAIL: "t@t",
        GIT_COMMITTER_NAME: "t",
        GIT_COMMITTER_EMAIL: "t@t",
      },
    })
  run(["init", "--quiet"])
  run(["add", fileName])
  run(["-c", "commit.gpgsign=false", "commit", "--quiet", "--no-verify", "-m", "base"])
  return run
}

const DAY = 24 * 60 * 60 * 1000
const iso = (msAgo) => new Date(Date.now() - msAgo).toISOString()

// ---------------------------------------------------------------------------

test("parseTomlPackageSections: package sections, quoted strings, inline arrays, non-package tables", () => {
  const raw = [
    "# comment",
    "[[package]]",
    'name = "serde"',
    'version = "1.0.210"',
    'dependencies = ["a", "b"]',
    "",
    "[[package]]",
    'name = "local"',
    'source = "registry+https://github.com/rust-lang/crates.io-index"',
    "",
    "[metadata]",
    'not_a_package = "x"',
    "",
    "[[package]]",
    'name = "nopath"',
  ].join("\n")
  const sections = parseTomlPackageSections(raw)
  assert.equal(sections.length, 3)
  assert.deepEqual(sections[0], {
    section: "package",
    fields: { name: "serde", version: "1.0.210", dependencies: '["a", "b"]' },
  })
  assert.equal(sections[1].fields.source, "registry+https://github.com/rust-lang/crates.io-index")
  assert.equal(sections[2].fields.name, "nopath")
})

test("stripTomlComment: quote-aware — `#` inside a quoted value survives, trailing comment stripped", () => {
  const sections = parseTomlPackageSections(
    '[[package]]\nname = "evil"\nsource = "git+https://github.com/attacker/evil#deadbeef" # pinned rev\n',
  )
  assert.equal(sections[0].fields.source, "git+https://github.com/attacker/evil#deadbeef")
  const stripped = parseTomlPackageSections('[[package]] # real comment\nname = "x"\n')
  assert.equal(stripped[0].section, "package")
})

test("entriesFromCargoLock: crates.io registry vs path vs git sources", () => {
  const raw = [
    "[[package]]",
    'name = "serde"',
    'version = "1.0.210"',
    'source = "registry+https://github.com/rust-lang/crates.io-index"',
    "",
    "[[package]]",
    'name = "curated-thoughts"',
    'version = "0.1.0"',
    "",
    "[[package]]",
    'name = "evil"',
    'version = "2.0.0"',
    'source = "git+https://github.com/attacker/evil?rev=deadbeef"',
    "",
    "[[package]]",
    'name = "mirror"',
    'version = "1.2.3"',
    'source = "registry+https://example.com/some-mirror"',
  ].join("\n")
  const entries = entriesFromCargoLock(raw)
  assert.deepEqual(entries, [
    { name: "serde", version: "1.0.210", registry: "crates", src: "crates.io" },
    {
      name: "curated-thoughts",
      version: "0.1.0",
      registry: null,
      git: false,
      src: "",
      reason: "no source (path/git/local)",
    },
    {
      name: "evil",
      version: "2.0.0",
      registry: null,
      git: true,
      src: "git+https://github.com/attacker/evil?rev=deadbeef",
      reason: "non-registry source git+https://github.com/attacker/evil?rev=deadbeef",
    },
    {
      name: "mirror",
      version: "1.2.3",
      registry: null,
      git: false,
      src: "registry+https://example.com/some-mirror",
      reason: "non-registry source registry+https://example.com/some-mirror",
    },
  ])
})

test("entriesFromUvLock: pypi registry vs virtual/path sources", () => {
  const raw = [
    "[[package]]",
    'name = "flask"',
    'version = "3.0.0"',
    'source = { registry = "https://pypi.org/simple" }',
    "",
    "[[package]]",
    'name = "app"',
    'version = "0.1.0"',
    'source = { virtual = "." }',
  ].join("\n")
  const entries = entriesFromUvLock(raw)
  assert.equal(entries[0].registry, "pypi")
  assert.equal(entries[1].registry, null)
  assert.equal(entries[1].git, false)
})

test("entriesFromPoetryLock: constraint prefix stripped, source default, explicit non-pypi source", () => {
  const raw = [
    "[[package]]",
    'name = "requests"',
    'version = "2.31.0"',
    "",
    "[[package]]",
    'name = "old-style (==1.2.3)"',
    'version = ">=1.0"',
    "",
    "[[package]]",
    'name = "vendored"',
    'version = "1.0"',
    'source = "git"',
  ].join("\n")
  const entries = entriesFromPoetryLock(raw)
  assert.deepEqual(entries[0], { name: "requests", version: "2.31.0", registry: "pypi", src: "pypi" })
  assert.equal(entries[1].name, "old-style")
  assert.equal(entries[1].version, "1.0")
  assert.equal(entries[2].registry, null)
  assert.equal(entries[2].git, true)
})

test("entriesFromRequirements: pins, extras, env markers, comments, continuations; unpinned and foreign-index/option lines fail closed", () => {
  const problems = []
  const entries = entriesFromRequirements(
    [
      "# header comment",
      "Flask[async]==3.0.0 ; python_version < '3.9'",
      "requests==2.31.0  # inline comment",
      "-r dev-requirements.txt",
      "--index-url https://evil.example/simple", // foreign index -> fail closed
      "-e git+https://github.com/attacker/x#egg=x", // editable git -> fail closed
      "somepkg", // unpinned -> fail closed
      "other==1.0 \\",
      "    --hash=sha256:abc",
    ].join("\n"),
    problems,
  )
  assert.deepEqual(entries, [
    { name: "flask", version: "3.0.0", registry: "pypi", src: "pypi" },
    { name: "requests", version: "2.31.0", registry: "pypi", src: "pypi" },
    { name: "other", version: "1.0", registry: "pypi", src: "pypi" },
  ])
  assert.equal(problems.length, 4)
  assert.match(problems[0], /include directive.*-r dev-requirements/)
  assert.match(problems[1], /install.*--index-url/)
  assert.match(problems[2], /install.*-e git\+/)
  assert.match(problems[3], /unpinned or unparseable requirement line .*somepkg/)
})

test("entriesForFile: dispatches by basename, throws on unsupported format", () => {
  assert.equal(entriesForFile("a/b/Cargo.lock", '[[package]]\nname="x"\nversion="1"\n').length, 1)
  assert.equal(entriesForFile("requirements-prod.txt", "x==1\n").length, 1)
  assert.throws(() => entriesForFile("composer.lock", "{}"), /Unsupported lockfile format/)
})

test("deltaEntries: new packages and version changes only", () => {
  const base = [
    { name: "a", version: "1.0.0" },
    { name: "b", version: "2.0.0" },
  ]
  const current = [
    { name: "a", version: "1.0.0" }, // unchanged
    { name: "b", version: "3.0.0" }, // version change
    { name: "c", version: "0.1.0" }, // new
  ]
  assert.deepEqual(deltaEntries(current, base), [
    { name: "b", version: "3.0.0" },
    { name: "c", version: "0.1.0" },
  ])
})

test("collectEntries: groups by registry, skips non-registry and first-party", () => {
  const skipped = []
  const wanted = collectEntries(
    [
      { name: "a", version: "1.0", registry: "crates" },
      { name: "a", version: "2.0", registry: "crates" },
      { name: "p", version: "1.0", registry: "pypi" },
      { name: "local", version: "0.1", registry: null, reason: "no source (path/git/local)" },
      { name: "@ea/thing", version: "9.9", registry: "crates" },
    ],
    skipped,
    ["@ea/"],
  )
  assert.deepEqual([...wanted.get("crates").get("a")], ["1.0", "2.0"])
  assert.deepEqual([...wanted.get("pypi").get("p")], ["1.0"])
  assert.equal(wanted.get("crates").has("@ea/thing"), false)
  assert.deepEqual(skipped.map((s) => s.reason), ["no source (path/git/local)", "first-party exemption"])
})

test("runCheck: usage errors return 2 without network", async () => {
  assert.equal((await runQuiet(["--days", "0"])).code, 2)
  assert.equal((await runQuiet(["--lockfile", "/nonexistent/Cargo.lock"], { spacingMs: 1 })).code, 2)
  assert.equal((await runQuiet(["--lockfile", "Cargo.lock", "--base"])).code, 2)
})

test("runCheck: unsupported lockfile format returns 2", async () => {
  const { file } = tmpLockfile("composer.lock", "{}")
  const { code, err } = await runQuiet(["--lockfile", file], { spacingMs: 1 })
  assert.equal(code, 2)
  assert.match(err.join("\n"), /Unsupported lockfile format/)
})

test("runCheck: empty delta passes without registry queries", async () => {
  const content =
    '[[package]]\nname = "serde"\nversion = "1.0.210"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n'
  const { dir } = tmpLockfile("Cargo.lock", content)
  initGitRepo(dir, "Cargo.lock", content)
  let fetchCalls = 0
  const { code, out } = await runQuiet(["--lockfile", path.join(dir, "Cargo.lock"), "--base", "HEAD"], {
    fetchImpl: async () => {
      fetchCalls++
      return jsonResponse({})
    },
    spacingMs: 1,
  })
  assert.equal(code, 0)
  assert.equal(fetchCalls, 0)
  assert.match(out.join("\n"), /No dependency changes vs HEAD/)
})

test("runCheck: whole-lockfile sweep flags a too-young version and passes an old one", async () => {
  const content = [
    "[[package]]",
    'name = "young"',
    'version = "2.0.0"',
    'source = "registry+https://github.com/rust-lang/crates.io-index"',
    "",
    "[[package]]",
    'name = "old"',
    'version = "1.0.0"',
    'source = "registry+https://github.com/rust-lang/crates.io-index"',
  ].join("\n")
  const { file } = tmpLockfile("Cargo.lock", content)
  const fetchCalls = []
  const { code, err } = await runQuiet(["--lockfile", file], {
    fetchImpl: async (url) => {
      fetchCalls.push(url)
      if (String(url).includes("/young")) return jsonResponse(crateDoc({ "2.0.0": iso(2 * DAY) }))
      return jsonResponse(crateDoc({ "1.0.0": iso(400 * DAY) }))
    },
    spacingMs: 1,
  })
  assert.equal(code, 1)
  assert.match(err.join("\n"), /young@2\.0\.0 .* published \d{4}-\d{2}-\d{2}/)
  assert.equal(err.join("\n").includes("old@1.0.0"), false)
  assert.equal(fetchCalls.length, 2)
})

test("runCheck: unknown version (404) fails closed", async () => {
  const { file } = tmpLockfile(
    "Cargo.lock",
    '[[package]]\nname = "ghost"\nversion = "9.9.9"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n',
  )
  const { code, err } = await runQuiet(["--lockfile", file], {
    fetchImpl: async () => jsonResponse({}, 404),
    spacingMs: 1,
  })
  assert.equal(code, 1)
  assert.match(err.join("\n"), /no publish timestamp in registry \(fail-closed\)/)
})

test("runCheck: registry query failure fails closed", async () => {
  const { file } = tmpLockfile(
    "Cargo.lock",
    '[[package]]\nname = "netfail"\nversion = "1.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n',
  )
  const { code, err } = await runQuiet(["--lockfile", file], {
    fetchImpl: async () => {
      throw new Error("boom")
    },
    spacingMs: 1,
  })
  assert.equal(code, 1)
  assert.match(err.join("\n"), /registry query failed after retries/)
})

test("runCheck: newly added git-source dependency fails in delta mode, established git dep skips in whole mode", async () => {
  const registryEntry = (name, version) =>
    `[[package]]\nname = "${name}"\nversion = "${version}"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n`
  const gitEntry = (name, version) =>
    `[[package]]\nname = "${name}"\nversion = "${version}"\nsource = "git+https://github.com/example/${name}#abc"\n`
  const base = registryEntry("serde", "1.0.210")
  const withGit = base + gitEntry("evil", "2.0.0")
  const { dir } = tmpLockfile("Cargo.lock", base)
  const run = initGitRepo(dir, "Cargo.lock", base)
  const lock = path.join(dir, "Cargo.lock")

  fs.writeFileSync(lock, withGit)
  run(["add", "Cargo.lock"])
  run(["commit", "--quiet", "--no-verify", "-m", "add git dep"])

  const delta = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], {
    spacingMs: 1,
    fetchImpl: async () => jsonResponse(crateDoc({ "1.0.210": iso(400 * DAY) })),
  })
  assert.equal(delta.code, 1)
  assert.match(delta.err.join("\n"), /evil@2\.0\.0: new or changed non-registry source/)

  // Whole-lockfile mode: the established git dep is skipped (informational),
  // the registry dep is old -> exit 0.
  const whole = await runQuiet(["--lockfile", lock], {
    spacingMs: 1,
    fetchImpl: async () => jsonResponse(crateDoc({ "1.0.210": iso(400 * DAY) })),
  })
  assert.equal(whole.code, 0)
  assert.match(whole.out.join("\n"), /evil@2\.0\.0 .*git\+/)
})

test("runCheck: unpinned requirements line fails closed in whole mode", async () => {
  const { file } = tmpLockfile("requirements.txt", "flask==3.0.0\nloose-package\n")
  const { code, err } = await runQuiet(["--lockfile", file], {
    fetchImpl: async () => jsonResponse({ urls: [{ upload_time: iso(400 * DAY) }] }),
    spacingMs: 1,
  })
  assert.equal(code, 1)
  assert.match(err.join("\n"), /unpinned or unparseable requirement line/)
})

test("runCheck: first-party exemption exempts a too-young version", async () => {
  const content =
    '[[package]]\nname = "ea-widget"\nversion = "1.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n'
  const { file } = tmpLockfile("Cargo.lock", content)
  let fetchCalls = 0
  const { code } = await runQuiet(["--lockfile", file, "--first-party-prefix", "ea-"], {
    fetchImpl: async () => {
      fetchCalls++
      return jsonResponse(crateDoc({ "1.0.0": iso(0) }))
    },
    spacingMs: 1,
  })
  assert.equal(code, 0)
  assert.equal(fetchCalls, 0)
})

test("runCheck: a SAME-VERSION source swap (e.g. a [patch] to git) is a delta change and fails", async () => {
  const serdeReg =
    '[[package]]\nname = "serde"\nversion = "1.0.210"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n'
  const serdeGit =
    '[[package]]\nname = "serde"\nversion = "1.0.210"\nsource = "git+https://github.com/attacker/serde#deadbeef"\n'
  const { dir } = tmpLockfile("Cargo.lock", serdeReg)
  const run = initGitRepo(dir, "Cargo.lock", serdeReg)
  const lock = path.join(dir, "Cargo.lock")
  fs.writeFileSync(lock, serdeGit)
  run(["add", "Cargo.lock"])
  run(["commit", "--quiet", "--no-verify", "-m", "swap source"])

  const { code, err } = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], { spacingMs: 1 })
  assert.equal(code, 1)
  assert.match(err.join("\n"), /serde@1\.0\.210: new or changed non-registry source/)

  // A foreign registry swap fails the same way.
  const serdeMirror =
    '[[package]]\nname = "serde"\nversion = "1.0.210"\nsource = "registry+https://evil.example/mirror"\n'
  fs.writeFileSync(lock, serdeMirror)
  run(["add", "Cargo.lock"])
  run(["commit", "--quiet", "--no-verify", "-m", "swap to mirror"])
  const mirror = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], { spacingMs: 1 })
  assert.equal(mirror.code, 1)
  assert.match(mirror.err.join("\n"), /serde@1\.0\.210: new or changed non-registry source/)
})

test("runCheck: NEW unpinned or foreign-index requirement lines fail delta mode even with an empty entry delta", async () => {
  const clean = "flask==3.0.0\n"
  const dirtyUnpinned = "flask==3.0.0\nloose-package\n"
  const dirtyIndex = "flask==3.0.0\n--index-url https://evil.example/simple\n"
  const { dir } = tmpLockfile("requirements.txt", clean)
  const run = initGitRepo(dir, "requirements.txt", clean)
  const lock = path.join(dir, "requirements.txt")

  // Inherited unpinned line (base AND current have it), no new pins: passes.
  fs.writeFileSync(lock, dirtyUnpinned)
  run(["add", "requirements.txt"])
  run(["commit", "--quiet", "--no-verify", "-m", "inherit"])
  fs.writeFileSync(lock, dirtyUnpinned) // unchanged content
  const inherited = await runQuiet(["--lockfile", lock, "--base", "HEAD"], { spacingMs: 1 })
  assert.equal(inherited.code, 0)

  // A NEWLY ADDED unpinned line: empty entry delta, still exit 1.
  fs.writeFileSync(lock, dirtyUnpinned + "another-loose\n")
  run(["add", "requirements.txt"])
  run(["commit", "--quiet", "--no-verify", "-m", "add unpinned"])
  const newUnpinned = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], { spacingMs: 1 })
  assert.equal(newUnpinned.code, 1)
  assert.match(newUnpinned.err.join("\n"), /New unage-checkable lines/)

  // A NEWLY ADDED foreign-index line: same.
  fs.writeFileSync(lock, dirtyIndex)
  run(["add", "requirements.txt"])
  run(["commit", "--quiet", "--no-verify", "-m", "add evil index"])
  const newIndex = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], { spacingMs: 1 })
  assert.equal(newIndex.code, 1)
  assert.match(newIndex.err.join("\n"), /foreign index or editable\/git\/URL install/)
})

test("runCheck: a brand-new lockfile (absent on base) treats the whole file as the delta", async () => {
  const { dir } = tmpLockfile("requirements.txt", "placeholder\n")
  const run = initGitRepo(dir, "placeholder.txt", "placeholder\n")
  const lock = path.join(dir, "requirements.txt")
  fs.writeFileSync(lock, "flask==3.0.0\n") // lockfile NOT on base
  run(["add", "requirements.txt"])
  run(["commit", "--quiet", "--no-verify", "-m", "add lockfile"])

  const { code, out } = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], {
    fetchImpl: async () => jsonResponse({ urls: [{ upload_time: iso(400 * DAY) }] }),
    spacingMs: 1,
  })
  assert.equal(code, 0)
  // The whole file becomes the delta (base had no lockfile), and the run is
  // an age-check delta run — not a "nothing to age-check" early exit and not
  // a usage error.
  assert.match(out.join("\n"), /delta vs HEAD~1 \(1 changed entry; 0 inherited unchanged\)/)
  assert.match(out.join("\n"), /Checked 1 package version/)
})

test("runCheck: uv git-source dependency fails in delta mode", async () => {
  const base =
    '[[package]]\nname = "flask"\nversion = "3.0.0"\nsource = { registry = "https://pypi.org/simple" }\n'
  const withGit =
    base +
    '[[package]]\nname = "evil"\nversion = "2.0.0"\nsource = { git = "https://github.com/attacker/evil?rev=abc" }\n'
  const { dir } = tmpLockfile("uv.lock", base)
  const run = initGitRepo(dir, "uv.lock", base)
  const lock = path.join(dir, "uv.lock")
  fs.writeFileSync(lock, withGit)
  run(["add", "uv.lock"])
  run(["commit", "--quiet", "--no-verify", "-m", "add git dep"])
  const { code, err } = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], { spacingMs: 1 })
  assert.equal(code, 1)
  assert.match(err.join("\n"), /evil@2\.0\.0: new or changed non-registry source/)
})

test("runCheck: poetry [package.source] tables and a new uv workspace member behave per policy", async () => {
  // poetry.lock with a [package.source] table: rejected outright (the
  // parser cannot verify nested git/URL sources).
  const { file: pfile } = tmpLockfile(
    "poetry.lock",
    '[[package]]\nname = "requests"\nversion = "2.31.0"\n\n[package.source]\ntype = "git"\nurl = "https://github.com/attacker/requests"\n',
  )
  const poetry = await runQuiet(["--lockfile", pfile], {
    fetchImpl: async () => jsonResponse({ urls: [{ upload_time: iso(400 * DAY) }] }),
    spacingMs: 1,
  })
  assert.equal(poetry.code, 1)
  assert.match(poetry.err.join("\n"), /\[package\.source\]/)

  // uv: a newly added local workspace member (virtual source) is skipped,
  // not failed — same policy as Cargo path entries.
  const uvBase =
    '[[package]]\nname = "flask"\nversion = "3.0.0"\nsource = { registry = "https://pypi.org/simple" }\n'
  const uvWithMember =
    uvBase + '[[package]]\nname = "app"\nversion = "0.1.0"\nsource = { virtual = "." }\n'
  const { dir } = tmpLockfile("uv.lock", uvBase)
  const run = initGitRepo(dir, "uv.lock", uvBase)
  const lock = path.join(dir, "uv.lock")
  fs.writeFileSync(lock, uvWithMember)
  run(["add", "uv.lock"])
  run(["-c", "commit.gpgsign=false", "commit", "--quiet", "--no-verify", "-m", "add member"])
  const uv = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], {
    fetchImpl: async () => jsonResponse({ urls: [{ upload_time: iso(400 * DAY) }] }),
    spacingMs: 1,
  })
  assert.equal(uv.code, 0)
  assert.match(uv.out.join("\n"), /app@0\.1\.0 .*virtual/)
})

test("runCheck: delta mode — a too-new version bump fails and a path-entry version change skips", async () => {
  const mk = (name, version) =>
    `[[package]]\nname = "${name}"\nversion = "${version}"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n`
  const { dir } = tmpLockfile("Cargo.lock", mk("serde", "1.0.210") + '[[package]]\nname = "app"\nversion = "0.1.0"\n')
  const run = initGitRepo(dir, "Cargo.lock", mk("serde", "1.0.210") + '[[package]]\nname = "app"\nversion = "0.1.0"\n')
  const lock = path.join(dir, "Cargo.lock")

  // Registry version bump to a too-new version: exit 1.
  fs.writeFileSync(lock, mk("serde", "1.0.211") + '[[package]]\nname = "app"\nversion = "0.1.0"\n')
  run(["add", "Cargo.lock"])
  run(["-c", "commit.gpgsign=false", "commit", "--quiet", "--no-verify", "-m", "bump"])
  const bump = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], {
    fetchImpl: async () =>
      jsonResponse(crateDoc({ "1.0.210": iso(400 * DAY), "1.0.211": iso(2 * DAY) })),
    spacingMs: 1,
  })
  assert.equal(bump.code, 1)
  assert.match(bump.err.join("\n"), /serde@1\.0\.211 .* published \d{4}-\d{2}-\d{2}/)

  // Path/workspace entry version change: skipped, not failed — exit 0.
  fs.writeFileSync(lock, mk("serde", "1.0.210") + '[[package]]\nname = "app"\nversion = "0.2.0"\n')
  run(["add", "Cargo.lock"])
  run(["-c", "commit.gpgsign=false", "commit", "--quiet", "--no-verify", "-m", "local bump"])
  const local = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], {
    fetchImpl: async () => jsonResponse(crateDoc({ "1.0.210": iso(400 * DAY) })),
    spacingMs: 1,
  })
  assert.equal(local.code, 0)
  assert.match(local.out.join("\n"), /app@0\.2\.0 .*no source/)
})

test("runCheck: PyPI timestamp takes the YOUNGEST file's upload time (fail-closed)", async () => {
  const { file } = tmpLockfile("requirements.txt", "flask==3.0.0\n")
  const old = iso(400 * DAY)
  const young = iso(2 * DAY)
  const { code, err } = await runQuiet(["--lockfile", file], {
    fetchImpl: async () =>
      jsonResponse({
        urls: [
          { upload_time_iso_8601: old },
          { upload_time_iso_8601: young }, // file added later -> release is young
        ],
      }),
    spacingMs: 1,
  })
  assert.equal(code, 1)
  assert.match(err.join("\n"), /flask@3\.0\.0 .* published \d{4}-\d{2}-\d{2}/)
})

test("runCheck: transient 5xx is retried, not failed immediately", async () => {
  const { file } = tmpLockfile(
    "Cargo.lock",
    '[[package]]\nname = "flaky"\nversion = "1.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n',
  )
  let calls = 0
  const { code } = await runQuiet(["--lockfile", file], {
    fetchImpl: async () => {
      calls++
      if (calls <= 2) return jsonResponse({}, 502)
      return jsonResponse(crateDoc({ "1.0.0": iso(400 * DAY) }))
    },
    spacingMs: 1,
  })
  assert.equal(code, 0)
  assert.equal(calls, 3)
})

test("entriesFromRequirements: pip comment stripping ignores quotes (delta nit 3)", () => {
  const problems = []
  const entries = entriesFromRequirements(
    ["flask==3.0.0 \"weird unmatched quote # trailing note"].join("\n"),
    problems,
  )
  assert.deepEqual(entries, [
    { name: "flask", version: "3.0.0", registry: "pypi", src: "pypi" },
  ])
  assert.equal(problems.length, 0)
})

test("entriesFromRequirements: include/constraint directives fail closed (T1)", () => {
  for (const line of ["-r deps.txt", "-rdeps.txt", "--requirement=deps.txt", "--requirement deps.txt", "-c cons.txt", "--constraint cons.txt"]) {
    const problems = []
    entriesFromRequirements(`flask==3.0.0\n${line}\n`, problems)
    assert.equal(problems.length, 1, line)
    assert.match(problems[0], /include directive/, line)
  }
})

test("entriesFromRequirements: a comment line ending in a backslash does not swallow the next pin (T4)", () => {
  const problems = []
  const entries = entriesFromRequirements("# note \\\nflask==3.0.0\n", problems)
  assert.deepEqual(entries, [{ name: "flask", version: "3.0.0", registry: "pypi", src: "pypi" }])
  assert.equal(problems.length, 0)
})

test("entriesFromRequirements: attached short options (-iURL, -e<path>, -fURL) fail closed (T5)", () => {
  for (const line of ["-ihttps://evil.example/simple", "-e./local-pkg", "-fhttps://evil.example/links"]) {
    const problems = []
    entriesFromRequirements(`${line}\n`, problems)
    assert.equal(problems.length, 1, line)
    assert.match(problems[0], /foreign index or editable/, line)
  }
})

const POETRY_PKG = (name, url) =>
  `[[package]]\nname = "${name}"\nversion = "1.0.0"\n\n[package.source]\ntype = "git"\nurl = "${url}"\n\n`

test("runCheck: removing a poetry source table passes; changing a source's url fails (T2, T3)", async () => {
  const two = POETRY_PKG("aaa", "https://git.example/a") + POETRY_PKG("bbb", "https://git.example/b")
  const one = POETRY_PKG("aaa", "https://git.example/a")
  const { dir } = tmpLockfile("poetry.lock", two)
  const run = initGitRepo(dir, "poetry.lock", two)
  const lock = path.join(dir, "poetry.lock")
  fs.writeFileSync(lock, one)
  run(["add", "poetry.lock"])
  run(["-c", "commit.gpgsign=false", "commit", "--quiet", "--no-verify", "-m", "drop b"])
  const removed = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], {
    fetchImpl: async () => jsonResponse({ urls: [{ upload_time: iso(400 * DAY) }] }),
    spacingMs: 1,
  })
  assert.equal(removed.code, 0, removed.err.join("\n"))

  const changed = POETRY_PKG("aaa", "https://git.example/EVIL")
  fs.writeFileSync(lock, changed)
  run(["add", "poetry.lock"])
  run(["-c", "commit.gpgsign=false", "commit", "--quiet", "--no-verify", "-m", "change url"])
  const swap = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], {
    fetchImpl: async () => jsonResponse({ urls: [{ upload_time: iso(400 * DAY) }] }),
    spacingMs: 1,
  })
  assert.equal(swap.code, 1)
  assert.match(swap.err.join("\n"), /New unage-checkable lines/)
})

test("runCheck: a new pin fails while an unsupported index applies in base OR head (T6)", async () => {
  const withIndex = "--index-url https://internal.example/simple\nflask==3.0.0\n"
  const noIndexPlusNew = "flask==3.0.0\nrequests==2.31.0\n"
  const old = async () => jsonResponse({ urls: [{ upload_time: iso(400 * DAY) }] })
  const { dir } = tmpLockfile("requirements.txt", withIndex)
  const run = initGitRepo(dir, "requirements.txt", withIndex)
  const lock = path.join(dir, "requirements.txt")

  // Base had the index; head removes it AND adds a pin.
  fs.writeFileSync(lock, noIndexPlusNew)
  run(["add", "requirements.txt"])
  run(["-c", "commit.gpgsign=false", "commit", "--quiet", "--no-verify", "-m", "drop index, add pin"])
  const removed = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], { fetchImpl: old, spacingMs: 1 })
  assert.equal(removed.code, 1)
  assert.match(removed.err.join("\n"), /requests@2\.31\.0.*index/)

  // Head keeps the (inherited) index and adds a pin.
  fs.writeFileSync(lock, withIndex + "click==8.1.0\n")
  run(["add", "requirements.txt"])
  run(["-c", "commit.gpgsign=false", "commit", "--quiet", "--no-verify", "-m", "keep index, add pin"])
  const kept = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], { fetchImpl: old, spacingMs: 1 })
  assert.equal(kept.code, 1)
  assert.match(kept.err.join("\n"), /click@8\.1\.0.*index/)
})

test("runCheck: request spacing reserves slots synchronously (no two calls together)", async () => {
  const names = ["a", "b", "c"]
  const content = names
    .map(
      (n) =>
        `[[package]]\nname = "${n}"\nversion = "1.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n`,
    )
    .join("\n")
  const { file } = tmpLockfile("Cargo.lock", content)
  const callTimes = []
  const SPACING = 30
  const { code } = await runQuiet(["--lockfile", file], {
    fetchImpl: async () => {
      callTimes.push(Date.now())
      return jsonResponse(crateDoc({ "1.0.0": iso(400 * DAY) }))
    },
    spacingMs: SPACING,
  })
  assert.equal(code, 0)
  assert.equal(callTimes.length, 3)
  for (let i = 1; i < callTimes.length; i++) {
    assert.ok(
      callTimes[i] - callTimes[i - 1] >= SPACING - 5,
      `calls ${i - 1}->${i} only ${callTimes[i] - callTimes[i - 1]}ms apart (spacing ${SPACING})`,
    )
  }
})
