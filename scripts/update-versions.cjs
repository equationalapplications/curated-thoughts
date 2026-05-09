#!/usr/bin/env node
const fs = require('fs');
const path = require('path');

const version = process.argv[2];
if (!version || !/^\d+\.\d+\.\d+/.test(version)) {
  console.error('Usage: node update-versions.cjs <version>');
  console.error('Version must be in semver format (e.g., 1.0.0)');
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
