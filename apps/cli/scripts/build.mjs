/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { build } from "esbuild";
import { chmod } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const outfile = join(root, "dist", "index.js");

await build({
  entryPoints: [join(root, "src", "index.ts")],
  bundle: true,
  platform: "node",
  format: "cjs",
  external: [
    "esbuild",
    "@babel/core",
    "@babel/preset-typescript",
    "babel-preset-solid",
    "bufferutil",
    "utf-8-validate",
  ],
  outfile,
});

if (process.platform !== "win32") await chmod(outfile, 0o755);
