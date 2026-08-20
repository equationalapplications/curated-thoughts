---
type: fact
title: Fact with full provenance
timestamp: 2026-08-01T00:00:00.000Z
id: fact_provenance
entity_id: demo
confidence: certain
source_type: user_stated
created_at: 1719835200000
status: stable
stale_after: 2027-01-01
generated: { by: "human:alice", at: 2026-08-01T00:00:00.000Z }
verified: [ { by: "process:nightly", at: 2026-08-02T00:00:00.000Z } ]
usage_window: { from: "2026-08-01", to: "2026-12-31" }
sources: [ { id: "src-1", resource: "documents/notes.md", title: "Notes", author: "human:alice", usage_count: 3, last_modified: "2026-07-30" } ]
---
Body with footnote attribution[^src-1].

[^src-1]: See documents/notes.md
