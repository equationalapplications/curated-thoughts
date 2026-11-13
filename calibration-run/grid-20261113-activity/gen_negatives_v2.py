#!/usr/bin/env python3
"""Regenerate ONLY the negatives for the #344 mixed-provenance probe set.

v2: the v1 perturbation negatives reused the positives' own path stems, so
they sat inside the brain's topic space and opened the gate everywhere
(FP 87/125 @ 0.65). v2 follows the 2026-10-18 record: near-miss negatives are
real files from the CT repo that are NOT mentioned anywhere in the brain
corpus, plus command-shaped variants of them.
"""
import json, sqlite3, subprocess, random, collections, os

DB = "/home/kv-thinkpad-t420-ubuntu/ct-calibration-scratch/brain-dir-20261113/brain.db"
REPO = "/mnt/kv-work/build/worktrees/curated-thoughts/rule10-delivery"
OUT = os.path.dirname(os.path.abspath(__file__))

conn = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
bodies = "\n".join(r[0] for r in conn.execute(
    "SELECT body FROM llm_wiki_entries WHERE deleted_at IS NULL").fetchall())

allp = []
for ext in ("*.md", "*.rs", "*.ts", "*.tsx", "*.json", "*.sh", "*.toml", "*.yml"):
    allp += subprocess.run(
        ["find", ".", "-type", "f", "-name", ext,
         "-not", "-path", "./target/*", "-not", "-path", "./.git/*",
         "-not", "-path", "./node_modules/*", "-not", "-path", "./.cargo-shared-target/*"],
        cwd=REPO, capture_output=True, text=True).stdout.splitlines()
repo_paths = sorted({p[2:] for p in allp})

random.seed(344)
random.shuffle(repo_paths)
neg = []
seen_stems = set()
for p in repo_paths:
    if len(neg) >= 100:
        break
    base = p.rsplit("/", 1)[-1]
    if base.lower() in seen_stems:
        continue
    if p in bodies or base in bodies:
        continue  # mentioned anywhere in the corpus -> not a negative
    seen_stems.add(base.lower())
    neg.append({"text": p, "_kind": "neg-path"})

# command-shaped negatives over the same unrelated files
paths_n = [n["text"] for n in neg if n["_kind"] == "neg-path"]
verbs = ["cat", "less", "nvim", "rg TODO", "git log -p"]
i = 0
while len(neg) < 125 and i < 500:
    i += 1
    p = random.choice(paths_n)
    t = f"{random.choice(verbs)} {p}"
    if any(t == x["text"] for x in neg):
        continue
    neg.append({"text": t, "_kind": "neg-command"})

print("negatives:", len(neg), dict(collections.Counter(n["_kind"] for n in neg)))
print("sample:", [n["text"] for n in neg[:5]])

# splice into the existing probe file (keep v2 positives)
old = [json.loads(l) for l in open(f"{OUT}/activity-mixed-probes.jsonl")]
pos = [p for p in old if p["expect"]]
with open(f"{OUT}/activity-mixed-probes.jsonl", "w") as fh:
    for p in pos + neg:
        fh.write(json.dumps({"text": p["text"], "expect": p.get("expect", [])}) + "\n")

mix = json.load(open(f"{OUT}/provenance-mix.json"))
mix["negative_kinds"] = dict(collections.Counter(n["_kind"] for n in neg))
mix["probes_total"] = len(pos) + len(neg)
mix["negative_generation"] = "v2: real CT-repo files (and command variants) absent from the entire brain corpus, per the 2026-10-18 record"
json.dump(mix, open(f"{OUT}/provenance-mix.json", "w"), indent=2)
print("wrote", len(pos) + len(neg), "probes")
