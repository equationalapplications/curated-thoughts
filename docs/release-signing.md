# Release code signing

`build.yml` signs release builds on tag runs. Each platform's signing switches
on only when its secrets exist, so the pipeline keeps producing unsigned
builds until the accounts below are set up. Once a platform is configured, a
verify step fails the release if any artifact comes out unsigned. The release
is created as a DRAFT — semantic-release (`draftRelease: true`) on the main
path, `tauri-action` on a human-pushed tag — and the `publish-release` job
flips it public only after every build leg verified and uploaded, so a failure
leaves a draft, never a public unsigned release. Residual exposure accepted on
signed Windows runs: the Azure client secret is present in the Tauri build env
(reaching `beforeBuildCommand` and build scripts) because `signCommand` runs
inside the bundler — trusted first-party code only, and the secret is gated to
Windows legs where `WIN_SIGNING` is true.

| Platform | What gets signed | How |
|---|---|---|
| macOS | `.app` (+ bundled MCP sidecar), `.dmg`, `ct` CLI | Developer ID Application cert, hardened runtime, notarized via App Store Connect API key; the app is stapled. The `.dmg` is codesigned, not stapled (Tauri bundler behavior) — the stapled ticket rides on the `.app` inside |
| Windows | app exe, MCP sidecar, `.msi`, NSIS `.exe`, `ct.exe` | Azure Artifact Signing via `artifact-signing-cli` (Tauri `signCommand`) |
| Linux | — | Nothing to sign for GitHub Releases; Flathub and the Snap Store sign on their side |

Distribution decision (2026-11): macOS ships as a notarized download, not via
the Mac App Store. The App Sandbox conflicts with `~/.brain`, arbitrary vault
watching, starting Ollama, and an MCP sidecar launched by outside clients.

## macOS setup (Apple Developer account, Account Holder role)

1. **Developer ID Application certificate.** On a Mac: Keychain Access →
   Certificate Assistant → Request a Certificate From a Certificate
   Authority (save to disk). Then developer.apple.com → Certificates → **+** →
   *Developer ID Application* → upload the CSR → download and double-click
   the `.cer`. In Keychain Access, export the certificate *with its private
   key* as `.p12` and set a password.
2. **Notarization API key.** App Store Connect → Users and Access →
   Integrations → App Store Connect API → Team Keys → **+**, access
   *Developer*. Download the `.p8` (it can only be downloaded once). Note the
   Key ID and the Issuer ID.
3. **GitHub secrets** (repo → Settings → Secrets and variables → Actions):

   | Secret | Value |
   |---|---|
   | `APPLE_CERTIFICATE` | `base64 -i DeveloperID.p12 \| pbcopy` |
   | `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
   | `APPLE_API_PRIVATE_KEY` | full contents of `AuthKey_XXXX.p8` |
   | `APPLE_API_KEY_ID` | Key ID |
   | `APPLE_API_ISSUER` | Issuer ID |

## Windows setup (Azure Artifact Signing, ~$9.99/month Basic)

1. Azure portal → create an **Artifact Signing account** (Basic SKU). The
   region sets the endpoint, e.g. `https://eus.codesigning.azure.net` (East US).
2. On the account, grant yourself **Artifact Signing Identity Verifier**, then
   create an **identity validation**: Public Trust, Organization =
   Equational Applications LLC. Microsoft may require several years of
   business history for organizations. If EqApp doesn't qualify, an
   *Individual* validation works for US developers, but the certificate then
   shows your personal name.
3. Once validated, create a **certificate profile** (Public Trust).
4. Entra ID → App registrations → new app → Certificates & secrets → new
   client secret. On the Artifact Signing account, grant that app
   **Artifact Signing Certificate Profile Signer**.
5. **GitHub secrets:** `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`,
   `AZURE_TENANT_ID`.
   **GitHub variables** (not secrets): `AZURE_SIGNING_ENDPOINT`,
   `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE`.

The certificate subject must stay the same across releases, because SmartScreen
reputation follows it.

## Follow-ups not done here

- Microsoft Store listing: Partner Center → *EXE or MSI app*, pointing at the
  versioned signed `.msi` on GitHub Releases. The Store expects the WebView2
  `offlineInstaller` install mode.
- Flathub / Snap packaging (manifests and home-folder permissions).
- Replace the Azure client secret with GitHub OIDC federation once the
  signing client supports it (see `OIDC_SETUP.md`).
- macOS CLI install path for `ct` (symlink from the app bundle, or a
  Homebrew tap/cask).
