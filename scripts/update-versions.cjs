#!/usr/bin/env node
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const version = process.argv[2];
// Accept full semver: x.y.z with optional pre-release and build metadata
// Examples: 1.0.0, 1.0.0-beta.1, 1.0.0+build.5, 1.0.0-rc.1+build.123
if (!version || !/^\d+\.\d+\.\d+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$/.test(version)) {
  console.error('Usage: node scripts/update-versions.cjs <version>');
  console.error('Version must be in semver format (e.g., 1.0.0, 1.0.0-beta.1, 1.0.0+build.5)');
  process.exit(1);
}

try {
  function verifyLockVersion(lockfilePath, expectedVersion, label) {
    const lockContent = fs.readFileSync(lockfilePath, 'utf8');
    const packageMatch = lockContent.match(/\[\[package\]\]\r?\n\s*name\s*=\s*"curated-thoughts"\r?\n\s*version\s*=\s*"([^"]+)"/);

    if (!packageMatch || packageMatch[1] !== expectedVersion) {
      const foundVersion = packageMatch ? packageMatch[1] : 'not found';
      throw new Error(
        `${label} verification failed: expected version ${expectedVersion}, found ${foundVersion}. ` +
        `cargo metadata may not have updated the lockfile correctly.`
      );
    }
  }

  // Update Cargo.toml
  const cargoPath = path.join(__dirname, '..', 'src-tauri', 'Cargo.toml');
  let cargoContent = fs.readFileSync(cargoPath, 'utf8');
  const updated = cargoContent.replace(
    /^version\s*=\s*"[^"]*"/m,
    `version = "${version}"`
  );
  if (updated === cargoContent) {
    throw new Error('Failed to update version in Cargo.toml - no match found');
  }
  fs.writeFileSync(cargoPath, updated);
  console.log(`Updated Cargo.toml to ${version}`);

  // Update the workspace Cargo.lock to reflect the new package version (without upgrading
  // dependencies). Using cargo metadata instead of cargo update to avoid semver dependency
  // upgrades. src-tauri and tools are members of one workspace, so there is a single root
  // lockfile and the tools path dependency stays in sync automatically.
  const workspaceDir = path.join(__dirname, '..');
  const cargoLockPath = path.join(workspaceDir, 'Cargo.lock');

  execSync('cargo metadata --format-version 1', { cwd: workspaceDir, stdio: ['ignore', 'ignore', 'inherit'] });

  verifyLockVersion(cargoLockPath, version, 'Cargo.lock');

  console.log(`Updated and verified Cargo.lock package version to ${version}`);

  // Update tauri.conf.json
  const tauriConfPath = path.join(__dirname, '..', 'src-tauri', 'tauri.conf.json');
  const tauriConf = JSON.parse(fs.readFileSync(tauriConfPath, 'utf8'));
  tauriConf.version = version;
  fs.writeFileSync(tauriConfPath, JSON.stringify(tauriConf, null, 2) + '\n');
  console.log(`Updated tauri.conf.json to ${version}`);

  process.exit(0);
} catch (error) {
  console.error(`Error updating versions: ${error.message}`);
  process.exit(1);
}
