# Contributing to Curated Thoughts

## Conventional Commits

All commits to `main` must follow [Conventional Commits](https://www.conventionalcommits.org/) format:

```
type(scope): description

Optional body explaining the change.

Optional footer: BREAKING CHANGE: description
```

**Commit types and effects:**
- `feat:` → minor version bump
- `fix:` → patch version bump
- `BREAKING CHANGE:` → major version bump
- `chore:`, `docs:`, `style:`, `refactor:`, `test:` → no release

**Examples:**
- ✅ `feat: add wiki fact extraction to chunking pipeline`
- ✅ `fix: prevent path traversal in vault access`
- ✅ `docs: update README installation steps`
- ❌ `updated stuff` (missing type)
- ❌ `feat: added and fixed and refactored` (too vague, multiple concerns)

## Release Process

Releases happen automatically when commits are pushed to `main`:

1. Semantic-release analyzes your commits
2. Version bumps (if applicable)
3. Git tag created (e.g., `v0.2.0`)
4. GitHub Release published with changelog
5. Desktop builds generated (macOS .dmg, Linux AppImage)

Artifacts available on [Releases page](https://github.com/equationalapplications/curated-thoughts/releases).

## Local Development

**macOS unsigned builds:** Right-click → Open to bypass Gatekeeper warning on first launch.

## Before Submitting

1. Run tests: `npm test`
2. Build locally: `npm run tauri build`
3. Verify commit message format
4. One feature or fix per commit (or group logically related changes)

## Security

Release workflows use OpenID Connect (OIDC) for authentication instead of stored tokens. See [OIDC_SETUP.md](OIDC_SETUP.md) if you're interested in setting up registry publishing (npm, Crates.io).

## Troubleshooting

**Release workflow fails:**
- Check the [Actions tab](https://github.com/equationalapplications/curated-thoughts/actions) for error logs
- Verify your commit messages follow conventional commit format
- Ensure all tests pass locally before pushing

**Manual re-trigger build workflow:**
- Go to [Actions](https://github.com/equationalapplications/curated-thoughts/actions)
- Select "Build" workflow
- Click "Run workflow" → select tag → "Run workflow"

**Check workflow logs:**
- Navigate to [Actions](https://github.com/equationalapplications/curated-thoughts/actions)
- Click on the failed workflow run
- Expand steps to view detailed error messages
