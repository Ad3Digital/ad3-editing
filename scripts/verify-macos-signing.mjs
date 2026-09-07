/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// Validate the distributed app, not just Forge's input configuration. A bundle
// can pass codesign --verify yet fail DYLD library validation on another Mac.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { join, resolve } from "node:path";

if (process.platform !== "darwin") throw new Error("Run on macOS against an installed app bundle.");
if (!process.argv[2]) throw new Error("Usage: node scripts/verify-macos-signing.mjs <app bundle>");
const app = resolve(process.argv[2]);
execFileSync("codesign", ["--verify", "--deep", "--strict", app], { stdio: "inherit" });
const frameworks = join(app, "Contents", "Frameworks");
const helpers = readdirSync(frameworks).filter((name) => name.endsWith(".app"));
assert(helpers.length >= 4, "Expected Electron helper bundles");
for (const bundle of [app, ...helpers.map((name) => join(frameworks, name))]) {
  // codesign writes metadata to stderr and its plist to stdout.
  const { spawnSync } = await import("node:child_process");
  const result = spawnSync("codesign", ["-dvv", "--entitlements", ":-", bundle], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  const description = result.stdout + result.stderr;
  assert.match(description, /flags=.*runtime/, "Hardened runtime must remain enabled: " + bundle);
  if (/Signature=adhoc/.test(description)) {
    assert.match(description, /<key>com\.apple\.security\.cs\.disable-library-validation<\/key>\s*<true\s*\/>/,
      "Ad-hoc Electron process lacks library-validation exception: " + bundle);
  }
}
const executable = join(app, "Contents", "MacOS", "AD3 Editing");
const version = execFileSync(executable, ["-e", "console.log(process.versions.electron)"], {
  env: { ...process.env, ELECTRON_RUN_AS_NODE: "1" }, encoding: "utf8", timeout: 30000,
}).trim();
assert.match(version, /^\d+\.\d+\.\d+/, "Packaged Electron did not launch");
console.log("Verified hardened runtime, ad-hoc process entitlements and packaged Electron " + version);
