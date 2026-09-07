/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// Electron Forge's app-bundle pass does not sign arbitrary binaries staged
// beneath Contents/Resources. Sign the local CLI and HyperFrames runtime before
// packaging, using a Developer ID only when its name is explicitly provided.
import { closeSync, openSync, readSync, readdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

if (process.platform !== "darwin") process.exit(0);

const desktopDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const resourceDirs = [join(desktopDir, "cli"), join(desktopDir, "hyperframes-engine")];
const identity = process.env.APPLE_SIGNING_IDENTITY ?? "-";
const MACH_O_MAGIC = new Set([
  0xfeedface,
  0xcefaedfe,
  0xfeedfacf,
  0xcffaedfe,
  0xcafebabe,
  0xbebafeca,
  0xcafebabf,
  0xbfbafeca,
]);

function isMachO(path) {
  const header = Buffer.allocUnsafe(4);
  const file = openSync(path, "r");
  try {
    return readSync(file, header, 0, header.length, 0) === header.length && MACH_O_MAGIC.has(header.readUInt32BE());
  } finally {
    closeSync(file);
  }
}

function machOBinaries(dir) {
  const binaries = [];
  for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      binaries.push(...machOBinaries(path));
      continue;
    }
    if (entry.isFile() && isMachO(path)) binaries.push(path);
  }
  return binaries;
}

function appBundles(dir) {
  const bundles = [];
  for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    if (!entry.isDirectory()) continue;
    const path = join(dir, entry.name);
    if (entry.name.endsWith(".app")) bundles.push(path);
    bundles.push(...appBundles(path));
  }
  return bundles;
}

const signArgs = ["--force", "--sign", identity];
if (identity !== "-") signArgs.unshift("--timestamp", "--options", "runtime");
for (const dir of resourceDirs) {
  for (const binary of machOBinaries(dir)) {
    execFileSync("codesign", [...signArgs, binary], { stdio: "inherit" });
  }
  // Chromium is distributed as a nested .app. Re-sign its enclosing bundle
  // after the leaves so macOS sees a valid CodeResources manifest at launch.
  for (const appBundle of appBundles(dir).sort((a, b) => b.length - a.length)) {
    execFileSync("codesign", [...signArgs, "--deep", appBundle], { stdio: "inherit" });
  }
}
