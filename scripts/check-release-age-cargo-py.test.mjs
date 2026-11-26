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

function tmpLockfile(name, content) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "release-age-test-"))
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
        GIT_AUTHOR_NAME: "t",
        GIT_AUTHOR_EMAIL: "t@t",
        GIT_COMMITTER_NAME: "t",
        GIT_COMMITTER_EMAIL: "t@t",
      },
    })
  run(["init", "--quiet"])
  run(["add", fileName])
  run(["commit", "--quiet", "--no-verify", "-m", "base"])
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
  assert.deepEqual(entries[0], { name: "requests", version: "2.31.0", registry: "pypi" })
  assert.equal(entries[1].name, "old-style")
  assert.equal(entries[1].version, "1.0")
  assert.equal(entries[2].registry, null)
  assert.equal(entries[2].git, true)
})

test("entriesFromRequirements: pins, extras, env markers, comments, options, continuations; unpinned fails closed", () => {
  const problems = []
  const entries = entriesFromRequirements(
    [
      "# header comment",
      "Flask[async]==3.0.0 ; python_version < '3.9'",
      "requests==2.31.0  # inline comment",
      "-r dev-requirements.txt",
      "--index-url https://example.com",
      "somepkg", // unpinned -> fail closed
      "other==1.0 \\",
      "    --hash=sha256:abc",
    ].join("\n"),
    problems,
  )
  assert.deepEqual(entries, [
    { name: "flask", version: "3.0.0", registry: "pypi" },
    { name: "requests", version: "2.31.0", registry: "pypi" },
    { name: "other", version: "1.0", registry: "pypi" },
  ])
  assert.equal(problems.length, 1)
  assert.match(problems[0], /unpinned or unparseable requirement line .*somepkg/)
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
  assert.match(delta.err.join("\n"), /evil@2\.0\.0: new or changed non-crates\.io source/)

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
  assert.match(err.join("\n"), /serde@1\.0\.210: new or changed non-crates\.io source/)

  // A foreign registry swap fails the same way.
  const serdeMirror =
    '[[package]]\nname = "serde"\nversion = "1.0.210"\nsource = "registry+https://evil.example/mirror"\n'
  fs.writeFileSync(lock, serdeMirror)
  run(["add", "Cargo.lock"])
  run(["commit", "--quiet", "--no-verify", "-m", "swap to mirror"])
  const mirror = await runQuiet(["--lockfile", lock, "--base", "HEAD~1"], { spacingMs: 1 })
  assert.equal(mirror.code, 1)
  assert.match(mirror.err.join("\n"), /serde@1\.0\.210: new or changed non-crates\.io source/)
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
