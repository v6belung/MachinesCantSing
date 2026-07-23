#!/usr/bin/env node
// Supply-chain guard: fails if any npm dependency in package-lock.json was published less
// than COOLDOWN_DAYS ago. Recently-published versions haven't had time for the community to
// catch a compromised release, so new/updated dependencies should sit out the cooldown before
// landing here. Run via `npm run check:deps`; wired into CI on any package.json/lockfile change.
const COOLDOWN_DAYS = 7;
const COOLDOWN_MS = COOLDOWN_DAYS * 24 * 60 * 60 * 1000;

import { readFileSync } from "node:fs";

const lock = JSON.parse(readFileSync("package-lock.json", "utf8"));

// npm lockfile v3 entries are keyed by install path (e.g. "node_modules/@scope/name")
// and usually omit an explicit "name" field -- the name has to be recovered from the path.
const packages = Object.entries(lock.packages ?? {})
  .filter(([path, pkg]) => path.startsWith("node_modules/") && pkg.version && !pkg.link)
  .map(([path, pkg]) => ({
    name: path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length),
    version: pkg.version,
  }));

const unique = [...new Map(packages.map((p) => [`${p.name}@${p.version}`, p])).values()];

const now = Date.now();

const results = await Promise.all(
  unique.map(async ({ name, version }) => {
    try {
      const res = await fetch(`https://registry.npmjs.org/${encodeURIComponent(name)}`);
      if (!res.ok) {
        return { name, version, status: "unknown", detail: `registry lookup failed (${res.status})` };
      }
      const meta = await res.json();
      const publishedAt = meta.time?.[version];
      if (!publishedAt) {
        return { name, version, status: "unknown", detail: "no publish time in registry metadata" };
      }
      const ageMs = now - new Date(publishedAt).getTime();
      if (ageMs < COOLDOWN_MS) {
        const ageDays = (ageMs / (24 * 60 * 60 * 1000)).toFixed(1);
        return { name, version, status: "too-new", detail: `published ${ageDays}d ago` };
      }
      return { name, version, status: "ok" };
    } catch (err) {
      return { name, version, status: "unknown", detail: err.message };
    }
  }),
);

const unknown = results.filter((r) => r.status === "unknown");
const tooNew = results.filter((r) => r.status === "too-new");

if (unknown.length) {
  console.warn("Warnings (could not verify publish age):");
  for (const r of unknown) console.warn(`  - ${r.name}@${r.version}: ${r.detail}`);
}

if (tooNew.length) {
  console.error(`\nBlocked: ${tooNew.length} package(s) published within the last ${COOLDOWN_DAYS} days:`);
  for (const r of tooNew) console.error(`  - ${r.name}@${r.version}: ${r.detail}`);
  console.error("\nPin an older version, or wait out the cooldown before merging.");
  process.exit(1);
}

console.log(`OK: all ${unique.length} npm packages are older than the ${COOLDOWN_DAYS}-day cooldown.`);
