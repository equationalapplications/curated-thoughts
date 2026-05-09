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

  // Regenerate Cargo.lock (only update curated-thoughts package, not deps)
  const cargoDir = path.join(__dirname, '..', 'src-tauri');
  execSync('cargo update -p curated-thoughts', { cwd: cargoDir, stdio: 'inherit' });
  console.log(`Regenerated Cargo.lock`);

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
