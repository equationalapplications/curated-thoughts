# OpenID Connect (OIDC) Setup for GitHub Actions

This guide explains how OIDC works in your release and build workflows, and how to configure it. OIDC replaces stored credentials with short-lived, token-based authentication.

## Why OIDC?

**Before OIDC:**
```
Workflow needs to upload to GitHub → Store GITHUB_TOKEN secret in repo → Anyone with repo access can see/misuse token
```

**With OIDC:**
```
Workflow requests token from GitHub → GitHub issues short-lived token → Token auto-expires → Safer
```

No secrets stored. No token rotation needed.

## How It Works in Your Workflows

### Current Setup

Both workflows now include an OIDC token step:

```yaml
- name: Get OIDC token for GitHub
  id: github-token
  uses: actions/github-script@v7
  with:
    script: |
      const token = await core.getIDToken('https://github.com');
      core.setOutput('token', token);

- name: Run semantic-release
  env:
    GITHUB_TOKEN: ${{ steps.github-token.outputs.token }}
  run: npx semantic-release
```

**What happens:**
1. Workflow requests token for `https://github.com`
2. GitHub issues short-lived OIDC token (valid ~5 minutes)
3. Token passed to semantic-release via `GITHUB_TOKEN` env var
4. semantic-release authenticates with GitHub using token
5. Token auto-expires, no cleanup needed

## No Setup Required for GitHub Releases

Your workflows already work. GitHub Actions automatically:
- Recognizes `id-token: write` permission
- Issues tokens on request
- Validates them

**You're done.** Workflows use OIDC now.

## Future: Publishing to npm or Crates.io

To publish packages to npm or Crates.io registries using OIDC, follow these steps:

### Step 1: Configure npm Trust Relationship

**On npm (for your account or organization):**

1. Go to [npmjs.com](https://npmjs.com) → Account Settings → Publishing → Trusted Publishers
2. Click "Add trusted publisher" and configure:
   - **Provider:** GitHub Actions
   - **Repository:** `equationalapplications/curated-thoughts`
   - **Workflow file:** `.github/workflows/publish-npm.yml`
   - **Environment name:** `npm` (optional)

**Note:** OIDC trusted publishing replaces the need for manual access tokens. No token creation step needed.

### Step 2: Create Publish Workflow

Create `.github/workflows/publish-npm.yml`:

```yaml
name: Publish to npm

on:
  push:
    tags:
      - "v*"

permissions:
  contents: read
  id-token: write

jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6

      - uses: actions/setup-node@v6
        with:
          node-version: 24
          registry-url: https://registry.npmjs.org/

      - run: npm ci

      - name: Get OIDC token for npm
        id: npm-token
        uses: actions/github-script@v7
        with:
          script: |
            const token = await core.getIDToken('https://registry.npmjs.org/');
            core.setOutput('token', token);

      - name: Publish to npm
        env:
          NODE_AUTH_TOKEN: ${{ steps.npm-token.outputs.token }}
        run: npm publish
```

**Key parts:**
- `registry-url: https://registry.npmjs.org/` — tells Node to use npm registry
- `core.getIDToken('https://registry.npmjs.org/')` — requests token scoped to npm
- `NODE_AUTH_TOKEN` — npm recognizes this env var for authentication

### Step 3: Configure Crates.io Trust Relationship (for Rust)

**On Crates.io (for your account):**

1. Go to [crates.io](https://crates.io) → Account Settings → Manage Tokens → "Authorize GitHub"
2. Add trusted GitHub Actions:
   - **Repository:** `equationalapplications/curated-thoughts`
   - **Workflow file:** `.github/workflows/publish-crates.yml`
   - **Ref:** `refs/tags/v*` (triggers on version tags)

**Note:** OIDC trusted publishing replaces the need for manual API tokens. No token creation step needed.

### Step 4: Create Crates.io Publish Workflow

Create `.github/workflows/publish-crates.yml`:

```yaml
name: Publish to Crates.io

on:
  push:
    tags:
      - "v*"

permissions:
  contents: read
  id-token: write

jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6

      - uses: dtolnay/rust-toolchain@stable

      - name: Get OIDC token for Crates.io
        id: crates-token
        uses: actions/github-script@v7
        with:
          script: |
            const token = await core.getIDToken('https://crates.io');
            core.setOutput('token', token);

      - name: Publish to Crates.io
        env:
          CARGO_REGISTRY_TOKEN: ${{ steps.crates-token.outputs.token }}
        run: cargo publish --manifest-path src-tauri/Cargo.toml
```

**Key parts:**
- `core.getIDToken('https://crates.io')` — token scoped to Crates.io
- `CARGO_REGISTRY_TOKEN` — Cargo recognizes this env var

## Troubleshooting

**"401 Unauthorized" on publish:**
- Check that the trusted publisher config matches your workflow file path exactly
- Verify token request domain matches the registry domain
- On npm: confirm third-party publishing is enabled in Account Settings

**"OIDC token not available":**
- Ensure `id-token: write` permission is set in workflow
- Token is only available inside GitHub Actions; won't work locally

**"Repository not recognized":**
- Trusted publisher config must have exact repo name: `equationalapplications/curated-thoughts`
- Check spelling and casing

## Local Testing

OIDC tokens only work in GitHub Actions. To test locally:

```bash
# Won't work locally (no token):
npm publish

# Work around locally with personal access token:
npm config set //registry.npmjs.org/:_authToken=your-pat-token
npm publish
```

## Security Notes

- **Tokens expire in ~5 minutes** — no manual cleanup needed
- **No secrets stored** in GitHub repo settings
- **Audience restriction** — tokens only work for the specified registry (npm vs Crates.io)
- **Workflow file specificity** — trusted publishers are bound to specific workflow files, preventing misuse

## References

- [GitHub OIDC documentation](https://docs.github.com/en/actions/deployment/security-hardening-your-deployments/about-security-hardening-with-openid-connect)
- [npm trusted publishing](https://docs.npmjs.com/cli/v10/using-npm/config#registry)
- [Crates.io trusted publishing](https://doc.rust-lang.org/cargo/registries.html)
