import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtempSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
const require = createRequire(import.meta.url);
const output = join(mkdtempSync(join(tmpdir(), 'ad3-native-client-')), 'client.cjs');
await require('esbuild').build({ entryPoints: [fileURLToPath(new URL('../packages/runtime/src/media/native-video.ts', import.meta.url))], outfile: output, bundle: true, platform: 'node', format: 'cjs' });
const { NativeVideoBuffer } = require(output);
test('native client bounds requests, ignores obsolete scrubs and cancels on idle', async () => {
  let draws = 0, closed = 0;
  globalThis.OffscreenCanvas = class { constructor(width, height) { Object.assign(this, { width, height }); } getContext() { return { putImageData() { draws++; } }; } };
  globalThis.ImageData = class {};
  const requests = [];
  globalThis.ad3NativePreview = {
    frame: request => new Promise(resolve => requests.push({ request, resolve })),
    close: async () => { closed++; },
  };
  const decoder = new NativeVideoBuffer({ duration: 1000, frameRate: 60, width: 1920, height: 1080 }, 'C:/video.mp4');
  decoder.seekTo(0, 30);
  for (let i = 1; i <= 300; i++) decoder.seekTo(i, 30);
  assert.equal(requests.length, 1);
  requests[0].resolve({ time: 0, width: 2, height: 2, pixels: new Uint8Array(16) });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(draws, 0, 'an old scrub must not flash after a large jump');
  decoder.nextReadAt = 0;
  decoder.seekTo(300, 30);
  assert.equal(requests[1].request.time, 10);
  requests[1].resolve({ time: 10, width: 2, height: 2, pixels: new Uint8Array(16) });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(draws, 1);
  decoder.nextReadAt = 0; decoder.seekTo(301, 30);
  decoder.idle(); assert.equal(closed, 1);
  requests[2].resolve({ time: 10.0333, width: 2, height: 2, pixels: new Uint8Array(16) });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(draws, 1);
  decoder.dispose(); assert.equal(decoder.toBitmap(), null);
  delete globalThis.ad3NativePreview;
});
