#!/usr/bin/env node
const fs = require('fs');
const path = require('path');

const version = process.argv[2];
if (!version) {
  console.error('Usage: node update-versions.js <version>');
  process.exit(1);
}

// Update Cargo.toml
const cargoPath = path.join(__dirname, '..', 'src-tauri', 'Cargo.toml');
let cargoContent = fs.readFileSync(cargoPath, 'utf8');
cargoContent = cargoContent.replace(
  /^version\s*=\s*"[^"]*"/m,
  `version = "${version}"`
);
fs.writeFileSync(cargoPath, cargoContent);
console.log(`Updated Cargo.toml to ${version}`);

// Update tauri.conf.json
const tauriConfPath = path.join(__dirname, '..', 'src-tauri', 'tauri.conf.json');
const tauriConf = JSON.parse(fs.readFileSync(tauriConfPath, 'utf8'));
tauriConf.version = version;
fs.writeFileSync(tauriConfPath, JSON.stringify(tauriConf, null, 2) + '\n');
console.log(`Updated tauri.conf.json to ${version}`);

process.exit(0);
