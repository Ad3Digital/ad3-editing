/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// Stages the authoring docs into apps/desktop/docs so electron-forge can ship
// them as an app resource (Contents/Resources/docs). The layout mirrors the
// repo — reference/ beside examples/ — because the relative links between the
// pages assume it. scaffold() copies the tree into each project's
// .diffusion/docs, so what an agent reads in a project is exactly what was
// staged here (see src/projects.ts).

import { cpSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const desktopDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = join(desktopDir, "..", "..");
const stageDir = join(desktopDir, "docs");

rmSync(stageDir, { recursive: true, force: true });
mkdirSync(stageDir, { recursive: true });
for (const name of ["reference", "examples"]) {
  cpSync(join(repoRoot, name), join(stageDir, name), { recursive: true });
}

console.log(`stage-docs: staged docs at ${stageDir}`);
