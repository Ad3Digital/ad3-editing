/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// Prepares a local version tag by bumping the single global version in the root
// and all app package.jsons, refreshing the lockfile, committing, and tagging.
// Distribution is intentionally a separate manual decision.
// Usage: npm run release <patch|minor|major|x.y.z>

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const PKGS = [
  "package.json",
  "apps/desktop/package.json",
  "apps/cli/package.json",
  "apps/web/package.json",
];

const git = (...args) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();

const arg = process.argv[2];
if (!arg) {
  console.error("Usage: npm run release <patch|minor|major|x.y.z>");
  process.exit(1);
}

if (git("status", "--porcelain")) {
  console.error("Working tree is not clean; commit or stash first.");
  process.exit(1);
}

const current = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).version;

let next;
if (/^\d+\.\d+\.\d+$/.test(arg)) {
  next = arg;
} else {
  const [major, minor, patch] = current.split(".").map(Number);
  if (arg === "major") next = `${major + 1}.0.0`;
  else if (arg === "minor") next = `${major}.${minor + 1}.0`;
  else if (arg === "patch") next = `${major}.${minor}.${patch + 1}`;
  else {
    console.error(`Unknown bump "${arg}" (expected patch, minor, major, or x.y.z)`);
    process.exit(1);
  }
}

const tag = `v${next}`;
if (git("tag", "--list", tag)) {
  console.error(`Tag ${tag} already exists.`);
  process.exit(1);
}

for (const rel of PKGS) {
  const path = join(root, rel);
  const pkg = JSON.parse(readFileSync(path, "utf8"));
  pkg.version = next;
  writeFileSync(path, JSON.stringify(pkg, null, 2) + "\n");
}

execFileSync("npm", ["install", "--package-lock-only", "--no-audit", "--no-fund"], {
  cwd: root,
  stdio: "inherit",
});

git("add", ...PKGS, "package-lock.json");
git("commit", "-m", tag);
git("tag", tag);

console.log(`\n${current} -> ${next}`);
console.log(`Release with: git push origin HEAD ${tag}`);
