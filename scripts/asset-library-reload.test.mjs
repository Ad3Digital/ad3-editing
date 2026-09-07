/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const requireDesktop = createRequire(new URL("../apps/desktop/package.json", import.meta.url));
const { build } = requireDesktop("esbuild");
let temporary;
let AssetLibrary;

before(async () => {
  temporary = await mkdtemp(join(tmpdir(), "ad3-library-reload-"));
  const output = join(temporary, "library.cjs");
  await build({
    entryPoints: [fileURLToPath(new URL("../packages/assets/src/library.ts", import.meta.url))],
    outfile: output, bundle: true, platform: "node", format: "cjs", logLevel: "silent",
  });
  ({ AssetLibrary } = requireDesktop(output));
});

after(async () => { if (temporary) await rm(temporary, { recursive: true, force: true }); });

test("an in-flight manifest reload preserves a newly assigned generated-media link", async () => {
  let disk = {
    version: 1, folders: [], assets: [{
      id: "render-content", path: "Titles/title.mp4", source: "assets/title.mp4",
      createdAt: "2026-01-01T00:00:00.000Z", type: "VIDEO", mimeType: "video/mp4",
      width: 320, height: 180, frameRate: 24, duration: 1.5,
    }],
  };
  const entered = Promise.withResolvers();
  const release = Promise.withResolvers();
  let holdNextRead = false;
  const fs = {
    async readManifest() {
      const snapshot = structuredClone(disk);
      if (holdNextRead) {
        holdNextRead = false;
        entered.resolve();
        await release.promise;
      }
      return snapshot;
    },
    async writeManifest(manifest) { disk = structuredClone(manifest); },
    async stat() { return null; },
    async list() { return []; },
  };
  const library = new AssetLibrary(fs);
  try {
    await library.load();
    holdNextRead = true;
    const reloading = library.load();
    await entered.promise;
    const generation = { key: "hyperframes", id: "title-composition" };
    library.update(library.get("render-content"), { generation });
    release.resolve();
    await reloading;
    assert.deepEqual(library.get("Titles/title.mp4").generation, generation);
    await library.flush();
    assert.deepEqual(disk.assets[0].generation, generation);
  } finally {
    release.resolve();
    await library.dispose();
  }
});
