#!/usr/bin/env python3
"""Generate the mixed-provenance activity probe set for the #344 floor re-tune.

Read-only over the scratch brain copy. Mirrors the 2026-10-18 probe shapes
(path / command / pattern per positive fact; near-miss negatives) WITHOUT the
user_stated-only restriction: positives are sampled across the live
population's actual provenance mix (librarian_inferred + user_confirmed).
"""
import json, re, sqlite3, subprocess, random, collections, os, time

DB = "/home/kv-thinkpad-t420-ubuntu/ct-calibration-scratch/brain-dir-20261113/brain.db"
REPO = "/mnt/kv-work/build/worktrees/curated-thoughts/rule10-delivery"
OUT = "/mnt/kv-work/build/worktrees/curated-thoughts/rule10-delivery/calibration-run/grid-20261113-activity"
os.makedirs(OUT, exist_ok=True)

conn = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)

# Temporal eligibility: the SAME conditions the scorer applies at query time
# (wisdom_match.rs:474-478) with the SAME `now` semantics — current epoch ms
# at generation time (the scorer's now_ms is current time at run time).
# CodeRabbit 4568979416: the positive-sampling pool previously ignored
# supersession/expiry, so a probe could expect a fact whose live head is a
# different id — the scorer would count it a miss/FP the sampler never saw.
now_ms = int(time.time() * 1000)
cols = {r[1] for r in conn.execute("PRAGMA table_info(llm_wiki_entries)").fetchall()}
elig_sql = "AND (superseded_by IS NULL OR superseded_at > :now) " if "superseded_by" in cols else ""
elig_sql += "AND (valid_to IS NULL OR valid_to > :now) " if "valid_to" in cols else ""
# Embedding/scheme eligibility (CodeRabbit 4570174108): a positive with a
# NULL embedding_blob or an embedding from ANOTHER scheme can never be
# returned by score_entries — it would be silently counted as a miss and
# bias the floor down. Apply the scorer's `entry_eligibility_where`
# predicates (wisdom_match.rs:465-483) for THIS run's scheme (raw):
# non-null title/body/blob + `AND embed_scheme = 'raw'` on the V27+ shape
# (empty on pre-V27, where rows are de-facto raw).
RUN_SCHEME = "raw"
elig_sql += "AND e.title IS NOT NULL AND e.body IS NOT NULL "
elig_sql += "AND e.embedding_blob IS NOT NULL "
elig_sql += f"AND e.embed_scheme = '{RUN_SCHEME}' " if "embed_scheme" in cols else ""
rows = conn.execute(
    "SELECT id, source_type, title, body FROM llm_wiki_entries e "
    "WHERE e.deleted_at IS NULL " + elig_sql,
    {"now": now_ms}).fetchall()

PATH_RE = re.compile(r"(?:[A-Za-z0-9_\-]+/)+[A-Za-z0-9_\-]+\.(?:md|sh|rs|ts|tsx|json|py|toml|yaml|yml)")
CMD_RE = re.compile(r"`([^`\n]{4,80})`")
HOME_RE = re.compile(r"~/[A-Za-z0-9_\-./]+")

facts = []
for fid, st, title, body in rows:
    facts.append({"id": fid, "source_type": st, "title": title, "body": body})
bodies_all = "\n".join(f["body"] for f in facts)

by_conf = collections.defaultdict(list)
for f in facts:
    paths = set(PATH_RE.findall(f["body"])) | set(p for p in HOME_RE.findall(f["body"]) if "." in p)
    paths = {p for p in paths if len(p) > 6}
    paths = {p.rstrip(".") for p in paths}
    paths = {p for p in paths if len(p) > 6}
    if paths:
        f["paths"] = sorted(paths)
        by_conf[f["source_type"]].append(f)

random.seed(344)
# Take ALL user_confirmed facts with path tokens, then fill to ~30 with librarian_inferred.
picked, used_stems = [], set()
def take(bucket, n, label):
    got = 0
    cand = bucket[:]
    random.shuffle(cand)
    for f in cand:
        if got >= n:
            break
        # choose a path unique to this fact among ALL facts (unambiguous expect)
        ok_paths = [p for p in f["paths"] if bodies_all.count(p) == 1]
        if not ok_paths:
            continue
        p = max(ok_paths, key=len)
        stem = p.rsplit("/", 1)[-1].rsplit(".", 1)[0].lower()
        if stem in used_stems or len(stem) < 4:
            continue
        used_stems.add(stem)
        f["chosen_path"] = p
        picked.append(f)
        got += 1
    print(f"{label}: {got}")

n_conf = len(by_conf.get("user_confirmed", []))
take(by_conf.get("user_confirmed", []), n_conf, "user_confirmed positives")
take(by_conf.get("librarian_inferred", []), 30 - len(picked), "librarian_inferred positives")
print("total positives facts:", len(picked),
      dict(collections.Counter(f["source_type"] for f in picked)))

probes = []
for f in picked:
    p = f["chosen_path"]
    probes.append({"text": p, "expect": [f["id"]], "_kind": "path"})
    # command shape: a real command mention from the body, else cat/read of the path
    cmds = [c for c in CMD_RE.findall(f["body"]) if " " in c and len(c) < 70
            and not c.startswith("/") and "..." not in c]
    if cmds:
        probes.append({"text": cmds[0], "expect": [f["id"]], "_kind": "command"})
    else:
        probes.append({"text": f"cat {p}", "expect": [f["id"]], "_kind": "command"})
    # pattern shape: the distinctive stem
    stem = p.rsplit("/", 1)[-1].rsplit(".", 1)[0].replace("-", " ")
    probes.append({"text": stem, "expect": [f["id"]], "_kind": "pattern"})

# ---- near-miss negatives (expect []) ----
neg = []
# (a) real files in the CT repo not mentioned in any fact body
out = subprocess.run(
    ["find", ".", "-type", "f",
     "(", "-name", "*.md", "-o", "-name", "*.rs", "-o", "-name", "*.ts",
     "-o", "-name", "*.tsx", "-o", "-name", "*.json", "-o", "-name", "*.sh",
     "-o", "-name", "*.toml", "-o", "-name", "*.yml", ")",
     "-not", "-path", "./target/*", "-not", "-path", "./.git/*",
     "-not", "-path", "./node_modules/*", "-not", "-path", "./.cargo-shared-target/*"],
    cwd=REPO, capture_output=True, text=True)
if out.returncode != 0:
    raise SystemExit(f"find failed (exit {out.returncode}): {out.stderr}")
repo_paths = sorted({p[2:] for p in out.stdout.splitlines()})
random.shuffle(repo_paths)
for p in repo_paths:
    if len(neg) >= 60:
        break
    stem = p.rsplit("/", 1)[-1]
    if stem.lower() in bodies_all.lower() or p in bodies_all:
        continue
    neg.append({"text": p, "_kind": "neg-path"})
# (b) perturbed vault-ish paths (swap basenames)
real = [f["chosen_path"] for f in picked]
dirs = sorted({p.rsplit("/", 1)[0] for p in real if "/" in p})
stems = sorted({p.rsplit("/", 1)[-1] for p in real})
i = 0
while len(neg) < 110 and i < 400:
    i += 1
    d = random.choice(dirs) if dirs else "docs"
    s = random.choice(stems)
    cand = f"{d}/{s}-v2.md" if not s.endswith(".md") else f"{d}/{s}.bak"
    if cand in bodies_all or any(cand == p["text"] for p in neg + probes):
        continue
    neg.append({"text": cand, "_kind": "neg-pattern"})
# (c) plausible-but-wrong commands
verbs = ["ct wisdom match", "cargo build", "git push", "vault_write_note", "gh pr view"]
objs = [s.replace(".md", "").replace(".sh", "") for s in stems]
while len(neg) < 125:
    neg.append({"text": f"{random.choice(verbs)} {random.choice(objs)}-draft", "_kind": "neg-command"})

kind_counts = collections.Counter(p["_kind"] for p in probes)
mix = collections.Counter(f["source_type"] for f in picked)
neg_counts = collections.Counter(p["_kind"] for p in neg)
print("positive shapes:", dict(kind_counts), "negatives:", dict(neg_counts), "total probes:", len(probes) + len(neg))

with open(f"{OUT}/activity-mixed-probes.jsonl", "w") as fh:
    for p in probes + neg:
        fh.write(json.dumps({"text": p["text"], "expect": p.get("expect", [])}) + "\n")
with open(f"{OUT}/activity-mixed-facts.jsonl", "w") as fh:
    for f in picked:
        fh.write(json.dumps({k: f[k] for k in ("id", "source_type", "title", "body")}) + "\n")
mixreport = {
    "generated": "2026-11-13",
    "brain": "scratch copy brain-dir-20261113/brain.db (sqlite .backup of live ~/.brain/brain.db, read-only)",
    "positive_facts": dict(mix),
    "positive_shapes": dict(kind_counts),
    "negative_kinds": dict(neg_counts),
    "probes_total": len(probes) + len(neg),
    "note": "mixed-provenance pool per issue #344 (no user_stated-only restriction)",
}
with open(f"{OUT}/provenance-mix.json", "w") as fh:
    json.dump(mixreport, fh, indent=2)
print("wrote", OUT)
