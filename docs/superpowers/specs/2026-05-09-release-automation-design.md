# Release Automation & Desktop Build Design

## Overview

Automated release workflow using semantic-release for version management and GitHub Actions for building unsigned macOS and Linux desktop applications.

## Goals

1. Automatic version tagging based on conventional commits
2. Build unsigned Tauri desktop apps (macOS .dmg, Linux AppImage)
3. Upload artifacts to GitHub Releases
4. Document contribution workflow for external contributors
5. Add status badges to README

## Architecture

Two-workflow approach with tag-based triggering:

### Workflow 1: Semantic Release (`.github/workflows/release.yml`)

**Trigger:** Push to `main` branch

**Purpose:** Analyze conventional commits, bump versions, create git tag and GitHub Release

**Steps:**
1. Checkout with full git history (semantic-release needs commit history)
2. Setup Node.js environment
3. Install semantic-release plugins:
   - `@semantic-release/commit-analyzer` — parse conventional commits
   - `@semantic-release/release-notes-generator` — generate changelog
   - `@semantic-release/changelog` — update CHANGELOG.md
   - `@semantic-release/npm` — bump package.json (skip publish to npm)
   - `@semantic-release/exec` — run custom script to sync versions across files
   - `@semantic-release/git` — commit version changes back to main
   - `@semantic-release/github` — create GitHub Release
4. Run semantic-release with configuration that updates:
   - `package.json` version field (via @semantic-release/npm)
   - `src-tauri/Cargo.toml` version field (via custom script)
   - `src-tauri/tauri.conf.json` version field (via custom script)
   - Creates/updates `CHANGELOG.md`
5. Commit version bump changes back to main with `[skip ci]` to prevent workflow loop

**Permissions required:**
- `contents: write` — for creating commits and tags
- `issues: write` — for semantic-release GitHub plugin
- `pull-requests: write` — for semantic-release GitHub plugin

**Output:** New git tag (e.g., `v0.2.0`), empty GitHub Release created, version files synchronized

### Workflow 2: Build & Upload (`.github/workflows/build.yml`)

**Trigger:** Tag push matching pattern `v*`

**Purpose:** Build unsigned Tauri desktop applications and upload to GitHub Release

**Matrix strategy:**
- `macos-latest` — macOS universal build (x86_64 + aarch64)
- `ubuntu-22.04` — Linux AppImage build (x86_64)

**Steps per platform:**
1. Checkout repository at tag ref
2. Install platform-specific dependencies:
   - **macOS:** Rust toolchain only (Tauri CLI handles Xcode dependencies)
   - **Linux:** webkit2gtk-4.1-dev, libappindicator3-dev, librsvg2-dev, patchelf, pkg-config, libssl-dev
3. Setup Node.js and install npm dependencies
4. Setup Rust stable toolchain
5. Build Tauri application: `npm run tauri build`
   - macOS produces: universal `.dmg` (unsigned, will show Gatekeeper warning on first launch)
   - Linux produces: `.AppImage` (portable executable, no signing needed)
6. Upload build artifacts to GitHub Release using `softprops/action-gh-release`

**Permissions required:**
- `contents: write` — for uploading assets to GitHub Release

**Artifacts produced:**
- `Curated-Thoughts_<version>_universal.dmg` — macOS installer
- `curated-thoughts_<version>_amd64.AppImage` — Linux portable app

## Release Flow

```
Developer commits to main
        ↓
Conventional commit analyzed by semantic-release
        ↓
Version bumped in 3 files (package.json, Cargo.toml, tauri.conf.json)
        ↓
Git tag created (e.g., v0.2.0)
        ↓
Empty GitHub Release published
        ↓
Tag push triggers build workflow
        ↓
macOS + Linux builds run in parallel
        ↓
Artifacts uploaded to existing GitHub Release
```

## Conventional Commits & Versioning

### Commit Format

Conventional commits determine version increments:

- `feat: <description>` → **minor** version bump (0.1.0 → 0.2.0)
- `fix: <description>` → **patch** version bump (0.1.0 → 0.1.1)
- `BREAKING CHANGE:` in commit footer → **major** version bump (0.1.0 → 1.0.0)
- `chore:`, `docs:`, `style:`, `refactor:`, `test:` → **no release**

### Version Synchronization

Semantic-release maintains version consistency across three files:

1. `package.json` → `"version": "0.2.0"`
2. `src-tauri/Cargo.toml` → `version = "0.2.0"`
3. `src-tauri/tauri.conf.json` → `"version": "0.2.0"`

Git tag is the single source of truth. Files updated to match tag version.

### No-Release Scenarios

- Only `chore:` or `docs:` commits since last release → workflow runs but skips release creation
- No conventional commits → no version bump, no tag, no build triggered

## Documentation

### CONTRIBUTORS.md

New file at repository root documenting contribution workflow.

**Sections:**

1. **Conventional Commit Requirement**
   - Explain format: `type(scope): description`
   - List commit types and version bump effects
   - Provide good/bad commit message examples
   - Link to conventionalcommits.org specification

2. **Release Workflow Overview**
   - Automatic releases on main branch push
   - How semantic-release analyzes commits
   - Build artifacts produced (macOS .dmg, Linux AppImage)
   - Where releases are published (GitHub Releases tab)

3. **Local Development**
   - Testing builds locally: `npm run tauri build`
   - macOS unsigned build requires Gatekeeper override (Right-click → Open)
   - Validating commit messages before pushing
   - Running tests before submitting PRs

4. **Troubleshooting**
   - What to do when release workflow fails
   - How to manually re-trigger build workflow from Actions tab
   - Where to check workflow logs for errors

**Tone:** Concise and example-driven. Target audience: external contributors unfamiliar with semantic-release.

### README.md Badges

Add status badges at top of README.md (before title):

**Badges included:**

1. **GitHub Release** — Shows latest semantic-release version
2. **CI Status** — Shows if CI build is passing or failing
3. **GitHub Downloads** — Total download count from releases
4. **License** — Project license (MIT or current)
5. **macOS Platform** — Indicates macOS support
6. **Linux Platform** — Indicates Linux support

**Not including NPM version badge:** Project is a desktop application, not published to npm registry. The `package.json` file only manages frontend dependencies.

**Badge order and layout:**

```markdown
[![GitHub Release](https://img.shields.io/github/v/release/equationalapplications/curated-thoughts)](https://github.com/equationalapplications/curated-thoughts/releases)
[![CI](https://github.com/equationalapplications/curated-thoughts/actions/workflows/release.yml/badge.svg)](https://github.com/equationalapplications/curated-thoughts/actions/workflows/release.yml)
[![Downloads](https://img.shields.io/github/downloads/equationalapplications/curated-thoughts/total)](https://github.com/equationalapplications/curated-thoughts/releases)
[![License](https://img.shields.io/github/license/equationalapplications/curated-thoughts)](LICENSE)
[![macOS](https://img.shields.io/badge/macOS-supported-success)](https://github.com/equationalapplications/curated-thoughts/releases)
[![Linux](https://img.shields.io/badge/Linux-supported-success)](https://github.com/equationalapplications/curated-thoughts/releases)
```

Badges generated using shields.io. Links point to Releases page, Actions tab, and license file.

## Technical Details

### GitHub Authentication

Both workflows use the default GitHub token (`secrets.GITHUB_TOKEN`) automatically provided by GitHub Actions:

**Release Workflow:**
```yaml
- name: Run semantic-release
  env:
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
  run: npx semantic-release
```

**Build Workflow:**
The `softprops/action-gh-release` action automatically uses the default token when no explicit token is provided.

**Benefits:**
- No stored secrets in repository settings
- Tokens are automatically scoped to the workflow run
- No additional configuration required
- Standard GitHub Actions pattern

**OIDC for registry publishing:** OpenID Connect tokens can be used for authenticated publishing to npm registry and Crates.io. See `OIDC_SETUP.md` for npm/Crates.io OIDC setup instructions when publishing packages.

### Semantic-Release Configuration

Configuration file: `.releaserc.json`

```json
{
  "branches": ["main"],
  "plugins": [
    "@semantic-release/commit-analyzer",
    "@semantic-release/release-notes-generator",
    "@semantic-release/changelog",
    ["@semantic-release/npm", { "npmPublish": false }],
    [
      "@semantic-release/exec",
      {
        "prepareCmd": "node scripts/update-versions.cjs ${nextRelease.version}"
      }
    ],
    [
      "@semantic-release/git",
      {
        "assets": [
          "package.json",
          "src-tauri/Cargo.toml",
          "src-tauri/tauri.conf.json",
          "CHANGELOG.md"
        ],
        "message": "chore(release): ${nextRelease.version} [skip ci]\n\n${nextRelease.notes}"
      }
    ],
    "@semantic-release/github"
  ]
}
```

**Custom version sync script:** The `@semantic-release/exec` plugin runs a custom Node.js script (`scripts/update-versions.cjs`) to synchronize version numbers across Cargo.toml and tauri.conf.json. This approach provides more control than the `semantic-release-cargo` plugin and allows for Tauri-specific configuration updates beyond just the version field.

### CI/CD Permissions

**Semantic Release Workflow (.github/workflows/release.yml):**
```yaml
permissions:
  contents: write      # Create commits and tags
  issues: write        # semantic-release GitHub plugin
  pull-requests: write # semantic-release GitHub plugin
  id-token: write      # Reserved for future OIDC registry publishing
```

**Build Workflow (.github/workflows/build.yml):**
```yaml
permissions:
  contents: write # Upload release artifacts
```

**Repository Settings:**
- No additional secrets configuration needed
- Default GitHub token (`secrets.GITHUB_TOKEN`) is automatically provided
- Workflows are self-contained; no stored credentials to manage

### Unsigned Build Distribution

**macOS:**
- `.dmg` files are unsigned and will trigger Gatekeeper on first launch
- Users must right-click → Open to bypass warning
- For production: add code signing certificate and notarization step

**Linux:**
- `.AppImage` files are portable executables requiring no installation
- No code signing required for Linux distribution
- Users may need to `chmod +x` the AppImage file

### Future Enhancements

Scope limited to unsigned prototype builds. Future work:

1. **Code signing:** Add macOS Developer ID signing + notarization
2. **Windows builds:** Add Windows .msi with optional code signing certificate
3. **Auto-update:** Integrate Tauri updater for in-app update checks
4. **Beta/canary channels:** Support pre-release distributions
5. **Commit message validation:** Add commitlint to enforce conventional commits in PRs

## Testing Plan

Before merging implementation:

1. **Test semantic-release dry-run:**
   ```bash
   npx semantic-release --dry-run
   ```

2. **Test local Tauri builds:**
   ```bash
   npm run tauri build
   ```
   Verify macOS .dmg and Linux AppImage are produced

3. **Test workflow in fork:**
   - Create fork with workflows enabled
   - Push feat commit to main
   - Verify tag creation and release
   - Verify build workflow triggers and uploads artifacts

4. **Validate CONTRIBUTORS.md:**
   - Review with external contributor mindset
   - Ensure all steps are clear and actionable

5. **Verify README badges:**
   - Check badge rendering on GitHub
   - Ensure all links point to correct pages

## Success Criteria

- [ ] Push to main with `feat:` commit creates new release
- [ ] Version synchronized across package.json, Cargo.toml, tauri.conf.json
- [ ] Git tag created matching semantic version
- [ ] GitHub Release created with changelog
- [ ] macOS .dmg uploaded to release
- [ ] Linux AppImage uploaded to release
- [ ] CONTRIBUTORS.md explains conventional commits clearly
- [ ] README badges display correctly
- [ ] No release created for `chore:` or `docs:` commits
- [ ] Build workflow can be manually re-triggered from Actions tab
