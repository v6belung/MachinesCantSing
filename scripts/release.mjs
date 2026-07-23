#!/usr/bin/env node
// Bumps the version in package.json, src-tauri/Cargo.toml, and src-tauri/tauri.conf.json
// together (they must stay in sync -- Tauri reads its version from tauri.conf.json, not
// Cargo.toml), refreshes Cargo.lock, and creates a release commit + git tag. Pushing the
// tag (`git push origin vX.Y.Z`) triggers .github/workflows/release.yml, which builds the
// Windows installer/binary and attaches it to a GitHub Release.
import { execSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";

const version = process.argv[2];
if (!version || !/^\d+\.\d+\.\d+$/.test(version)) {
  console.error("Usage: npm run release -- <x.y.z>");
  process.exit(1);
}

function bumpJsonVersion(path) {
  const json = JSON.parse(readFileSync(path, "utf8"));
  json.version = version;
  writeFileSync(path, JSON.stringify(json, null, 2) + "\n");
}

function bumpTomlVersion(path) {
  const toml = readFileSync(path, "utf8");
  const bumped = toml.replace(/^version = "[^"]*"/m, `version = "${version}"`);
  if (bumped === toml) {
    throw new Error(`Could not find a version field to bump in ${path}`);
  }
  writeFileSync(path, bumped);
}

bumpJsonVersion("package.json");
bumpJsonVersion("src-tauri/tauri.conf.json");
bumpTomlVersion("src-tauri/Cargo.toml");

// Refresh Cargo.lock's record of our own package version.
execSync("cargo check --quiet", { cwd: "src-tauri", stdio: "inherit" });

execSync(
  "git add package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock",
  { stdio: "inherit" },
);
execSync(`git commit -m "Release v${version}"`, { stdio: "inherit" });
execSync(`git tag v${version}`, { stdio: "inherit" });

console.log(`\nTagged v${version}. Push it to trigger the release build:`);
console.log(`  git push && git push origin v${version}`);
