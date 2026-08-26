## [1.31.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.30.0...v1.31.0) (2026-08-26)

### Features

* **tools:** add --foreground flag to ct watch ([d4b8069](https://github.com/equationalapplications/curated-thoughts/commit/d4b8069b58874241b6d9284a4b09c9fc9cd6abbb)), closes [#96](https://github.com/equationalapplications/curated-thoughts/issues/96)
* **tools:** add ct watch subcommand ([5ce3268](https://github.com/equationalapplications/curated-thoughts/commit/5ce32689e5b64f9aca69b0d040fe3fece1bffe1e))
* **watch:** add VaultLock to src-tauri watcher and duplicate in tools (phase 2 v2) ([7c35b11](https://github.com/equationalapplications/curated-thoughts/commit/7c35b117bfd29a89f8d09e7d128e2c83f531701e))

### Bug Fixes

* **tauri:** acquire VaultLock on brain_dir, not vault path ([6258cfd](https://github.com/equationalapplications/curated-thoughts/commit/6258cfd2b98e3dc2b0a42107fe44cc9bfed717e8))
* **tauri:** swap in-process pipeline for DB-backed enqueue_vault_event ([2ed0acf](https://github.com/equationalapplications/curated-thoughts/commit/2ed0acfb4b29a3993d3b05cc23e9a2bfaacf346b))
* **tools:** add watcher absolute-paths test + fix VaultLock name shadowing ([8f49511](https://github.com/equationalapplications/curated-thoughts/commit/8f495115362c99049a1a5966bd90f705ca7d4857))
* **tools:** address CodeRabbit review on PR [#96](https://github.com/equationalapplications/curated-thoughts/issues/96) ([c1c54cb](https://github.com/equationalapplications/curated-thoughts/commit/c1c54cbe717e2845c8fef59eac334fbebb4ea773))
* **tools:** address CodeRabbit review pass 2 on PR [#96](https://github.com/equationalapplications/curated-thoughts/issues/96) ([a1c33b3](https://github.com/equationalapplications/curated-thoughts/commit/a1c33b3d235fb7a0b511c5746cf15453a598abd4))
* **tools:** address CodeRabbit review pass 3 on PR [#96](https://github.com/equationalapplications/curated-thoughts/issues/96) ([5c22235](https://github.com/equationalapplications/curated-thoughts/commit/5c22235a2fcd37bf1aafa60f132219c166be1f93)), closes [#12](https://github.com/equationalapplications/curated-thoughts/issues/12)
* **tools:** correct paths.rs vault_contains doc-comment line ref + trailing newlines ([e93c283](https://github.com/equationalapplications/curated-thoughts/commit/e93c2836b2dbc1d0bf99be64fba9624702aa5d5f))
* **tools:** ct watch JSON to stdout, kind field, add ts_ms ([fca4e77](https://github.com/equationalapplications/curated-thoughts/commit/fca4e779b2b305b02c34a5ee346410fea6f0ab60)), closes [#96](https://github.com/equationalapplications/curated-thoughts/issues/96)
* **tools:** map DB errors to exit 3, notify init errors to exit 4 ([9b5eb7b](https://github.com/equationalapplications/curated-thoughts/commit/9b5eb7bb8be692bb2d3b815800dcc73f113627b8)), closes [#96](https://github.com/equationalapplications/curated-thoughts/issues/96)

## [1.30.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.29.0...v1.30.0) (2026-08-25)

### Features

* **db:** add MIGRATION_V11 synthesis watermark columns with backfill ([eaa5495](https://github.com/equationalapplications/curated-thoughts/commit/eaa5495e3774473565d292a68c50992778829584))
* **db:** dedupe fact_add proposals by normalized exact body match ([1ce2e63](https://github.com/equationalapplications/curated-thoughts/commit/1ce2e63e4d55e7b9e43ca461c4927359babab5d1))
* **librarian:** hash-gated synthesis watermark, dirty-doc selection, --force ([b4f14b2](https://github.com/equationalapplications/curated-thoughts/commit/b4f14b207450304e07d3fad6c745438ae4680cf7))
* **librarian:** merge observability branch + hand-resolve merge conflicts (phase 1 intergration) ([d91f7a5](https://github.com/equationalapplications/curated-thoughts/commit/d91f7a5c90f3fc36e689a12eb966bbc821a0a330))
* **librarian:** stderr per-doc progress, error surfacing, configurable timeout_secs, run summary ([104fd4e](https://github.com/equationalapplications/curated-thoughts/commit/104fd4e4410a5f8c0f69a465e6bf406f378d5ac8))

### Bug Fixes

* **ci:** repair tests + CodeRabbit findings on phase-1 watermark ([f8a02cc](https://github.com/equationalapplications/curated-thoughts/commit/f8a02cc2cdf272d2ff429decc44f05dff2f52919))
* **db:** anchor duplicate-count test by event_type to avoid timestamp tie ([4c6ecf4](https://github.com/equationalapplications/curated-thoughts/commit/4c6ecf45e6e223120c8800caad90a5573a275336)), closes [#84](https://github.com/equationalapplications/curated-thoughts/issues/84)
* **db:** treat NULL synth_model as dirty in watermark gate ([6e00531](https://github.com/equationalapplications/curated-thoughts/commit/6e00531a0289ba10d2587289dca82cbbf32e4740)), closes [#84](https://github.com/equationalapplications/curated-thoughts/issues/84)
* **librarian:** revert drive-by cargo fmt artifact in embedder/mod.rs ([a1620c8](https://github.com/equationalapplications/curated-thoughts/commit/a1620c8c3205cee1f3443599f01db79194c6e003))

## [1.29.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.28.1...v1.29.0) (2026-08-25)

### Features

* **tools:** add ct dispatcher with status subcommand, --json and exit-code contract ([5f5c1ad](https://github.com/equationalapplications/curated-thoughts/commit/5f5c1ada0bbc21a4abc1278e53bbffb4570948ed))
* **tools:** add tools lib shell with cli_common path resolution ([ab7498a](https://github.com/equationalapplications/curated-thoughts/commit/ab7498a8605b7b00814a7660edd1936f9266db75))
* **tools:** ct graph traversal and ct wiki get/list ([0589491](https://github.com/equationalapplications/curated-thoughts/commit/0589491bf2da009a201026c613dd5cae3b0521e9))
* **tools:** ct proposals list/show for headless proposal inspection ([78da780](https://github.com/equationalapplications/curated-thoughts/commit/78da7808374df7d3a4e09ebdf300f24547986347))
* **tools:** ct search/recall/code over shared query helpers ([02e4643](https://github.com/equationalapplications/curated-thoughts/commit/02e4643713080db07843071f735ffece2593fa0b))

### Bug Fixes

* **tools:** address CodeRabbit review round on ct CLI ([a39a860](https://github.com/equationalapplications/curated-thoughts/commit/a39a860ed2958c0c039927132230e5c6e2ff4e4b))
* **tools:** clippy clean under -D warnings (needless Ok/?, while-let, unused mut) ([b3c2a47](https://github.com/equationalapplications/curated-thoughts/commit/b3c2a47ee5d55022a3bf4a43bcb66afc35ab7531))

## [1.28.1](https://github.com/equationalapplications/curated-thoughts/compare/v1.28.0...v1.28.1) (2026-08-25)

### Bug Fixes

* **mcp:** read llm_wiki_entries.updated_at as integer in recall_context ([3bd6298](https://github.com/equationalapplications/curated-thoughts/commit/3bd62980a20932384f49e33c38fb58d6f7e96617)), closes [#78](https://github.com/equationalapplications/curated-thoughts/issues/78)

## [1.28.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.27.2...v1.28.0) (2026-08-25)

### Features

* **tools:** add approve_pending_proposals one-off bin ([f90c84f](https://github.com/equationalapplications/curated-thoughts/commit/f90c84f460970557abd039a9afda2c83caafe8ba))

## [1.27.2](https://github.com/equationalapplications/curated-thoughts/compare/v1.27.1...v1.27.2) (2026-08-25)

### Bug Fixes

* **librarian:** raise LLM completer timeout and cap reasoning effort ([ddbf8c5](https://github.com/equationalapplications/curated-thoughts/commit/ddbf8c53cf45da806485197f910e446d8841b931))

## [1.27.1](https://github.com/equationalapplications/curated-thoughts/compare/v1.27.0...v1.27.1) (2026-08-24)

### Bug Fixes

* **mcp:** aggregate+rank wiki candidates across all query terms; TEXT column types ([d2b2ca9](https://github.com/equationalapplications/curated-thoughts/commit/d2b2ca9e1f3a211afac01f38b92a38076b75aedc))
* **mcp:** harden wiki-entry NULL handling per review gate ([badeaec](https://github.com/equationalapplications/curated-thoughts/commit/badeaec3d4d722cdf51e045eaa50b0df95954317))
* **mcp:** rewrite recall_context/search_code/get_wiki_entry SQL to real schema ([ee1d08c](https://github.com/equationalapplications/curated-thoughts/commit/ee1d08c6c25cc5166c514f817398817b654f71dd))

## [1.27.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.26.0...v1.27.0) (2026-08-24)

### Features

* **mcp:** graph_neighbors tool — walk call/import graph without SQL ([dd2de9f](https://github.com/equationalapplications/curated-thoughts/commit/dd2de9f5e374a4cfb94bdca554a247e23e1649d7))

### Bug Fixes

* **mcp:** address CodeRabbit review — validate direction, cap before enrichment ([52503e4](https://github.com/equationalapplications/curated-thoughts/commit/52503e49838211cea01a8f4641a494feff6f572d))
* **mcp:** graph_neighbors review fixes — resolve defs, normalize case, cap results ([cd83a7b](https://github.com/equationalapplications/curated-thoughts/commit/cd83a7bf8d62fdf5746b87bd9d1ecc7c5e73a59a))

## [1.26.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.25.3...v1.26.0) (2026-08-24)

### Features

* **db:** preserve per-doc ingest history (ingest_runs, V10) ([68701d4](https://github.com/equationalapplications/curated-thoughts/commit/68701d4d3272ca93159f1de1915f6e41886505f7))

### Bug Fixes

* **db:** bump schema-version test to 10; use MIGRATION_V10 import ([4535426](https://github.com/equationalapplications/curated-thoughts/commit/4535426380a5e9fed39113670335486de83e8eca))

## [1.25.3](https://github.com/equationalapplications/curated-thoughts/compare/v1.25.2...v1.25.3) (2026-08-24)

### Bug Fixes

* **mcp:** add busy_timeout to agent-log connection ([93393f2](https://github.com/equationalapplications/curated-thoughts/commit/93393f2a44be838aa066b3894d5e7e3151122ac2))
* **mcp:** wire agent-access logging into sidecar tool handlers ([47877dd](https://github.com/equationalapplications/curated-thoughts/commit/47877dd29aedd8a4fa54fd0e7ff11bcd94b7a7e0))

## [1.25.2](https://github.com/equationalapplications/curated-thoughts/compare/v1.25.1...v1.25.2) (2026-08-24)

### Bug Fixes

* **ingest:** exclude machine-generated files instead of failing on them ([c452983](https://github.com/equationalapplications/curated-thoughts/commit/c4529831bc59b1a1e697f6013a6df68093530601))

## [1.25.1](https://github.com/equationalapplications/curated-thoughts/compare/v1.25.0...v1.25.1) (2026-08-24)

### Bug Fixes

* **ingest:** restrict symlink follow to documents/ children; count traversal errors; tighten multibyte regression test ([cccd349](https://github.com/equationalapplications/curated-thoughts/commit/cccd349d4748606ea8bbca1ace6b7028e7f3f050))

## [1.25.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.24.2...v1.25.0) (2026-08-24)

### Features

* **embedder:** add External profile for OpenAI-compatible embedding endpoints ([b7619e7](https://github.com/equationalapplications/curated-thoughts/commit/b7619e7ec4412710751dfd67d1d2bf1956ee984f))

### Bug Fixes

* **embedder:** restore exhaustive EmbedProfile match arm lost in fmt commit ([6caf82a](https://github.com/equationalapplications/curated-thoughts/commit/6caf82afe8db49c2b00607aa5ea10a5925fc135c))
* **embedder:** serde default for external base_url; no_proxy on external client ([2f28e5f](https://github.com/equationalapplications/curated-thoughts/commit/2f28e5fa929b92f61b6885e3c0b7f803c9b68b1a))
* **embedder:** strict response validation; never persist external api_key; spec default model correction ([af6ff55](https://github.com/equationalapplications/curated-thoughts/commit/af6ff55dd594ad3032a03508b9e776e107347633))

## [1.24.2](https://github.com/equationalapplications/curated-thoughts/compare/v1.24.1...v1.24.2) (2026-08-24)

### Bug Fixes

* **embedder:** replace redaction-placeholder default Ollama base URL with localhost ([02f1806](https://github.com/equationalapplications/curated-thoughts/commit/02f1806ed79c254d4d37542bc8a38ecc8b7b5d0d))

## [1.24.1](https://github.com/equationalapplications/curated-thoughts/compare/v1.24.0...v1.24.1) (2026-08-23)

### Bug Fixes

* **embed:** guard embed inputs against model context length ([#69](https://github.com/equationalapplications/curated-thoughts/issues/69)) ([a7ecc23](https://github.com/equationalapplications/curated-thoughts/commit/a7ecc23fb83585fc69dcc8d55b672e2f70f08f8d))

## [1.24.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.23.1...v1.24.0) (2026-08-23)

### Features

* ship MCP server as bundled sidecar ([#67](https://github.com/equationalapplications/curated-thoughts/issues/67)) ([5eab8fa](https://github.com/equationalapplications/curated-thoughts/commit/5eab8fa962d97672134a39519453c51bb9d0262a))

## [1.23.1](https://github.com/equationalapplications/curated-thoughts/compare/v1.23.0...v1.23.1) (2026-08-23)

### Bug Fixes

* **config:** tolerate unknown embed_profile variants in config.json ([#68](https://github.com/equationalapplications/curated-thoughts/issues/68)) ([b3de0b9](https://github.com/equationalapplications/curated-thoughts/commit/b3de0b9d46d383c96976e002218b62244e30917f))

## [1.23.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.22.0...v1.23.0) (2026-08-22)

### Features

* add Curated Thoughts MCP tools for context recall and code search ([8c4d40e](https://github.com/equationalapplications/curated-thoughts/commit/8c4d40e62bb65c8afed069708de075e9dbab61b3))
* add curated_superpowers_setup tool and Superpowers skill file ([922f4ad](https://github.com/equationalapplications/curated-thoughts/commit/922f4adccb69e240deabd59025dfae32f46aa576))

### Bug Fixes

* **tools:** compile curated_thoughts_mcp against rmcp 3.x / rusqlite 0.32 (params_from_iter, clone before move) ([ee661cf](https://github.com/equationalapplications/curated-thoughts/commit/ee661cfc4e9bf9d97722b3a8596e971531b558f5))

## [1.22.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.21.2...v1.22.0) (2026-08-22)

### Features

* **tools:** add ingest_vault_once for headless initial vault ingestion ([#66](https://github.com/equationalapplications/curated-thoughts/issues/66)) ([d0fe992](https://github.com/equationalapplications/curated-thoughts/commit/d0fe992b02acf7883168c39e7cc5c974c4996083))

## [1.21.2](https://github.com/equationalapplications/curated-thoughts/compare/v1.21.1...v1.21.2) (2026-08-22)

### Bug Fixes

* **embedder:** disable proxy resolution and raise request timeout ([#64](https://github.com/equationalapplications/curated-thoughts/issues/64)) ([98be2af](https://github.com/equationalapplications/curated-thoughts/commit/98be2afda8cccc948acce5a901e6321574b627a1))

## [1.21.1](https://github.com/equationalapplications/curated-thoughts/compare/v1.21.0...v1.21.1) (2026-08-22)

### Bug Fixes

* **deps:** remediate 30 of 31 Dependabot alerts + supply-chain hardening ([#45](https://github.com/equationalapplications/curated-thoughts/issues/45)) ([bbc3122](https://github.com/equationalapplications/curated-thoughts/commit/bbc31221f7ee70bbb7e9b170dcc2149dba9b467c))

## [1.21.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.20.0...v1.21.0) (2026-08-22)

### Features

* **shell:** global ⌘K command palette (Phase 8 Plan C) ([#46](https://github.com/equationalapplications/curated-thoughts/issues/46)) ([90f60ca](https://github.com/equationalapplications/curated-thoughts/commit/90f60ca28a1437a5ef1b52bda7a4a1ec58933d2c))

## [1.20.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.19.0...v1.20.0) (2026-08-22)

### Features

* **brain:** alt+click dispatch rule on fact source chips ([187a034](https://github.com/equationalapplications/curated-thoughts/commit/187a034b164e1412e11420cb009bd0a90e28dbe7))
* **brain:** thread onPeekSource through EntityPage and BrainMode ([800fca2](https://github.com/equationalapplications/curated-thoughts/commit/800fca2afa57778da805f2b090aaf0b2c31d24b8))
* **chunks:** fetch_chunk_content command returning chunk text by (path, hash) ([7055a43](https://github.com/equationalapplications/curated-thoughts/commit/7055a43d839445d0a760e7df3f533b954be39358))
* **chunks:** find_chunk_text query for peek panels ([c4ee6f1](https://github.com/equationalapplications/curated-thoughts/commit/c4ee6f140bc5f3e79290c1dc9ee4e6b464297ed2))
* **shell:** AppShell owns peekTarget state and promotion navigation ([6d1edac](https://github.com/equationalapplications/curated-thoughts/commit/6d1edac9b3087593f7a024fb6874d08bc9fa763e))
* **shell:** PeekPanel slide-over with focus trap and chunk-slice body states ([8388d24](https://github.com/equationalapplications/curated-thoughts/commit/8388d2455fb58a4a3c12ee4c90556d5184ea3b71))

### Bug Fixes

* **phase-8-plan-b:** register overlay command under its frontend wire name ([902eeb8](https://github.com/equationalapplications/curated-thoughts/commit/902eeb8bdb6f41cb94d5f18a5ce2ef7b7e072386))
* **shell:** clear peek target on vault switch; drop deprecated word-break ([a718add](https://github.com/equationalapplications/curated-thoughts/commit/a718add2046a57139cc9ad715a8216e04b7877ed))

## [1.19.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.18.0...v1.19.0) (2026-08-21)

### Features

* **chunks:** resolve_chunk_overlay Tauri command + frontend binding ([9bddf74](https://github.com/equationalapplications/curated-thoughts/commit/9bddf74fdd8b4378e4e573c0485b6c3f56658c2c))
* **commit:** write content_hash on every evidence commit ([c30f141](https://github.com/equationalapplications/curated-thoughts/commit/c30f141458c1beed4dc5b9733bbc7dccab8ba074))
* **db:** add compute_chunk_hash for stable content-derived ids ([32fa105](https://github.com/equationalapplications/curated-thoughts/commit/32fa105de7c09a82dacbd5b98f2986aac495771a))
* **db:** bulk content_hash migration (idempotent, transactional, progress events) ([806e27a](https://github.com/equationalapplications/curated-thoughts/commit/806e27a9943fffd0b91f2f96456ae230b09b9ee8))
* **db:** bulk content_hash migration preserves embeddings + relationships ([b11043c](https://github.com/equationalapplications/curated-thoughts/commit/b11043cc9b9d5fdbe4fb7ace51731a28f0f07a56))
* **db:** schema V9 adds chunks.content_hash + pipeline computes it on ingest ([064c4d7](https://github.com/equationalapplications/curated-thoughts/commit/064c4d727fa76114f39b9f796e3595167f7e88d2))
* **editor:** hash-based line-range overlay (replaces heading-text match) ([4ca1d48](https://github.com/equationalapplications/curated-thoughts/commit/4ca1d4863b4ca705fb614acafaa97dd12256bba9))
* **editor:** inject line metadata into BlockNote + cached line-to-block map ([1f703a0](https://github.com/equationalapplications/curated-thoughts/commit/1f703a09d7159f81227ad6c7965807f79af69685))
* **entities:** source_docs join on content_hash (stable chunk id) ([edc841a](https://github.com/equationalapplications/curated-thoughts/commit/edc841a159e71f1062c756989725dce5375a2598))
* **startup:** run chunk_hash migration at first start, emit progress events ([10372e8](https://github.com/equationalapplications/curated-thoughts/commit/10372e8e11b38458ef051d38a9cb922ab0e0c99b))
* **ui:** SplashScreen + AppShell mount for chunk-hash migration ([c2f7cb2](https://github.com/equationalapplications/curated-thoughts/commit/c2f7cb22f5865a967ef997b9d5e3f4306844ec68))

### Bug Fixes

* **editor:** hide source-moved notice when × clicked ([1daa011](https://github.com/equationalapplications/curated-thoughts/commit/1daa011b30753f29bd69f2830ef76dd80e4a235d))
* **editor:** wire auto-dismiss after 1.5s for chunk overlay ([e2ba5c7](https://github.com/equationalapplications/curated-thoughts/commit/e2ba5c72c7a8f2b5ec1098aacf95f730f1d095a5))
* **phase-9:** address 3 post-review findings before merge ([c1e1735](https://github.com/equationalapplications/curated-thoughts/commit/c1e1735c51c86c1924c9bd2a9d3afe93d24e836a))
* **phase-9:** address 4 review findings before PR ([fae40bf](https://github.com/equationalapplications/curated-thoughts/commit/fae40bfe89a9dbd6f43e023d712508fdda96f671)), closes [#1](https://github.com/equationalapplications/curated-thoughts/issues/1) [#2](https://github.com/equationalapplications/curated-thoughts/issues/2) [#3](https://github.com/equationalapplications/curated-thoughts/issues/3) [#4](https://github.com/equationalapplications/curated-thoughts/issues/4)
* **phase-9:** address CodeRabbit review feedback before PR ([42729c5](https://github.com/equationalapplications/curated-thoughts/commit/42729c50e79bd309e450182fe14d32dc5cc82a84)), closes [#43](https://github.com/equationalapplications/curated-thoughts/issues/43)

## [1.18.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.17.1...v1.18.0) (2026-08-20)

### Features

* **brain:** pass source_docs chunk ids through fact source chips ([b07afca](https://github.com/equationalapplications/curated-thoughts/commit/b07afcac6a8a7d5853caa68c726574be77df6c6e))
* **entities:** surface chunk ids in source_docs_from_ref ([a2ca99e](https://github.com/equationalapplications/curated-thoughts/commit/a2ca99ed6567f449db8388055909560764cec814))

## [1.17.1](https://github.com/equationalapplications/curated-thoughts/compare/v1.17.0...v1.17.1) (2026-08-20)

### Bug Fixes

* **brain:** suppress empty Connections sections when embedder is unavailable ([#38](https://github.com/equationalapplications/curated-thoughts/issues/38)) ([0e6833a](https://github.com/equationalapplications/curated-thoughts/commit/0e6833a59e9d57c8d7c2d18f796881b22213a401))

## [1.17.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.16.0...v1.17.0) (2026-08-19)

### Features

* **brain:** [[Entity]] autocomplete in summary editor ([4ae049b](https://github.com/equationalapplications/curated-thoughts/commit/4ae049bbd42826e6a9ac0a45b0efc5c92889dd5e))
* **brain:** entity sort picker (recently updated / name / created) ([e54c57f](https://github.com/equationalapplications/curated-thoughts/commit/e54c57f8e1cad91807d9f7a7efd2996e54805c55))
* **brain:** expose OKF v0.2 fields on EntityFact ([8c30907](https://github.com/equationalapplications/curated-thoughts/commit/8c3090706c235dc907eece8c4604b224fae52ff9))
* **brain:** per-fact '…' power-layer menu with OKF v0.2 provenance ([f16eabf](https://github.com/equationalapplications/curated-thoughts/commit/f16eabf1d2aac2c982a26cbeb42cb0aaf9158e1b))
* **brain:** WikilinkText resolves names and styles broken links ([e611db6](https://github.com/equationalapplications/curated-thoughts/commit/e611db6e7f9daee8d708ce32b3dbb0838aab0a5b))
* **health:** inline feature notice when embedder or generation is down ([8e43a0b](https://github.com/equationalapplications/curated-thoughts/commit/8e43a0b7416c51944a89eb8f9ea3ffe561e8650f))
* **library:** chunk-level deep-link highlight ([2b715f7](https://github.com/equationalapplications/curated-thoughts/commit/2b715f7c135622359e6d7971739edcc0bf096aa0))
* **modes:** per-mode empty states (Brain, Library, Review) ([3b86ddd](https://github.com/equationalapplications/curated-thoughts/commit/3b86dddaf192a9a0f34ae4f63ffc326420e34a10))
* **okf:** default export to llm-wiki/2 + okf_version 0.2 ([bf2b81a](https://github.com/equationalapplications/curated-thoughts/commit/bf2b81a8e06633fb82042a34908b7fed3c3f8dfc))
* **okf:** emit v0.2 frontmatter from fact_file / task_file ([de35deb](https://github.com/equationalapplications/curated-thoughts/commit/de35debbce8c3c97a0d2802c9ae59ddd130b9756))
* **okf:** extend WikiFact/WikiTask typed models with v0.2 fields ([ab67c28](https://github.com/equationalapplications/curated-thoughts/commit/ab67c28776f7bdad586e1c6c07e21252e2ac4ebe))
* **okf:** flow-mapping frontmatter parser for v0.2 emit/read ([e9f30df](https://github.com/equationalapplications/curated-thoughts/commit/e9f30df8bebd397e1cf84dc72bf9c28b9a7afbe5))
* **okf:** v0.2 read path + status-rename rule + persist v0.2 columns on import ([74ab50d](https://github.com/equationalapplications/curated-thoughts/commit/74ab50d9c37d895ccb238d7c161d55f619fb23ca))
* **styles:** power menu, sort picker, provider notice, anchor highlight ([754306f](https://github.com/equationalapplications/curated-thoughts/commit/754306ff993c7e05503c5bce186d7044d803010c))

### Bug Fixes

* **brain:** dedup listEntities round-trips in WikilinkText resolver ([5f4411e](https://github.com/equationalapplications/curated-thoughts/commit/5f4411e8258c3983b2dad48d503de13de61858cc))
* **brain:** handle listEntities rejection silently in WikilinkText ([e28b233](https://github.com/equationalapplications/curated-thoughts/commit/e28b2331d88db6e7c188908a9de9f2aa14fb7123))
* **brain:** keep WikilinkText resolver in sync with entity list changes ([3aeb845](https://github.com/equationalapplications/curated-thoughts/commit/3aeb84598cc1bf7f60c5cfa7e272011904436d83))
* **brain:** render SuggestionMenuController as child of BlockNoteView ([78b336d](https://github.com/equationalapplications/curated-thoughts/commit/78b336df8b634eb85dc306c223d024a3193a4c9d))
* **brain:** return Promise<void> from ensureResolver to satisfy discriminated-union narrowing ([c657fe0](https://github.com/equationalapplications/curated-thoughts/commit/c657fe0c7ea48f9071b2cea9522752193e22d348))
* **ci:** unstick apt-get update on Ubuntu 22.04 runners ([d6bd9b2](https://github.com/equationalapplications/curated-thoughts/commit/d6bd9b2fb36ebb630aa7a03b3183e2eccc29261a))
* **ci:** wrap apt-get update with process-level 5m timeout ([f8fe40e](https://github.com/equationalapplications/curated-thoughts/commit/f8fe40ecaa6f5a6cc69d29a76371dc2a63288c63))
* **health:** label BrainMode feature correctly + gate ConnectionsPanel ([aaded88](https://github.com/equationalapplications/curated-thoughts/commit/aaded881fafbf134a04f185d765dffb74032f025))
* **okf:** capture URLs regardless of protocol casing in # Citations fallback ([c971570](https://github.com/equationalapplications/curated-thoughts/commit/c97157091bc5747563f76de5ae8879d949352bdf))
* **okf:** convert YAML flow text to JSON in v0.2 columns ([92fbab5](https://github.com/equationalapplications/curated-thoughts/commit/92fbab5114f824fa407c216f0af80ad6ae8620ca))
* **okf:** gate LLM_WIKI_PROFILE_V2 import to test builds ([efa1644](https://github.com/equationalapplications/curated-thoughts/commit/efa164404f2efca72072df64194d7327a50bfefc))
* **okf:** profile-aware writer so v0.1 export is genuinely v0.1-shaped ([0eba0c6](https://github.com/equationalapplications/curated-thoughts/commit/0eba0c69bcb20592998929e1b3b389d3d8512e95))
* **okf:** read v0.2 columns on bundle import; update stale phase-7 tests ([8ef1dc5](https://github.com/equationalapplications/curated-thoughts/commit/8ef1dc50de2c1d803c4aba94c1eb7fc6cc49f33a))
* **okf:** require valid lifecycle status when classifying tasks as v0.2 ([b1ea5dc](https://github.com/equationalapplications/curated-thoughts/commit/b1ea5dc6593f58f91c87b827e19588170ad4cc33))
* **okf:** share verified_at parsers between fact and task files ([bd0a0f5](https://github.com/equationalapplications/curated-thoughts/commit/bd0a0f5ed1e7548ff432b44768e21947646bc15f))
* **review:** address Phase 7 fifth review pass ([3088829](https://github.com/equationalapplications/curated-thoughts/commit/308882991a566946ad517f932b3949dbd660ceb1))
* **review:** address Phase 7 fourth CodeRabbit pass ([a89dc5d](https://github.com/equationalapplications/curated-thoughts/commit/a89dc5d15fefa9ba6cc01dc65c394de73e88a0db))
* **review:** address Phase 7 Plan A+B CodeRabbit review follow-ups ([e20b97d](https://github.com/equationalapplications/curated-thoughts/commit/e20b97ddccba1c0013acf5d51e7e1532d079a095)), closes [#34](https://github.com/equationalapplications/curated-thoughts/issues/34)
* **review:** address Phase 7 second CodeRabbit pass ([a9f4dac](https://github.com/equationalapplications/curated-thoughts/commit/a9f4dacf0f3878513502377db70ee589b1ececd1))
* **review:** address Phase 7 third CodeRabbit pass ([87452df](https://github.com/equationalapplications/curated-thoughts/commit/87452df101437b38a3a1e5237b542c05ea036e0d))
* **shell:** split EditorPane doc-load from anchor-highlight ([fb260c4](https://github.com/equationalapplications/curated-thoughts/commit/fb260c486d2a8b550cf25d4d92f322f27323a771))

## [1.16.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.15.0...v1.16.0) (2026-08-18)

### Features

* add activity feed panel and mode rail with error handling ([863dba3](https://github.com/equationalapplications/curated-thoughts/commit/863dba3fd8bce921515645a024c20356ea9204bb))
* **audit:** log MCP/bridge tool calls to curated_agent_log with 90-day pruning ([73406d0](https://github.com/equationalapplications/curated-thoughts/commit/73406d02549a110559be48002ac3d7c9bae73150))
* **errors,review:** background error feed with retry + OS proposal notifications ([92f7b29](https://github.com/equationalapplications/curated-thoughts/commit/92f7b29533e272d991a442c70477424799cb0aaa))
* **maintenance:** write healed events for repaired entities ([3a88f75](https://github.com/equationalapplications/curated-thoughts/commit/3a88f75f3c5b853fc5b751a93962c07bdb6686dc))
* **okf:** write exported event on bundle export ([ed97f4b](https://github.com/equationalapplications/curated-thoughts/commit/ed97f4b35620e18eae7f06f77a7e90afa548d49f))
* **shell:** timeline/tasks modes, live activity feed, nav wiring ([4534cb6](https://github.com/equationalapplications/curated-thoughts/commit/4534cb674a60ca21e680100257c608181c8e7e98))
* **styles:** timeline, tasks, activity feed styles ([5da7636](https://github.com/equationalapplications/curated-thoughts/commit/5da7636f477fa834a51fd066ba44965d82e3fd1f))
* **tasks:** tasks mode grouped by entity with manual create ([3825203](https://github.com/equationalapplications/curated-thoughts/commit/3825203bd1537476fd877140f9c40df12cbe2c24))
* **timeline:** global list_events query + tasks CRUD API with outbox CDC ([98e5cc3](https://github.com/equationalapplications/curated-thoughts/commit/98e5cc35b119b685d0994954b0f0596dacaa675a))
* **ui:** tauri bindings for timeline and tasks ([8dd3d75](https://github.com/equationalapplications/curated-thoughts/commit/8dd3d7538f338fd25a10c72babbd7f6bcd6c00b3))

### Bug Fixes

* add .worktrees to gitignore ([d9833bf](https://github.com/equationalapplications/curated-thoughts/commit/d9833bf11c3eeaa830bf0818d0c3aec03ae6dfb4))
* address code-review findings on Phase 5 ([930f913](https://github.com/equationalapplications/curated-thoughts/commit/930f9133ee77823ce822978bb0bef5c5477e148d))
* address PR review comments for Phase 5 implementation ([e3e4d53](https://github.com/equationalapplications/curated-thoughts/commit/e3e4d5342726c52cbdec0c531a90d11f50d084a8))
* **errorFeed:** refresh snapshot reference so useSyncExternalStore re-renders ([353abff](https://github.com/equationalapplications/curated-thoughts/commit/353abff1bcbe32a9f6038fcdaecebb73cbf2a97c))
* **events:** composite (created_at_ms, id) cursor so same-ms events paginate ([493f4b3](https://github.com/equationalapplications/curated-thoughts/commit/493f4b32ac3b440440821e5afd3f2b0cba873ed8))
* **events:** resolution events use approved/rejected taxonomy per backend spec ([426991d](https://github.com/equationalapplications/curated-thoughts/commit/426991d0d853c14a27e2aee9242dfca98abcaaf1))
* **librarian:** propagate synthesized-event write failures ([0d431cf](https://github.com/equationalapplications/curated-thoughts/commit/0d431cf76481f50f50dec9b744bf5c5f3cd22b69))
* **review:** actually wire inline edited_payload editing (Task 16) ([294f2c3](https://github.com/equationalapplications/curated-thoughts/commit/294f2c39c267c50af4307ccc5d74d718f2fb2296))
* sync Rust DDL with core-llm-wiki 5.5.1 and fix lint errors ([aeeae85](https://github.com/equationalapplications/curated-thoughts/commit/aeeae85247d622412e22a3664725a327a8673cc4))
* **tasks:** address CodeRabbit review on TasksMode ([b541466](https://github.com/equationalapplications/curated-thoughts/commit/b5414665d16e776d817f562b15c3df47cf47131a))

## [1.15.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.14.0...v1.15.0) (2026-07-07)

### Features

* **brain:** add frontend bindings for entity connections and fact CRUD ([ac6c5c8](https://github.com/equationalapplications/curated-thoughts/commit/ac6c5c8d18fb71cb21b04347377f7e6792682855))
* **brain:** composed entity page (header, summary, facts, strips) ([7f4558b](https://github.com/equationalapplications/curated-thoughts/commit/7f4558b25a2ef3d8f58937cb677af88407b974bc))
* **brain:** connections and fact CRUD tauri commands ([0e83b62](https://github.com/equationalapplications/curated-thoughts/commit/0e83b62f18eeeef29caaf8f2dc1651aac8f110b8))
* **brain:** connections panel (backlinks + edges by type) ([cb3de88](https://github.com/equationalapplications/curated-thoughts/commit/cb3de88f634a8f917e51c4423ea98dc8fb932246))
* **brain:** entity connections query (edges + wikilink backlinks) ([e67d33a](https://github.com/equationalapplications/curated-thoughts/commit/e67d33af5e08939c883d5708fe11c0d19bac0fbf))
* **brain:** entity list hook ([ef58aa0](https://github.com/equationalapplications/curated-thoughts/commit/ef58aa03892e0c6eddb5dddd872d82fe8def5bd5))
* **brain:** entity list sidebar with grouping, filter, create ([6cca7ce](https://github.com/equationalapplications/curated-thoughts/commit/6cca7ce7ee3d6b28726e13b0e4e1ea2dd3d265e9))
* **brain:** entity summary section with BlockNote editing ([ab41b1c](https://github.com/equationalapplications/curated-thoughts/commit/ab41b1c3adbe09d6d45d39d6b5dc3c3cd64e8444))
* **brain:** entity-first BrainMode layout ([52cc2af](https://github.com/equationalapplications/curated-thoughts/commit/52cc2af97cbe941f02e0ec243d35559185f99b3d))
* **brain:** fact card with inline edit, archive, source chips ([0364f19](https://github.com/equationalapplications/curated-thoughts/commit/0364f195d694cf17eb5034f02277f32955812114))
* **brain:** fact update and archive with outbox rows ([be3b03b](https://github.com/equationalapplications/curated-thoughts/commit/be3b03bf60567919c0be148e4c336f5c15c254e9))
* **brain:** phase 4 styles and spec status update ([5267159](https://github.com/equationalapplications/curated-thoughts/commit/5267159cf42b2af99b8b988dbc4944852f0dcdb1))
* **brain:** resolve fact source documents from source_ref ([78b3ea6](https://github.com/equationalapplications/curated-thoughts/commit/78b3ea6e1b02f3a3b9c8ba57be221ed2671847ab))
* **brain:** wikilink chip rendering ([a2d33c8](https://github.com/equationalapplications/curated-thoughts/commit/a2d33c8ba278d771096abd777322f45073ad0959))
* **shell:** cross-mode navigation with back/forward history ([5d5b851](https://github.com/equationalapplications/curated-thoughts/commit/5d5b85155d1fff6159df38cdde66950ffe4dffa3))
* **shell:** navigation history hook for cross-mode routing ([1251bcc](https://github.com/equationalapplications/curated-thoughts/commit/1251bcc4a3c693f5abb97e89ad5b5a3867e7cb17))

### Bug Fixes

* address PR [#31](https://github.com/equationalapplications/curated-thoughts/issues/31) review feedback ([82d67a5](https://github.com/equationalapplications/curated-thoughts/commit/82d67a5ff5ed8eda9392f76cac202b1611c25f04))
* **brain:** add entity validation to update/archive_fact ([b9bd02a](https://github.com/equationalapplications/curated-thoughts/commit/b9bd02a4fc2f31be4e13256b264d22c6ec1e7e70))

### Performance Improvements

* **brain:** batch-load endpoint labels to fix N+1 query issue ([e9a0cb5](https://github.com/equationalapplications/curated-thoughts/commit/e9a0cb5e7baaf4dc7d729c85184c64a1ba553995))

## [1.14.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.13.0...v1.14.0) (2026-07-07)

### Features

* **privacy:** enforce three-mode posture with Cloud Bridge gating ([a512051](https://github.com/equationalapplications/curated-thoughts/commit/a512051bfc88c4a893dd1d11ea98da364140016d))

### Bug Fixes

* **privacy:** remove unused refresh callback from usePrivacyMode ([0a57454](https://github.com/equationalapplications/curated-thoughts/commit/0a57454c38688511ca80be634cb24095752c0075))
* **test:** make provider rollback init-failure test runner-independent ([2f16c0f](https://github.com/equationalapplications/curated-thoughts/commit/2f16c0ffc3f62d440d27a68e963a6fd3ec273b36))
* **test:** skip keyring in provider rollback init-failure test ([6a2d0d2](https://github.com/equationalapplications/curated-thoughts/commit/6a2d0d24b4844909b24828199b6ff4f3d7ad7f59))

## [1.13.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.12.0...v1.13.0) (2026-07-06)

### Features

* **okf:** implement profile-v1 bundle import/export end-to-end ([96e97fc](https://github.com/equationalapplications/curated-thoughts/commit/96e97fc70e0e2910912fc62a25346a9664a18f77))

### Bug Fixes

* **okf:** address PR review feedback on bundle fidelity and UX ([91b3176](https://github.com/equationalapplications/curated-thoughts/commit/91b317647917d114b4852f4ed1149c20265abad0))
* **okf:** restore created_at param on import audit event INSERT ([bb5b9bc](https://github.com/equationalapplications/curated-thoughts/commit/bb5b9bc16515c6c23fdf2dd98785ba5ac6c77803))
* **test:** supply NOT NULL wiki fixture columns for MCP integration ([8d2f251](https://github.com/equationalapplications/curated-thoughts/commit/8d2f25114cbd19744fefb2d869da2a2efb86544a))

## [1.12.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.11.1...v1.12.0) (2026-07-06)

### Features

* **review:** Phase 2 slices 3–4 — per-item toggles and entity-aware diff ([f60d019](https://github.com/equationalapplications/curated-thoughts/commit/f60d0199c486272d6f685bac310f46d6cbd1010c))

### Bug Fixes

* **review:** address PR review feedback on diff and approvals ([859eb0d](https://github.com/equationalapplications/curated-thoughts/commit/859eb0db5af308fac369d7ea33a3452b97097f98))

## [1.11.1](https://github.com/equationalapplications/curated-thoughts/compare/v1.11.0...v1.11.1) (2026-07-06)

### Bug Fixes

* **release:** refresh tools lockfile during version bumps ([235ceaf](https://github.com/equationalapplications/curated-thoughts/commit/235ceafb5efd004631b029f1f471785566421b94))

## [1.11.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.10.0...v1.11.0) (2026-07-06)

### Features

* **review:** wire Phase 2 queue to V7 proposal API (Slice 1) ([ace568d](https://github.com/equationalapplications/curated-thoughts/commit/ace568d648a7785f3222b29e63b7720861f09ddb))

### Bug Fixes

* **ci:** scope outbox tests to lib crate to avoid linker OOM ([2625e42](https://github.com/equationalapplications/curated-thoughts/commit/2625e4266308e866f99eef8e391128e9a6eeacf5))
* **ci:** silence rustc warnings in outbox test build ([94cef6f](https://github.com/equationalapplications/curated-thoughts/commit/94cef6f4ab42bc43bab67978b02f79bb3055be5f))
* **review:** address PR review feedback on proposal detail loading ([b5c026a](https://github.com/equationalapplications/curated-thoughts/commit/b5c026a3628371b175cea2c822acea9632128141))
* **review:** handle null proposal detail without duplicate error ([13c88c1](https://github.com/equationalapplications/curated-thoughts/commit/13c88c10940a5dd3b74f31da4568591ea388b8b4))
* **review:** harden V7 proposal desk wiring from review feedback ([a27f70d](https://github.com/equationalapplications/curated-thoughts/commit/a27f70d8eb97ea2daece98fb15955d5afd7b327f))
* **review:** surface queue errors in empty state and clear stale data ([2d10c3f](https://github.com/equationalapplications/curated-thoughts/commit/2d10c3fc28e5d8569cf583c58b59e5d7841bac49))

## [1.10.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.9.0...v1.10.0) (2026-07-06)

### Features

* **api:** add entity CRUD and retire wiki ingest post-V7 ([ff1bef7](https://github.com/equationalapplications/curated-thoughts/commit/ff1bef7c3571663796e5e1b693799273142f204f))
* **api:** add proposal Tauri commands and legacy review shims ([7a3c571](https://github.com/equationalapplications/curated-thoughts/commit/7a3c5710651ffcfdbb169dfa783b3aa9e886b943))
* **db:** add curated proposal store with supersede and evidence hydration ([ad67fbe](https://github.com/equationalapplications/curated-thoughts/commit/ad67fbe6f3fdddb1eb54fd35fcb73c6ea782c3cd))
* **db:** add llm_wiki outbox CDC format matching core-llm-wiki@4.9.0 ([5094a9a](https://github.com/equationalapplications/curated-thoughts/commit/5094a9a4405a5ed64574f77c12a55eb163fd08bb))
* **db:** add resolve_proposal commit path with outbox staging ([19cda50](https://github.com/equationalapplications/curated-thoughts/commit/19cda5083f1cb6356e856231ff58835f21cfdb5c))
* **db:** add V7 OKF schema, DDL compat guard, and migration fixtures ([54c887b](https://github.com/equationalapplications/curated-thoughts/commit/54c887b8c635615f78dfa5ced057cf91859773eb))
* **db:** verify llm_wiki columns at startup against pinned package schema ([1d459a7](https://github.com/equationalapplications/curated-thoughts/commit/1d459a78efc8e4c9c27adb4877157bcd316bf329))
* **librarian:** replace wiki page synthesis with OKF JSON proposals ([8bd1bd1](https://github.com/equationalapplications/curated-thoughts/commit/8bd1bd1104b075696bd710b63a9d10ae60356bc6))
* **review:** ship three-column editorial desk (Phase 2 UX vision) ([c43dddb](https://github.com/equationalapplications/curated-thoughts/commit/c43dddb1ac694af297786186b7769fb00058e881))
* **ui:** replace modals with mode-based shell (Phase 1 UX vision) ([8f9e8c1](https://github.com/equationalapplications/curated-thoughts/commit/8f9e8c129b8fabe49798a8a73471b479e7a2b6d0))

### Bug Fixes

* address CodeRabbit review on spec links and CI setup ([7e114db](https://github.com/equationalapplications/curated-thoughts/commit/7e114dbdf4ae9a421cc12e40019710d613e44eb5))
* **ci:** install node deps before Rust tests for DDL compat guard ([9ec10e7](https://github.com/equationalapplications/curated-thoughts/commit/9ec10e7e73dee12bde2c454726d5e1299d3e5d2e))
* repair syntax errors breaking CI build ([8f2a53f](https://github.com/equationalapplications/curated-thoughts/commit/8f2a53f7c8bfd1c785677f9806e12440c6f603a3))
* **review:** guard readDocument effect against stale page switches ([9d71af2](https://github.com/equationalapplications/curated-thoughts/commit/9d71af201ec367829f11eeb4800ae7b5b0ad3cc3))
* **test:** seed V7 llm_wiki_entries columns in MCP integration test ([d13d366](https://github.com/equationalapplications/curated-thoughts/commit/d13d366b04947033451947e71765bf190a635bb1))

## [1.9.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.8.0...v1.9.0) (2026-07-05)

### Features

* **cloud_bridge:** align desktop bridge with Clanker wire protocol ([ae3f008](https://github.com/equationalapplications/curated-thoughts/commit/ae3f0081a89f6cb6ca8202a0139fff24d734202b))

### Bug Fixes

* **cloud_bridge:** address PR review and flaky no_ping_before_ready test ([fbb6c82](https://github.com/equationalapplications/curated-thoughts/commit/fbb6c82b17e68b2b6a51eba4278f85f0063aadda))
* **cloud_bridge:** refresh liveness only on well-formed frames ([26a59e6](https://github.com/equationalapplications/curated-thoughts/commit/26a59e69e53e256b75b5560429f6d355dcc419c0))

## [1.8.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.7.0...v1.8.0) (2026-07-01)

### Features

* add Clanker cloud bridge for remote vault tool dispatch ([6a834c7](https://github.com/equationalapplications/curated-thoughts/commit/6a834c712571bd839a03e8f5a72b5bc237db9443))

### Bug Fixes

* address Copilot review feedback on cloud bridge PR ([dcfeffa](https://github.com/equationalapplications/curated-thoughts/commit/dcfeffabf7d66dbac7572cd919996e0b5d154974))
* address PR [#21](https://github.com/equationalapplications/curated-thoughts/issues/21) review feedback for cloud bridge hardening ([16e0398](https://github.com/equationalapplications/curated-thoughts/commit/16e03985bfaa6db40535ebf9d097911413fce57e))

## [1.7.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.6.0...v1.7.0) (2026-06-24)

### Features

* **mcp:** add wiki graph tools for Active Librarian memory ([448b392](https://github.com/equationalapplications/curated-thoughts/commit/448b392fc07bc684aa343adf1242dda0d2535cd5))

### Bug Fixes

* **mcp:** address CodeRabbit review on wiki graph tools ([55e2cc7](https://github.com/equationalapplications/curated-thoughts/commit/55e2cc700e86fe9ee37ad7784ff1ec4eeb69523a))
* **mcp:** align wiki_graph scoring and traversal with tieredRead ([7b4195d](https://github.com/equationalapplications/curated-thoughts/commit/7b4195da31411e82a7d40813c16edf4224cd89ea))

## [1.6.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.5.1...v1.6.0) (2026-05-29)

### Features

* **byoi:** complete provider routing, fastembed bootstrap, and sidecar download progress ([c148eef](https://github.com/equationalapplications/curated-thoughts/commit/c148eefe8532ece88fdef0002236a54aa460ce2d))

### Bug Fixes

* **byoi:** make provider config writes safe, protect download paths, and harden sidecar routing ([0c374b3](https://github.com/equationalapplications/curated-thoughts/commit/0c374b314cc98f2d16fa683624ba0d0786522be9))
* improve provider rollback, local embedding routing, and setup accessibility ([7cdf516](https://github.com/equationalapplications/curated-thoughts/commit/7cdf516de0cad221806d44d3407f6847f29178d5))
* pass required WikiBusyError args for provider-not-ready mapping ([328f5b4](https://github.com/equationalapplications/curated-thoughts/commit/328f5b41b3a51096614c284c9a0b046bb3f8918b))
* preserve provider state on rollback failure and normalize docs to pnpm ([5dfc956](https://github.com/equationalapplications/curated-thoughts/commit/5dfc9569278ef5b9878bbcbebe8eba918ba4b228))
* preserve setup completion on skip and stabilize sidecar provider lifecycle ([5493b5a](https://github.com/equationalapplications/curated-thoughts/commit/5493b5a31d2cdc42ad98824106271156fda143ac))
* **release:** support pnpm lockfile in semantic-release config ([ac48a87](https://github.com/equationalapplications/curated-thoughts/commit/ac48a872bd5ca5450c3d2b1cadae5ede6ac28c11))
* **test:** restore CURATED_BRAIN_DIR in folder_rules tests and fix sidecar summary model selection ([132d083](https://github.com/equationalapplications/curated-thoughts/commit/132d083d49385c912ebd0f2031702828067c62ad))

## [1.5.1](https://github.com/equationalapplications/curated-thoughts/compare/v1.5.0...v1.5.1) (2026-05-24)

### Bug Fixes

* **windows:** compare GetConsoleWindow() to null_mut() not 0 ([7bdf905](https://github.com/equationalapplications/curated-thoughts/commit/7bdf905d5188e44d5a5076a7c0af6911bf296a55))

## [1.5.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.4.0...v1.5.0) (2026-05-24)

### Features

* **mcp:** add mcp_server module with vault_semantic_search and vault_related_chunks tools ([046f382](https://github.com/equationalapplications/curated-thoughts/commit/046f38204e1a4071eccae0499d45d914b990753e))
* **mcp:** add mcp-server feature flag with rmcp/schemars optional deps ([07ecdd9](https://github.com/equationalapplications/curated-thoughts/commit/07ecdd9bb5d18388a051f67c35b770111309c70b))
* **mcp:** expose mcp_server module from lib under mcp-server feature ([770435e](https://github.com/equationalapplications/curated-thoughts/commit/770435ef4548ef8087f8247fce89a1dc1dedabad))
* **mcp:** multi-call dispatch — --mcp flag routes to stdio MCP server, GUI hides Windows console at runtime ([9cc0a8d](https://github.com/equationalapplications/curated-thoughts/commit/9cc0a8dc9a9e8e41cde41dd655374e40cb060b0e))
* **settings:** add AgentIntegrationPanel with MCP config snippet ([1836fb1](https://github.com/equationalapplications/curated-thoughts/commit/1836fb11f626a19406f2d8cc98e49afadb88c126))
* **settings:** add get_brain_dir Tauri command for absolute brain dir IPC ([984b212](https://github.com/equationalapplications/curated-thoughts/commit/984b212ded66500500a7b3677fdfa0fbf85ef953))
* **settings:** resolve brainDir via Tauri IPC instead of navigator.platform heuristic ([f4c8336](https://github.com/equationalapplications/curated-thoughts/commit/f4c833635841651f38b75b835f57011ba2272bbf))
* **settings:** wire AgentIntegrationPanel into SettingsModal ([d796fa1](https://github.com/equationalapplications/curated-thoughts/commit/d796fa18e0a523bbae3b9c5fbd609f49ce66c92f))

### Bug Fixes

* **frontend:** resolve TypeScript CI failure in AgentIntegrationPanel test ([09cbee4](https://github.com/equationalapplications/curated-thoughts/commit/09cbee4bda8f72ede0395fc73782ad0c8f7ef722))
* **mcp-server:** rename vault path normalization variable in build_path_candidates ([e95cf82](https://github.com/equationalapplications/curated-thoughts/commit/e95cf8296d7952492471a32443b36e262aa3da01))
* **mcp-server:** validate MCP doc_path input with vault-safe normalization ([792249d](https://github.com/equationalapplications/curated-thoughts/commit/792249de6602a20956ff6f80ba770903a13b6a18))
* **mcp:** address PR review — correct CARGO_BIN_EXE env var name, set_default subscriber guard, remove outbox from MCP mode, gate FreeConsole to release builds ([4c3ce30](https://github.com/equationalapplications/curated-thoughts/commit/4c3ce30e89083c8b956485a1d0cbe813108b862c))
* **mcp:** address PR review — invalid ARIA role, limit clamp, brainDir loading state, CI comment ([23380b0](https://github.com/equationalapplications/curated-thoughts/commit/23380b0b4e28bdc65be189ab9e1c704649aa5455))
* **mcp:** address unresolved MCP review feedback ([ccb841f](https://github.com/equationalapplications/curated-thoughts/commit/ccb841f9699b9ee3c6a44700f2bbd0e041fc1abf))
* **mcp:** guard against empty strip_prefix result in normalize_vault_path ([edf439a](https://github.com/equationalapplications/curated-thoughts/commit/edf439a1951389971f8ccfd7a58bbedda52d7442))
* **mcp:** guard clipboard fallback and tolerate non-UTF8 binary paths ([9f68723](https://github.com/equationalapplications/curated-thoughts/commit/9f687239f0f0625779ee407aeafefa1a75b8c1fd))
* **mcp:** guard SettingsModal async update, use shallow CI checkout, and clarify legacy curated-thoughts-mcp doc ([85e4f69](https://github.com/equationalapplications/curated-thoughts/commit/85e4f69fb16135b39a493e019d6991b12d65bac2))
* **mcp:** handle brain dir errors and tighten MCP path handling ([5a45a3b](https://github.com/equationalapplications/curated-thoughts/commit/5a45a3b5f054922b3803bb6c02219d2c8f3e7fb5))
* **mcp:** harden MCP path validation, isolate blocking DB work, and restore clipboard test state ([c59ba16](https://github.com/equationalapplications/curated-thoughts/commit/c59ba16c859b9f6e9ad1982f290cf4f3f9bdc61a))
* **mcp:** improve clipboard fallback, preserve Windows console, and grant cache permissions in CI ([b86d532](https://github.com/equationalapplications/curated-thoughts/commit/b86d5325decb09f0c1f525ca76b69ea3cba8569e))
* **mcp:** make MCP tracing setup robust when global subscriber already exists ([74b526d](https://github.com/equationalapplications/curated-thoughts/commit/74b526d2834e1bfa130a0fbce8f646ad094d0cf0))
* **mcp:** narrow mutex scope in vault_semantic_search; gate Unix path tests ([7e401c6](https://github.com/equationalapplications/curated-thoughts/commit/7e401c6ac915187c52a946d4cfdbb4c3c91e48de))
* **mcp:** normalize absolute doc paths in vault_related_chunks to vault-relative form ([a1ad206](https://github.com/equationalapplications/curated-thoughts/commit/a1ad20604ce63a1c922212aa82f9d025e2b349ca))
* **mcp:** preserve relative path first for build_path_candidates ([39340ab](https://github.com/equationalapplications/curated-thoughts/commit/39340ab6ba21eb98060ea5ebd6e1b155ee0d6365))
* **mcp:** prevent blocking runtime and harden vault path candidates ([40c9764](https://github.com/equationalapplications/curated-thoughts/commit/40c976417b345ddf3f576937f9a9ab85e5259aec))
* **mcp:** remove redundant vault.join candidate, fix relative-path ordering in build_path_candidates ([f433bc1](https://github.com/equationalapplications/curated-thoughts/commit/f433bc17a60347648dc23b7f6684a4d4db541267))
* **mcp:** resolve binary path and absolute vault path handling ([c0f17e0](https://github.com/equationalapplications/curated-thoughts/commit/c0f17e0424266c1b5041bfdd6913492696f5386f))
* **mcp:** resolve final review feedback and improve clipboard test ([b1742a4](https://github.com/equationalapplications/curated-thoughts/commit/b1742a4188cd8af66a408f4123f68b7486ddd65d))
* **mcp:** resolve PR review feedback for path handling, accessibility, and CI ([0907a0d](https://github.com/equationalapplications/curated-thoughts/commit/0907a0d7b5808c1e6cafd4acc76d76790071ce51))
* **mcp:** tighten path candidate handling and remove stale spec review commentary ([a03d423](https://github.com/equationalapplications/curated-thoughts/commit/a03d423dc42662d9b8150d0983a4de1513dbc168))
* **mcp:** use build_path_candidates + related_chunks_try_paths for vault-layout-agnostic path resolution ([33a544e](https://github.com/equationalapplications/curated-thoughts/commit/33a544ea82a569baacff5b8f33766843931208c2))
* **mcp:** use canonical brain DB parent dir in get_brain_dir and avoid redundant vault canonicalization in build_path_candidates ([41105e3](https://github.com/equationalapplications/curated-thoughts/commit/41105e3dafe22509e429608c579d53b02b26257b))
* **mcp:** use existing vault path normalization in MCP server and scope CI write permissions ([bf1a9d8](https://github.com/equationalapplications/curated-thoughts/commit/bf1a9d8f7c2c0e88fd99e9ba68e6bdc04a44ab33))
* **settings:** guard navigator.clipboard with optional chaining; add clipboard test ([1b3b1cd](https://github.com/equationalapplications/curated-thoughts/commit/1b3b1cd0b95849f8d97d533545eb508b72677f57))
* **settings:** prefer userAgentData.platform over deprecated navigator.platform; document stdout hygiene risk in mcp_server ([d70072d](https://github.com/equationalapplications/curated-thoughts/commit/d70072ddf87e3140c3a2c16280f3acd720d3034e))

## [1.4.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.3.2...v1.4.0) (2026-05-18)

### Features

* **outbox:** add fetch_pending and acknowledge SQLite helpers ([430dec4](https://github.com/equationalapplications/curated-thoughts/commit/430dec49bc4d342720bb069bc1a836889e2ac96b))
* **outbox:** add module skeleton — types, Sink trait, OutboxConfig ([89d6208](https://github.com/equationalapplications/curated-thoughts/commit/89d62087e09b62d6fcbfbbfdb3bead8d884d5a56))
* **outbox:** add PgSink and spawn_postgres_worker with sqlx ([4afe786](https://github.com/equationalapplications/curated-thoughts/commit/4afe7861ae20ad2bab0601645322e40ad3de9787))
* **outbox:** auto-init OutboxWorker in curated-thoughts-mcp from DATABASE_URL ([62451ee](https://github.com/equationalapplications/curated-thoughts/commit/62451ee541f287c0819bad4d187cdb8c23bcfe1f))
* **outbox:** enable outbox in core-llm-wiki config ([1af0843](https://github.com/equationalapplications/curated-thoughts/commit/1af08439c842fcfe432a646bb3a9ba50a52576f3))
* **outbox:** implement OutboxWorker and sync_batch with full test coverage ([1b91d9e](https://github.com/equationalapplications/curated-thoughts/commit/1b91d9e9dc02b64fac1caface9120a94ee9b4a63))
* **outbox:** wire OutboxWorker into Tauri — auto-init from DATABASE_URL, runtime commands ([5c0155d](https://github.com/equationalapplications/curated-thoughts/commit/5c0155da38e3c02ffa6e43c86d8bd1dbd64a007a))

### Bug Fixes

* align Tauri wiki adapter transaction callback with SQLiteAdapter type ([88d138a](https://github.com/equationalapplications/curated-thoughts/commit/88d138a5c3b4a49d4760c2f5f7fe76a5b5f8def4))
* **ci:** skip Postgres tests when DATABASE_URL is empty ([161cfc6](https://github.com/equationalapplications/curated-thoughts/commit/161cfc618429ec827bf217d03b5772175d8bf870)), closes [#10](https://github.com/equationalapplications/curated-thoughts/issues/10)
* **ci:** stabilize Ubuntu Postgres integration tests ([61147d2](https://github.com/equationalapplications/curated-thoughts/commit/61147d27412426a84778150dff8688622072dde1))
* cooperative outbox worker shutdown and restart on vault recovery ([defd5e6](https://github.com/equationalapplications/curated-thoughts/commit/defd5e665b45f0d61bec8ca2de821a0f98cb8032))
* **outbox:** add ::BIGINT cast on synced_at DEFAULT expression for Postgres compatibility ([ac94779](https://github.com/equationalapplications/curated-thoughts/commit/ac94779e7ca606dd42819e8ddc8fc6420f6ada0b))
* **outbox:** add table safety note and ?1..?N binding comment ([d6e3754](https://github.com/equationalapplications/curated-thoughts/commit/d6e375485e7ad5212ddb9aa27bf657892f293829))
* **outbox:** address 3 unresolved Copilot review threads ([f721360](https://github.com/equationalapplications/curated-thoughts/commit/f7213608f649ff863fce82646aa729ce08ba56da))
* **outbox:** address Copilot review — CI, sqlx deps, worker path + validation ([c312995](https://github.com/equationalapplications/curated-thoughts/commit/c31299545c64567cd7effd3bafdccf2f78545935))
* **outbox:** address Copilot review — error propagation, vault lifecycle, retry, cleanup ([42cac63](https://github.com/equationalapplications/curated-thoughts/commit/42cac63854da0c786c137684cc39822ecbca9cfa))
* **outbox:** address Copilot review — transaction atomicity, CI, opt-in outbox, recovery respawn ([31faf37](https://github.com/equationalapplications/curated-thoughts/commit/31faf3769966f1b20989049fdc124228a6e93f42))
* **outbox:** address PR review issues ([c6c0d4a](https://github.com/equationalapplications/curated-thoughts/commit/c6c0d4a58f0a9d0ffdeab0014775154ae53937bd))
* **outbox:** align Sink trait with Prisma adapter spec ([3158287](https://github.com/equationalapplications/curated-thoughts/commit/31582879af4989f47057b4c09aeb5b8f11ce7d38))
* **outbox:** alphabetical mod ordering, sentinel defaults doc comment ([5c4fa41](https://github.com/equationalapplications/curated-thoughts/commit/5c4fa410d0c219b617b4b8faf1dd628dede3f246))
* **outbox:** await worker shutdown on vault switch and normalize DATABASE_URL handling ([03bfbda](https://github.com/equationalapplications/curated-thoughts/commit/03bfbdad8a191dba5aa4971cb8646f2b65d46ae3))
* **outbox:** bound PgSink connect to 10s timeout per attempt ([d0b8d42](https://github.com/equationalapplications/curated-thoughts/commit/d0b8d427c262738d6b3f23b0d99393db61ceeb7d))
* **outbox:** derive SQLite path from live DbState in start_outbox_worker ([d7492b0](https://github.com/equationalapplications/curated-thoughts/commit/d7492b0a90c67aaccd58d47bb5c706ef1fce75d2))
* **outbox:** document run() sleep-first behavior and JoinHandle abort contract ([6bb1881](https://github.com/equationalapplications/curated-thoughts/commit/6bb1881930e265007f3e92dd7fb8c30921480f26))
* **outbox:** emit outbox-worker-started after recovery restart ([28611cd](https://github.com/equationalapplications/curated-thoughts/commit/28611cdb7673e3a7f2d8376240b56c7a76ddf777))
* **outbox:** harden table name validation, improve runtime outbox state, add sqlx macros feature ([6901f86](https://github.com/equationalapplications/curated-thoughts/commit/6901f86a68f9ab6ca921b13937770d9ef7d9b67e))
* **outbox:** make Postgres retry sleep cancelable ([f26f075](https://github.com/equationalapplications/curated-thoughts/commit/f26f075b8e46772da10c0b3352be557a30e1284c))
* **outbox:** preserve runtime outbox config across vault switch and swap wiki instance after setup ([7ca8854](https://github.com/equationalapplications/curated-thoughts/commit/7ca8854adfefeeb8f467464349e2c77b07fe8b1e))
* **outbox:** preserve runtime worker config, cancel promptly, and update frontend wiki provider ([933de14](https://github.com/equationalapplications/curated-thoughts/commit/933de143508e61ecfdb0f0dd23253048ba5467bf))
* **outbox:** report skipped insert failures from sync_batch ([a1944bd](https://github.com/equationalapplications/curated-thoughts/commit/a1944bd99b3f033c9be8f0fba7cd63d0461f0fa4))
* **outbox:** resolve PR review issues ([d04deaa](https://github.com/equationalapplications/curated-thoughts/commit/d04deaa7db74f26ff70a7dc55744658b42e654ba))
* **outbox:** restrict runtime start to configured DATABASE_URL and remove unnecessary sqlx macros feature ([d2466bc](https://github.com/equationalapplications/curated-thoughts/commit/d2466bcef141cdfbfb09317cef7c72d92e8e3b5b))
* **outbox:** serialize env-var tests; emit stopped on total vault-switch failure ([c4ad99e](https://github.com/equationalapplications/curated-thoughts/commit/c4ad99ef00a67cc2f223ff9f41c4461d65575e63))
* **outbox:** stop stale worker on vault switch and remove empty DATABASE_URL from macOS CI ([ce0ecc1](https://github.com/equationalapplications/curated-thoughts/commit/ce0ecc1c7ea55b9e1dd8ca4678b3e9eba7a08f62))
* **outbox:** tolerate missing SQLite outbox table and emit stopped event on SQLite open failure ([2474850](https://github.com/equationalapplications/curated-thoughts/commit/2474850f05096680e44569af22f764d6dcea3037))
* **outbox:** validate DATABASE_URL in MCP, reject invalid on_error values, and sync runtime command docs ([8fffd9d](https://github.com/equationalapplications/curated-thoughts/commit/8fffd9dc88f41736d9fd0d4406fd16330faeab1a))
* resolve PR [#10](https://github.com/equationalapplications/curated-thoughts/issues/10) review comments ([353413f](https://github.com/equationalapplications/curated-thoughts/commit/353413fc4ca4eb8969d88e2c9e33ef40af25fb96))
* **wikiAdapter:** type invoke<void> so execAsync satisfies Promise<void> contract ([64bee1b](https://github.com/equationalapplications/curated-thoughts/commit/64bee1bc2f304322171b3d93b9a8748d1c5acf7e))

## [1.3.2](https://github.com/equationalapplications/curated-thoughts/compare/v1.3.1...v1.3.2) (2026-05-14)

### Bug Fixes

* LRU eviction, listen mock type, and search profiler guard ([c4c3b3c](https://github.com/equationalapplications/curated-thoughts/commit/c4c3b3c0534b50e1587362d03ec9658f0eaa1201))
* normalize legacy event keys in startAutoMaintenance, add partial-payload regression tests, reduce cache lock acquisitions ([af5d36d](https://github.com/equationalapplications/curated-thoughts/commit/af5d36d94fc944c24b79998119ad8dd36f0eff7f))
* **phase4:** address review findings for maintenance, graph, cache, and docs ([36b07fa](https://github.com/equationalapplications/curated-thoughts/commit/36b07faa9c7e2a99f18bbf68b9b79c3cf8e37042))
* **search:** Arc<[f32]> cache storage, isolate eviction tests from global state ([9554956](https://github.com/equationalapplications/curated-thoughts/commit/9554956db9e0419b8e2774868b777e3b23f791d7))

### Performance Improvements

* **search:** drop O(n) retain on cache hits, remove dead helpers ([29c0be0](https://github.com/equationalapplications/curated-thoughts/commit/29c0be0311a4b64cfeda5e4b09ef1bdec3ef574d))

## [1.3.1](https://github.com/equationalapplications/curated-thoughts/compare/v1.3.0...v1.3.1) (2026-05-14)

### Bug Fixes

* **reembed:** remove double pending.fetch_add in run_wiki_reembed ([4c50b46](https://github.com/equationalapplications/curated-thoughts/commit/4c50b465cd4ec60f1604a2b080519132728a8fab))
* **review:** resolve PR [#9](https://github.com/equationalapplications/curated-thoughts/issues/9) review threads and race conditions ([ae4ce21](https://github.com/equationalapplications/curated-thoughts/commit/ae4ce219ecb95a6cf66b34f4c6aa880c55b6a0dd))
* **run_wiki_reembed:** avoid manual ingesting state toggles and rely on pipeline PendingCount ([f5c9614](https://github.com/equationalapplications/curated-thoughts/commit/f5c9614d2e158240341839d5f21e5a434add79d5))
* **search:** plumb entity_id tier through SearchResult; remove score cap ([2b84211](https://github.com/equationalapplications/curated-thoughts/commit/2b84211f608bb1448cdb97e50512e13debbbc48e))

## [1.3.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.2.1...v1.3.0) (2026-05-14)

### Features

* add Connected badge and Structural context divider to SearchResults ([5938654](https://github.com/equationalapplications/curated-thoughts/commit/5938654711e70bc10a140bcbbf772d0df664a37e))
* add get_impact_radius Tauri command ([e40810f](https://github.com/equationalapplications/curated-thoughts/commit/e40810f2229e1b9e7c0412757e50129f8666d890))
* add structural context section and Cascading Violation directive to librarian ([1d7980a](https://github.com/equationalapplications/curated-thoughts/commit/1d7980a340cc816d0d2c9d7aa85589eb3daf614c))
* add tauriGraphAdapter, GraphExpansionOptions, and tieredRead graphExpansion ([93d1e91](https://github.com/equationalapplications/curated-thoughts/commit/93d1e9189c89df51b35e8e5716879a541d8550c9))
* **db:** schema V6 llm_wiki_entries + graph/maintenance integration tests ([d020d01](https://github.com/equationalapplications/curated-thoughts/commit/d020d012976097e20f946c56ddd3cf5aac317940))
* **graph:** recursive CTE callers/callees traversal with diamond deduplication ([9ebac02](https://github.com/equationalapplications/curated-thoughts/commit/9ebac02c8cacd9577facb79ec8a858809e6e1ccc))
* **indexer:** add AstRefUse strategy tag for IMPORTS, fix CTE GROUP BY aggregation ([5fec53a](https://github.com/equationalapplications/curated-thoughts/commit/5fec53a040235fc6203f4f2eb9bf7b9862a5de46))
* **indexer:** Pass 2 reference extraction with AstRef strategy tag ([e19d831](https://github.com/equationalapplications/curated-thoughts/commit/e19d8310849486e779559b2edb0b9f2f73383233))
* **linker:** Pass 3 global resolver with entity-scoped CALLS edge creation ([b4eca97](https://github.com/equationalapplications/curated-thoughts/commit/b4eca97cea0c86478730b8e0ebc36dd99db7c47f))
* register tauriGraphAdapter in createWiki ([292c4f5](https://github.com/equationalapplications/curated-thoughts/commit/292c4f5cc670944eecf62a29fe4f53903b5c9a2a))
* **rust:** add get_workspace_id command for per-vault entity isolation ([f49d72d](https://github.com/equationalapplications/curated-thoughts/commit/f49d72d322d081d5ee73a10a8d6dda5767e10f80))
* **rust:** add run_wiki_heal, run_wiki_prune, run_wiki_reembed maintenance commands ([137f32e](https://github.com/equationalapplications/curated-thoughts/commit/137f32e90a735c8eeca4408819f333f2ec3925c6))
* **rust:** tier-labelled librarian prompt with source metadata and conflict directive ([e1f9b89](https://github.com/equationalapplications/curated-thoughts/commit/e1f9b89cd42aab4684ea53f6464ee7d82d70e5f8))
* **schema:** V5 migration – curated_relationships, defined_symbol, entity_id ([f0d88f5](https://github.com/equationalapplications/curated-thoughts/commit/f0d88f5888cf01f0c6a907af10da8accbf2fc288))
* **ts:** add entityIdForPath helper for three-tier ingestion routing ([6552022](https://github.com/equationalapplications/curated-thoughts/commit/6552022af30eeb955b0cc7be73343b74e30983c8))
* **ts:** add initWorkspaceId, tieredRead, startAutoHeal to wiki.ts ([2fcccfb](https://github.com/equationalapplications/curated-thoughts/commit/2fcccfb36e77009ed6b2e0d806bd43ccdc265310))
* **ts:** add useWikiStatus hook for reactive wiki job state ([8307a32](https://github.com/equationalapplications/curated-thoughts/commit/8307a320e6401b672e3ef102feec3b004de4921e))
* **ui:** add MaintenanceDashboard with Heal, Prune, Re-index controls ([29b3349](https://github.com/equationalapplications/curated-thoughts/commit/29b3349ab2ba8f67c3781625309e0b7805749ba9))
* **ui:** wire MaintenanceDashboard, initWorkspaceId, startAutoHeal into App and SettingsModal ([900dcc2](https://github.com/equationalapplications/curated-thoughts/commit/900dcc237ede819b76d8a549d188fb9b8b41089f))

### Bug Fixes

* address Copilot review issues from PR [#7](https://github.com/equationalapplications/curated-thoughts/issues/7) ([ace4b70](https://github.com/equationalapplications/curated-thoughts/commit/ace4b701119570333cab4d7fba5fa64c8a98d390))
* backfill V5 entity_id from vault-relative path prefixes instead of documents.tier ([465e82e](https://github.com/equationalapplications/curated-thoughts/commit/465e82e3e4aa868615b893f61140f3d42a617f41))
* canonicalize vault root before passing to pipeline ([ec72d3a](https://github.com/equationalapplications/curated-thoughts/commit/ec72d3a024ef9f61979831ae400b68d026ad493c))
* canonicalize vault root in get_workspace_id and bulk_reindex ([b5000da](https://github.com/equationalapplications/curated-thoughts/commit/b5000dac1509daccf6e6f927d5cb9eda2c3ebfa9))
* exclude .worktree from lint/test and remove duplicate PipelineJob import ([ca581de](https://github.com/equationalapplications/curated-thoughts/commit/ca581de0ae02061b38a48dde93ac086d31dab9e7))
* ignore stale initWorkspaceId results during vault switches ([8cdc533](https://github.com/equationalapplications/curated-thoughts/commit/8cdc5338e2603a4a091c340fbeb3974c51c6b589))
* **indexer:** improve snippet context for method calls and TS imports, remove FK pragma from ingest_file ([54884a9](https://github.com/equationalapplications/curated-thoughts/commit/54884a95a9fcfb642e193f1f6e757229a965e212))
* **indexer:** remove dead delete_doc_relationships, add ref-extractor tests ([ee2176d](https://github.com/equationalapplications/curated-thoughts/commit/ee2176de0189b25128713f82df153892a19c0219))
* **lib:** import PipelineJob unconditionally ([b516549](https://github.com/equationalapplications/curated-thoughts/commit/b516549051a1bba2b0ce8034237a1833058c0c98))
* **librarian:** fallback to tier when entity_id is missing for label selection ([4e11279](https://github.com/equationalapplications/curated-thoughts/commit/4e112793e34e5f70af21e3bfd559322d6d610dfb))
* migrate tier_working entity ids and batch linker execution ([56620c0](https://github.com/equationalapplications/curated-thoughts/commit/56620c0c2abeb85edf73feb34165eba9cc9a29af))
* narrow tieredRead type cast and add graphExpansion forwarding test ([f82b98d](https://github.com/equationalapplications/curated-thoughts/commit/f82b98de2f89368cc20723ebbbfc10649d2a1b89))
* normalize vault root for tier_working workspace IDs in Rust pipeline ([c1176c0](https://github.com/equationalapplications/curated-thoughts/commit/c1176c0d5cbacc0c5bddc851df872030abf53c49))
* **phase2:** address Copilot review issues from PR [#7](https://github.com/equationalapplications/curated-thoughts/issues/7) (second pass) ([998e7fe](https://github.com/equationalapplications/curated-thoughts/commit/998e7fe476297a7f7fb6299d8a78d8f31a3050f1))
* **phase2:** address third Copilot review pass on PR [#7](https://github.com/equationalapplications/curated-thoughts/issues/7) ([ec80e80](https://github.com/equationalapplications/curated-thoughts/commit/ec80e805d63799f6928d6c1c727c61cd5800ea86))
* **phase2:** normalize workspace id, separate reembed pending counter, and emit auto-heal status ([a292b35](https://github.com/equationalapplications/curated-thoughts/commit/a292b3591db2f87d80a4fdeea96a4b04c2b452a8))
* **phase2:** route auto-heal through rust status events, map wiki graph roots to chunk ids, and run linker during bulk reindex ([d0b7e5d](https://github.com/equationalapplications/curated-thoughts/commit/d0b7e5d70462ac62e0f280efacdd5580bea0fead))
* **review:** address 2 Copilot review threads from PR [#7](https://github.com/equationalapplications/curated-thoughts/issues/7) ([2249e68](https://github.com/equationalapplications/curated-thoughts/commit/2249e687f2ad09dab31f21bee784c7d9015b32e3))
* **review:** address 3 remaining open Copilot review threads from PR [#7](https://github.com/equationalapplications/curated-thoughts/issues/7) ([1adb46a](https://github.com/equationalapplications/curated-thoughts/commit/1adb46a5bdf325586c15117c9bc70fd5b70247e7))
* **review:** address copilot review threads from PR [#7](https://github.com/equationalapplications/curated-thoughts/issues/7) ([2d9d1d0](https://github.com/equationalapplications/curated-thoughts/commit/2d9d1d07315aabec52dde6df147e781e8a2c0e88))
* **rust:** make bulk_reindex vault-root-aware and preserve wiki reembed status result ([5d6592a](https://github.com/equationalapplications/curated-thoughts/commit/5d6592af636a43eabb338dc92b6b33c0540a0c0c))
* **schema:** add unique constraint to curated_relationships, fix entity_id_for_path ([2612b42](https://github.com/equationalapplications/curated-thoughts/commit/2612b42008a3c9c6ac42190b1dcc0cde35ba49f0))
* **schema:** wrap V5 migration in transaction, simplify stale-rel cleanup ([244f073](https://github.com/equationalapplications/curated-thoughts/commit/244f073a861af29fe9274f927b3fb7d81c1ef74a))
* strengthen vault-safe path validation for wiki heal and structural neighbors ([133726a](https://github.com/equationalapplications/curated-thoughts/commit/133726a9f51b2f0b8b323cfdb3b3d12c50ef03c8))
* use stable composite keys in SearchResults ([e46bb66](https://github.com/equationalapplications/curated-thoughts/commit/e46bb668a26703a7f906741faa6e9a7f4dd085d6))
* **wiki:** remove dead listener, generalize ingestion helper ([63efa44](https://github.com/equationalapplications/curated-thoughts/commit/63efa44cacc15c79a088c27e9d46c66e58a5c77e))
* wire pending counter, cleanup, and busy guards ([2db4265](https://github.com/equationalapplications/curated-thoughts/commit/2db4265390c09b60fdd99730836a044c55245520))
* wrap run_linker in transaction for atomic edge rebuild ([961dd13](https://github.com/equationalapplications/curated-thoughts/commit/961dd13064180f05000a10a7571d3d0dffd93561))

## [1.2.1](https://github.com/equationalapplications/curated-thoughts/compare/v1.2.0...v1.2.1) (2026-05-12)

### Bug Fixes

* resolve merge conflicts and address Copilot review feedback ([659abb9](https://github.com/equationalapplications/curated-thoughts/commit/659abb9c998793b55776a3126e632985295be590))

## [1.2.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.1.0...v1.2.0) (2026-05-12)

### Features

* add default_vault_path() returning ~/Curated-Thoughts ([c965621](https://github.com/equationalapplications/curated-thoughts/commit/c965621f561e9a681f6edf5237c54a67943399d2))
* add VaultPanel CSS styles ([b98eecc](https://github.com/equationalapplications/curated-thoughts/commit/b98eecc1e5d7e27dc28a631a3ad47b75ae409b02))

### Bug Fixes

* add core-llm-wiki as direct dependency ([a4a154a](https://github.com/equationalapplications/curated-thoughts/commit/a4a154a18f71f63eeafaa5249cf8cc57e1e2ba3d))
* address PR review feedback ([28e3931](https://github.com/equationalapplications/curated-thoughts/commit/28e3931d1a0c9887e7c6c8634a5d7f7a4127d4ac))
* **ci:** add packageManager field to satisfy pnpm/action-setup@v4 ([e3cbffa](https://github.com/equationalapplications/curated-thoughts/commit/e3cbffad5530b8b6d77018aef1c7758b9ddb17e7))
* configure cargo binaries to fix multiplatform builds ([5a383b1](https://github.com/equationalapplications/curated-thoughts/commit/5a383b1851a71f01042c85b2977498ec91767c68))
* default vault initialization — only persist vault path if all directories created successfully ([1cb35d8](https://github.com/equationalapplications/curated-thoughts/commit/1cb35d85d5facfe7e9785eb76efa2a440e66a756))
* **deps:** downgrade eslint to v9 to resolve peer dependency conflict ([18d614f](https://github.com/equationalapplications/curated-thoughts/commit/18d614f511a7c6725671f0500f893eb155e0a8e8))
* fallback vault subdirs + --features dev-tools in bin doc comments ([62b290e](https://github.com/equationalapplications/curated-thoughts/commit/62b290e18e17c23e3129b77232c1424dc49d1c7c))
* remove duplicate test_default_vault_path_ends_with_curated_thoughts ([0543de5](https://github.com/equationalapplications/curated-thoughts/commit/0543de52557a5dc89398cad08e2cbb334268984e))
* remove package-lock.json and add vault recovery fallback ([03506a1](https://github.com/equationalapplications/curated-thoughts/commit/03506a1bf938b650e6be80d67fb0de12163ea80b))
* remove stray quote in embed_scifact parse_preset usage string ([17b7785](https://github.com/equationalapplications/curated-thoughts/commit/17b7785e93e25a75bcecfc9eaca67fce900b575e))
* workflows use pnpm, add rust cache, optimize build ([6dfa1e5](https://github.com/equationalapplications/curated-thoughts/commit/6dfa1e546aa877d9176f09c40e7a1b49f665dac4))

## [1.1.0](https://github.com/equationalapplications/curated-thoughts/compare/v1.0.4...v1.1.0) (2026-05-12)

### Features

* default vault path and settings vault switching ([780c6a0](https://github.com/equationalapplications/curated-thoughts/commit/780c6a02f001984c059ad6bc331e9f43dba51b77))

### Bug Fixes

* address PR review — spec, search state, atomic DB clear ([82a8415](https://github.com/equationalapplications/curated-thoughts/commit/82a8415ec7f49654906c45a23b7c0c153306b930))
* dialog restore prompt and watcher after vault switch ([92f4876](https://github.com/equationalapplications/curated-thoughts/commit/92f4876c0d5e752605f23d97195b84e3066552c5))
* guard same-vault switch and tidy notify import ([4325892](https://github.com/equationalapplications/curated-thoughts/commit/4325892d315521fce8cca6fa33e683838c35630f))
* restore prompt uses yes/no/cancel for plugin-dialog types ([6bd022a](https://github.com/equationalapplications/curated-thoughts/commit/6bd022ab239e96354364c52f210c14908361eea4))
* **tauri:** shorten locks in watcher start and full reindex ([2d3c53a](https://github.com/equationalapplications/curated-thoughts/commit/2d3c53abe52d024a2d82a69ed6a3f171bab7cd95))
* **tauri:** skip stub cleanup when recovery leaves stub DB open ([35beb4b](https://github.com/equationalapplications/curated-thoughts/commit/35beb4b51de1d10bbcab6f6449c76e73d4c5d0f6))
* **tauri:** validate vault path before DB backup ([6b76652](https://github.com/equationalapplications/curated-thoughts/commit/6b766520745fc663aa39cf7ee058321a2b0f6d13))
* use default_config_path in app startup ([572e955](https://github.com/equationalapplications/curated-thoughts/commit/572e95598f2e41afc2168185ba2f02d4bdb5ef58))
* vault backup dialog roles and switch_vault recovery ([e75d989](https://github.com/equationalapplications/curated-thoughts/commit/e75d9893a5b041d526101c197c3c660794ef3e55))
* vault path fallbacks, search stale guard, test app pipeline ([01842b4](https://github.com/equationalapplications/curated-thoughts/commit/01842b456bb995cbb77dccbcd48142bead18626a))
* vault switch UX, path validation, and SQLite sidecars ([d42cd0b](https://github.com/equationalapplications/curated-thoughts/commit/d42cd0b726b5b3ba519d537c72c4b3f1bb39334c))
* vault test mock and WAL-safe brain.db backup ([e1e89c7](https://github.com/equationalapplications/curated-thoughts/commit/e1e89c74a9bd29f8b3716e22b6dcc2f0f3c51920))
* **vault:** stub cleanup, validate backup path, align config on switch error ([3757f13](https://github.com/equationalapplications/curated-thoughts/commit/3757f136e02ee08a7485569046591bb0c95e40a7))

## [1.0.4](https://github.com/equationalapplications/curated-thoughts/compare/v1.0.3...v1.0.4) (2026-05-10)

### Bug Fixes

* trigger release to validate tauri-action build pipeline ([2f22577](https://github.com/equationalapplications/curated-thoughts/commit/2f2257718a5dc9586585ef7a12e34f6e32e27d9c))

## [1.0.3](https://github.com/equationalapplications/curated-thoughts/compare/v1.0.2...v1.0.3) (2026-05-10)

### Bug Fixes

* **build:** give dev-tools feature an optional dep so Tauri skips those bins ([abba76f](https://github.com/equationalapplications/curated-thoughts/commit/abba76f1e8a2e251226772cce37ffa39ca763332))

## [1.0.2](https://github.com/equationalapplications/curated-thoughts/compare/v1.0.1...v1.0.2) (2026-05-10)

### Bug Fixes

* **build:** gate utility bins behind required-features so Tauri skips them ([aac5b41](https://github.com/equationalapplications/curated-thoughts/commit/aac5b410067c38d55527306f82f2f0091dee6c88))

## [1.0.1](https://github.com/equationalapplications/curated-thoughts/compare/v1.0.0...v1.0.1) (2026-05-10)

### Bug Fixes

* **tauri:** set default-run for release builds ([4a6c77c](https://github.com/equationalapplications/curated-thoughts/commit/4a6c77c8c720aa74feb365004494bf00db761525))

## 1.0.0 (2026-05-10)

### Features

* add 3-panel app shell and wire setup gate in App.tsx ([afed95a](https://github.com/equationalapplications/curated-thoughts/commit/afed95a9044fbeba257421a00619a8e7bf2dc53e))
* add 4-step setup wizard with Ollama install and vault picker ([4c1bf11](https://github.com/equationalapplications/curated-thoughts/commit/4c1bf11d8c13ed2240b841ea912ff758a322d5bf))
* add background pipeline worker (ingest/delete jobs) ([b8dd0a2](https://github.com/equationalapplications/curated-thoughts/commit/b8dd0a2ab46a3bf9f6d8c03ffc1956dd212bd2fd))
* add DB query helpers for ingestion pipeline ([b5f088e](https://github.com/equationalapplications/curated-thoughts/commit/b5f088edd360ec98f7be71bd26b024e019703a4a))
* add embeddings table via migration V2 ([4806509](https://github.com/equationalapplications/curated-thoughts/commit/480650904354874ccf0a715d8605e5578b957680))
* add FastEmbed embedder wrapping AllMiniLML6V2 (384-dim) ([5041da4](https://github.com/equationalapplications/curated-thoughts/commit/5041da4bc538ccc801f28f7ce634ca8fb883a00e))
* add folder rules CRUD Tauri commands ([4aa095a](https://github.com/equationalapplications/curated-thoughts/commit/4aa095a9c2be65deb15e9f071e548cd706570826))
* add FolderTree with live vault file listing ([fb5b29e](https://github.com/equationalapplications/curated-thoughts/commit/fb5b29e535b0868356dc3417273e1c8c819302ea))
* add indexing status badge to sidebar (polls every 2s) ([58267bb](https://github.com/equationalapplications/curated-thoughts/commit/58267bbc2e600780ae26f73ed601da969da134e4))
* add librarian module — Ollama-powered wiki summary generation ([cec3f46](https://github.com/equationalapplications/curated-thoughts/commit/cec3f46ee7f86624fa3c931c2ffcb3daa4ba4919))
* add list_vault_files and read_document Tauri commands ([cc26dd5](https://github.com/equationalapplications/curated-thoughts/commit/cc26dd58ef7d1465215a4f37cd15dfacb41f07b8))
* add model management panel to settings (pull new model + list installed) ([8438ba4](https://github.com/equationalapplications/curated-thoughts/commit/8438ba4f43926cccb663af79cf1df6f3915e64e5))
* add model selection step with editable input pre-filled with recommended model ([83dd968](https://github.com/equationalapplications/curated-thoughts/commit/83dd968c8be1a85d141dd6aed2415274bf351520))
* add notify-based file watcher with Added/Modified/Deleted events ([92c7ce4](https://github.com/equationalapplications/curated-thoughts/commit/92c7ce49c620e8bc6feb6263c62f51fe34a9dee7))
* add Ollama detection and model pull with streaming progress ([f03f9d1](https://github.com/equationalapplications/curated-thoughts/commit/f03f9d1c6e598f4a948f41553a299c89c9a553e4))
* add SearchResult type, invoke wrappers, useSearch and useRelatedChunks hooks ([e172689](https://github.com/equationalapplications/curated-thoughts/commit/e1726895debd64d798f4c3d5aa31b0f969cd22aa))
* add SearchResults component and wire search into Sidebar ([6a58035](https://github.com/equationalapplications/curated-thoughts/commit/6a580358e514a9ddc0a20ae6a358cd0faba5f9a5))
* add semantic search and related-chunks Rust module ([24b1b4b](https://github.com/equationalapplications/curated-thoughts/commit/24b1b4bd378eff2da4d4691922ec6b33f703c2f9))
* add SHA-256 hashing utility for change detection ([0f0df17](https://github.com/equationalapplications/curated-thoughts/commit/0f0df1767461bf15c9c31d197f4d9350fd5b44b8))
* add SQLite connection with V1 schema migration ([5350508](https://github.com/equationalapplications/curated-thoughts/commit/5350508a87b92da8fc20212a4c59f3db8b16b7c5))
* add typed Tauri bridge, event listeners, and useSetupStatus hook ([a089d57](https://github.com/equationalapplications/curated-thoughts/commit/a089d575818efa4f997bd5870499a84678be9985))
* add vault config persistence with get/set ([fe2c1c9](https://github.com/equationalapplications/curated-thoughts/commit/fe2c1c950d9ffeb11e0ff38e6e68fecc729aec52))
* add word-based text chunker with 500-word chunks and 50-word overlap ([b2c988c](https://github.com/equationalapplications/curated-thoughts/commit/b2c988ccc7143df9a2c97132810fd18a937e1ff1))
* auto-open Ollama download page and poll for installation ([54d3f68](https://github.com/equationalapplications/curated-thoughts/commit/54d3f683cc230e156dae0da95c05f3c7c158494e))
* auto-select model based on RAM (0.5b/1b/3b) ([d10fdb3](https://github.com/equationalapplications/curated-thoughts/commit/d10fdb3ec35e776dca85008f767bc2e84151a98d))
* BlockNote editor in EditorPane (read-only source, editable wiki) ([b64c287](https://github.com/equationalapplications/curated-thoughts/commit/b64c2878fe3e5298e0be825f73dfd81a1a4f0301))
* bootstrap vault subdirs on set; restrict watcher ingestion to documents/ ([c0fbb5d](https://github.com/equationalapplications/curated-thoughts/commit/c0fbb5d70ab69a83bf9f8660379d082dbc3c0e7f))
* chunk metadata, MCP hints, CI frontend ([196f2f2](https://github.com/equationalapplications/curated-thoughts/commit/196f2f2421b04f0a367b260596b0d42e502460b0))
* **chunker:** AstSymbol chunking via tree-sitter ([bb706e2](https://github.com/equationalapplications/curated-thoughts/commit/bb706e2e232b0ffd0f6ff12d9c98805bc313f748))
* **chunker:** autodetect strategy by extension ([b0ef54c](https://github.com/equationalapplications/curated-thoughts/commit/b0ef54c7030c27a93ac058afc0b117d226173edf))
* convert PDF/DOCX via pandoc before chunking ([ad12054](https://github.com/equationalapplications/curated-thoughts/commit/ad120548f89f0256d9258f4200dab5884676f688))
* **db:** add MIGRATION_V4 chunk metadata columns ([0b70046](https://github.com/equationalapplications/curated-thoughts/commit/0b70046a76163cd6c4955f0e4ef4deeb29b74204))
* deletion cleanup (shadow copy + orphan wiki pages) + errors.log ([54ebfbb](https://github.com/equationalapplications/curated-thoughts/commit/54ebfbb05fdb6b5cb62aec1b6c709d003bee3e82))
* expose search_vault and get_related_chunks as Tauri commands ([f825978](https://github.com/equationalapplications/curated-thoughts/commit/f825978fd2a4de539ba55fa617eb969c2663bce1))
* **infra:** bulk reindex CLI and search profiling tools ([c284006](https://github.com/equationalapplications/curated-thoughts/commit/c284006c0e829e9961d85e556db86f0b202c41fc))
* integrate @equationalapplications/react-llm-wiki v3 with Tauri SQLite adapter ([6e26207](https://github.com/equationalapplications/curated-thoughts/commit/6e26207b71ada075b9b036ae972fddaa1e33b518))
* librarian respects folder_rules mode (index skips, auto_approve writes directly) ([c770816](https://github.com/equationalapplications/curated-thoughts/commit/c7708166e74ceefe8bf656bb604dd1abf14d0870))
* **mcp:** stdio vault_semantic_search and vault_related_chunks ([ee7cee1](https://github.com/equationalapplications/curated-thoughts/commit/ee7cee16e0dc136bddcf0cdbdd1b7a839bb8ce67))
* PDF/DOCX native extraction, drag-drop import, file delete, index reconciliation ([58d01d1](https://github.com/equationalapplications/curated-thoughts/commit/58d01d163728b280d92a9bca3083f8b2a179b9d0))
* **pipeline:** embed ingests via Ollama from EmbedProfile ([57da850](https://github.com/equationalapplications/curated-thoughts/commit/57da850cf714696117ea7c62fcdc2fcf745f045e))
* **rag:** V4 chunk metadata, Ollama embeds, search spans ([63855f4](https://github.com/equationalapplications/curated-thoughts/commit/63855f47ed0642571ff13bd70bd9be0f0b434647))
* **retrieval:** shared brain path resolution and search façade ([c543b72](https://github.com/equationalapplications/curated-thoughts/commit/c543b720d58892dce586b7ede46432ab0e08a45f))
* review modal loads actual proposed content from .brain/proposed/ ([f60b62c](https://github.com/equationalapplications/curated-thoughts/commit/f60b62ce14234b507fcf1180ceaf42d18b8e1571))
* review queue UI — badge opens modal to approve/reject proposed wiki pages ([b2aa1a9](https://github.com/equationalapplications/curated-thoughts/commit/b2aa1a9fc25014cfc4f13e8bfa200bd1e29d2b3b))
* run librarian after ingest; add review queue Tauri commands ([68be4ee](https://github.com/equationalapplications/curated-thoughts/commit/68be4ee0dbb3040c952956f0bb9e3107c79f237e))
* **safe_path:** canonicalize-and-contain path helper with unit tests ([7084d0b](https://github.com/equationalapplications/curated-thoughts/commit/7084d0bd8da699a565a45c510f084e261cfbff16))
* **safe_path:** module skeleton with SafePathError and PathMode ([4b89c0c](https://github.com/equationalapplications/curated-thoughts/commit/4b89c0cd306dd7485c8cd9220d5c8bf25247e66f))
* scaffold Tauri 2.x project with React TypeScript template ([25d5f29](https://github.com/equationalapplications/curated-thoughts/commit/25d5f29b81507ad2cbf9468ee5627880b06b766f))
* settings panel with folder rules (index/summarize/synthesize + auto-approve) ([1f652a5](https://github.com/equationalapplications/curated-thoughts/commit/1f652a53b36a2736a0ccb3e22ded95ca848349ce))
* **test:** expose make_test_app and PipelineWorker for integration tests ([aa883dc](https://github.com/equationalapplications/curated-thoughts/commit/aa883dc88a556a9e1aea72e043908b1b7ee5880b))
* wire all Rust modules and Tauri commands into lib.rs ([b58b0a2](https://github.com/equationalapplications/curated-thoughts/commit/b58b0a23755c2cdbc6ab686eb933a6fe7604a73a))
* wire pipeline into app — watcher events feed ingestion worker ([aa37241](https://github.com/equationalapplications/curated-thoughts/commit/aa3724199bf3ccf4ea56d2a1fa4a55a257a2b10f))
* wire RelatedNotes with cosine-similar chunks and selectedDoc state ([4442764](https://github.com/equationalapplications/curated-thoughts/commit/44427644435a281b885002d11a0c84e5899bd8c5))

### Bug Fixes

* add Cargo.lock verification and workflow_run fork guard ([01dce1f](https://github.com/equationalapplications/curated-thoughts/commit/01dce1f2e82b9cf8219661f80e4b3ccdaa0cc920))
* add error handling to version update script ([2336311](https://github.com/equationalapplications/curated-thoughts/commit/23363115c8ed0310de292900a0c766c65ceb223b))
* add orphaned status to wiki_pages; add save_wiki_page command; bootstrap .brain/converted/ ([6eaf0f2](https://github.com/equationalapplications/curated-thoughts/commit/6eaf0f260f0500a6814ebec02c5f86abfde249e3))
* address Copilot review (paths, watcher, drop UI, MayCreate) ([e48fcbb](https://github.com/equationalapplications/curated-thoughts/commit/e48fcbb86e6d737d1bb5062e45f073484bfe4dd1)), closes [#2](https://github.com/equationalapplications/curated-thoughts/issues/2)
* address final PR [#3](https://github.com/equationalapplications/curated-thoughts/issues/3) review feedback ([eae4f1f](https://github.com/equationalapplications/curated-thoughts/commit/eae4f1f9103160a4f5848b94d00f926b2e5d410c))
* address final PR [#3](https://github.com/equationalapplications/curated-thoughts/issues/3) review feedback ([500b649](https://github.com/equationalapplications/curated-thoughts/commit/500b649184e2b79ce150fa96792aaae34bfdfd7b))
* address final PR [#3](https://github.com/equationalapplications/curated-thoughts/issues/3) review feedback ([92a4056](https://github.com/equationalapplications/curated-thoughts/commit/92a40561c2e37d99504e623fb9edd84a8e86d4f8))
* address final PR [#3](https://github.com/equationalapplications/curated-thoughts/issues/3) review feedback ([81b3a84](https://github.com/equationalapplications/curated-thoughts/commit/81b3a84d69b388f420576a7c28f6b2ef0fc2a36e))
* address final round of PR [#3](https://github.com/equationalapplications/curated-thoughts/issues/3) review feedback ([ac8e861](https://github.com/equationalapplications/curated-thoughts/commit/ac8e8618848cbf1dfe5c5b37d278934165244410))
* address PR [#3](https://github.com/equationalapplications/curated-thoughts/issues/3) review feedback ([be952d0](https://github.com/equationalapplications/curated-thoughts/commit/be952d07328f5809e058e97e7da5130f910a2941))
* address PR [#3](https://github.com/equationalapplications/curated-thoughts/issues/3) review feedback ([9c12690](https://github.com/equationalapplications/curated-thoughts/commit/9c126901bbad87e67409b239f24b85ad6bb3e621))
* address PR [#3](https://github.com/equationalapplications/curated-thoughts/issues/3) security and reliability review feedback ([830cb22](https://github.com/equationalapplications/curated-thoughts/commit/830cb22c4c3578e6ac787fe1b93029e5a5d00063))
* address PR review issues for watcher idempotency, path canonicalization, and drop batch error handling ([d0daf00](https://github.com/equationalapplications/curated-thoughts/commit/d0daf0034eb8c15154c947b03fe9170f8bb0faa2))
* address PR2 review (symlink assertions, spec rollout) ([4634f5e](https://github.com/equationalapplications/curated-thoughts/commit/4634f5e8643c176a73b27f852010a83499ba7bc7))
* address second round PR [#3](https://github.com/equationalapplications/curated-thoughts/issues/3) review feedback ([fc83435](https://github.com/equationalapplications/curated-thoughts/commit/fc83435e226dbe5534210c9b753bbba9711d898f))
* auto-start Ollama server when installed but not running ([f73bfe0](https://github.com/equationalapplications/curated-thoughts/commit/f73bfe084f4f7ffcbd8a249900cd90668cfc0e7d))
* bump tauri 2.11.1 (CVE origin confusion), TS 6.0.3, react 19.2.6 ([f588e4b](https://github.com/equationalapplications/curated-thoughts/commit/f588e4b012f752a4af16ca3e26e7416e7cb13826))
* **ci:** satisfy clippy items_after_test_module ([fbd1b6f](https://github.com/equationalapplications/curated-thoughts/commit/fbd1b6fc68014d897d0cba4f5f35a57fde961fd1)), closes [#2](https://github.com/equationalapplications/curated-thoughts/issues/2)
* close remaining PR security review threads ([70159cb](https://github.com/equationalapplications/curated-thoughts/commit/70159cb3f4925bdb29c9d6d99f3dff742ccce2b3))
* correct npm trusted publishing reference link ([461a310](https://github.com/equationalapplications/curated-thoughts/commit/461a310b0a17b059ae4dc943347a588c5dad0271))
* correct OIDC trusted publishing instructions ([c24c5ab](https://github.com/equationalapplications/curated-thoughts/commit/c24c5abc4fd7ea215eed4702c2006a16a2ba3d96))
* correct plugin order and GitHub token usage in release workflow ([722bafc](https://github.com/equationalapplications/curated-thoughts/commit/722bafc4c0cea07ca9e88eeb3f40a2070cf1bb68))
* derive vault root from job path, not db_path ([7e1b753](https://github.com/equationalapplications/curated-thoughts/commit/7e1b753ab1d53857b9b77f8ca9925572b324229c))
* detect Ollama via path search; suppress foundation dead-code warnings ([9082cee](https://github.com/equationalapplications/curated-thoughts/commit/9082cee0d27806fe658ca9a8e010e14773dbc0f3))
* drain events in watcher delete test; remove protocol-asset feature ([123abcb](https://github.com/equationalapplications/curated-thoughts/commit/123abcbab3b3f5631fad94cd741fb4a92415c6cb))
* eliminate TOCTOU race in MayCreate path writes and copies ([b25e635](https://github.com/equationalapplications/curated-thoughts/commit/b25e635e8ca3bf9d44792d94efdb1d010ccd00df))
* enable FK pragma on pipeline connection; lower chunk size to 180 words for AllMiniLML6V2 ([8236c02](https://github.com/equationalapplications/curated-thoughts/commit/8236c02ca078bd5286b71daa3b8d3675755753dc))
* exclude test files from tsc build to fix blank app window ([aac4f6f](https://github.com/equationalapplications/curated-thoughts/commit/aac4f6f1330d114c3cf393548df9d684d4a7e9f0))
* offload drop-copy to thread, fix drag unlisten leak ([b2df671](https://github.com/equationalapplications/curated-thoughts/commit/b2df6716a06a949e9ca4abf9638c6b16d61d4092)), closes [#2](https://github.com/equationalapplications/curated-thoughts/issues/2)
* **plan:** add symlink-escape filter for allowed_canonical ([b4020c5](https://github.com/equationalapplications/curated-thoughts/commit/b4020c51f276f48521385f8d42feda11c66b3938))
* **plan:** prepend wiki/ prefix in approve_wiki_page migration ([0c32d40](https://github.com/equationalapplications/curated-thoughts/commit/0c32d40dd6ee2a5c8f28cc1fdccbb3bcd9a7b5be))
* **plan:** use VaultConfigState not caller-provided vault_path ([a95ea13](https://github.com/equationalapplications/curated-thoughts/commit/a95ea137ef850726c90784a21649af1a9fb70689))
* **release:** add conventionalcommits preset and doc accuracy ([6beeeed](https://github.com/equationalapplications/curated-thoughts/commit/6beeeed38d2661f2c94a4a783d25a0dcb9a2362a))
* remove incorrect OIDC token usage from build workflow ([1fea81a](https://github.com/equationalapplications/curated-thoughts/commit/1fea81a7059c654597e7ae51cdecf9e4e28c7a6b))
* replace semantic-release-cargo with custom version script ([5e7bfc8](https://github.com/equationalapplications/curated-thoughts/commit/5e7bfc852576a333eb84e0ee014c3b31c40ec912))
* **safe_path:** allocate temp file with create_new retry loop ([79edee8](https://github.com/equationalapplications/curated-thoughts/commit/79edee8e8f7fc9b2941f01bed15181ef4c64423b)), closes [#2](https://github.com/equationalapplications/curated-thoughts/issues/2)
* **safe_path:** preserve permissions on atomic write/copy ([32536da](https://github.com/equationalapplications/curated-thoughts/commit/32536da84a01d053d5aaf48563d7bbcae4295efb))
* **spec:** forbid vault_path as command parameter ([d88ae45](https://github.com/equationalapplications/curated-thoughts/commit/d88ae45a6d04f2c759830658eb10b8052ab212b0))
* **spec:** harden MCP retrieval plan per security review ([e093efe](https://github.com/equationalapplications/curated-thoughts/commit/e093efe1221400f6182c654af10ff2ec4c948e9b)), closes [#2](https://github.com/equationalapplications/curated-thoughts/issues/2) [#1](https://github.com/equationalapplications/curated-thoughts/issues/1) [#4](https://github.com/equationalapplications/curated-thoughts/issues/4) [#7](https://github.com/equationalapplications/curated-thoughts/issues/7)
* **tauri:** add NotARegularFile for MayCreate targets ([98e9e61](https://github.com/equationalapplications/curated-thoughts/commit/98e9e61f55026d5283314fd7936255458933c48e)), closes [#2](https://github.com/equationalapplications/curated-thoughts/issues/2)
* **tauri:** address bug bot path-contract and validation feedback ([808276a](https://github.com/equationalapplications/curated-thoughts/commit/808276af3749b2f8d7e8b7b19509c95839cd462e)), closes [#1](https://github.com/equationalapplications/curated-thoughts/issues/1)
* **tauri:** address Copilot path-security review round 2 ([0013301](https://github.com/equationalapplications/curated-thoughts/commit/00133015f038d14e6f0ff8a10f904f42111af83c))
* **tauri:** address Copilot path-security review round 3 ([1c6acd5](https://github.com/equationalapplications/curated-thoughts/commit/1c6acd51616aa04da483859c432ddd80a930a94b))
* **tauri:** address Copilot path-security review round 4 ([663ac96](https://github.com/equationalapplications/curated-thoughts/commit/663ac9642337ba741e1eb8e0fd4a52c05615fa96))
* **tauri:** address Copilot review path + proposed path edge cases ([2a5ac7a](https://github.com/equationalapplications/curated-thoughts/commit/2a5ac7a3818e39b1aa0e4eb7bbf8c24c3ca96d7e))
* **tauri:** address Copilot security review feedback ([cb232d4](https://github.com/equationalapplications/curated-thoughts/commit/cb232d49e4c983971101762c12882d882a52ab7e))
* **tauri:** address final Copilot security review feedback ([47c6713](https://github.com/equationalapplications/curated-thoughts/commit/47c67134ef498411c23fe70daa31f15a6a6b238b))
* **tauri:** allow get_related_chunks when doc file missing on disk ([cefcf73](https://github.com/equationalapplications/curated-thoughts/commit/cefcf73ac723a92259bf90c2c33ef9df7993165e))
* **tauri:** avoid overwriting on OS file drops ([19b99e5](https://github.com/equationalapplications/curated-thoughts/commit/19b99e5311130259d608820b74c766f7d593db4c)), closes [#2](https://github.com/equationalapplications/curated-thoughts/issues/2)
* **tauri:** close delete_vault_file path-traversal (Vuln 3) ([c49b356](https://github.com/equationalapplications/curated-thoughts/commit/c49b35614446e2cc142ae83587d801230656a9ed))
* **tauri:** close save_wiki_page path-traversal (Vuln 1) ([a4ecdff](https://github.com/equationalapplications/curated-thoughts/commit/a4ecdff68ce011cdf7f707fe654584a8a082f87b))
* **tauri:** harden vault path normalization for symlinks ([a285bc7](https://github.com/equationalapplications/curated-thoughts/commit/a285bc790b5fb71f91bcdafd8cc5f6560ba8409d))
* **tauri:** keep trying drop suffixes on non-file collisions ([e98b8cf](https://github.com/equationalapplications/curated-thoughts/commit/e98b8cf57f70be80216b5feca81d5508dbdb3224))
* **tauri:** mutex watcher gate and vault path normalization ([233399a](https://github.com/equationalapplications/curated-thoughts/commit/233399ab7e54f2b89b457efaaf47ec883e708483)), closes [#2](https://github.com/equationalapplications/curated-thoughts/issues/2)
* **tauri:** normalize path separators for cross-platform wiki operations ([d27058f](https://github.com/equationalapplications/curated-thoughts/commit/d27058fb1e57f5ddfecc3a3d0698d06a909a8299))
* **tauri:** probe perms via symlink_metadata in safe_write_bytes ([0854337](https://github.com/equationalapplications/curated-thoughts/commit/08543376681db031b0d4a46d6997060cca4ed292)), closes [#2](https://github.com/equationalapplications/curated-thoughts/issues/2)
* **tauri:** return symlink path from MustExist for delete semantics ([5fb4211](https://github.com/equationalapplications/curated-thoughts/commit/5fb421117702782da6699465f91fd68dc98774bc))
* **tauri:** route read_document through safe_vault_path ([b8c4220](https://github.com/equationalapplications/curated-thoughts/commit/b8c4220867f16480ae726f912b5636707438cd1c))
* **tauri:** try multiple path keys for related chunks lookup ([17d9f2c](https://github.com/equationalapplications/curated-thoughts/commit/17d9f2c354315bab3efce0ca9777682216e0fef8))
* **tauri:** validate approve_wiki_page page_path through safe helper ([24fda34](https://github.com/equationalapplications/curated-thoughts/commit/24fda344a5f03726e9325dcd3d6cc1d9b96d14a0))
* **tauri:** validate copy_to_vault destination through safe helper ([4a5f161](https://github.com/equationalapplications/curated-thoughts/commit/4a5f1612299edf0a966611a09339f198a68a8599))
* **tauri:** validate get_proposed_content path through safe helper ([5c3d6cd](https://github.com/equationalapplications/curated-thoughts/commit/5c3d6cd0ea8b51fd28e752fbc19491500bbc0958))
* tighten proposed-content errors and vault path matching ([c23713c](https://github.com/equationalapplications/curated-thoughts/commit/c23713cec8086bf758089c6d2b25d223b9e04af9))
* **ui:** wiki detection uses vault-relative first segment ([89b2636](https://github.com/equationalapplications/curated-thoughts/commit/89b2636424897f779367f38655cf79cbbb9cb54b))
* upload artifacts on workflow_dispatch manual builds ([506576b](https://github.com/equationalapplications/curated-thoughts/commit/506576b265d18dd30f7987cd6193a01db9d570c2))
* use head_branch instead of head_sha to avoid detached HEAD ([d9a49c6](https://github.com/equationalapplications/curated-thoughts/commit/d9a49c6afc68936cce08548ff78a860c3eafbccf))
* **vault:** ensure MayCreate parent is directory ([b36f748](https://github.com/equationalapplications/curated-thoughts/commit/b36f7481ba086207592ae3e44f56dd53a5d157c2))
* **watcher:** enqueue normalized canonical paths ([ac7a555](https://github.com/equationalapplications/curated-thoughts/commit/ac7a5553fc373cb9fd428081be4016e45d63332a))
* wiki isWiki + MustExist regular-file guard ([0509e09](https://github.com/equationalapplications/curated-thoughts/commit/0509e09b8169721a5580824ec214f6086ebcfe8a))
* wrap schema migrations in atomic transaction ([d5a8c86](https://github.com/equationalapplications/curated-thoughts/commit/d5a8c865379022524bac48c072d6e1d1413d12ac))
