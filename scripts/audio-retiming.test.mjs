/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(new URL('../apps/desktop/package.json', import.meta.url));
let temporary;
let AudioDecoder;
const originalAudioBuffer = globalThis.AudioBuffer;

before(async () => {
  temporary = await mkdtemp(join(tmpdir(), 'ad3-audio-retiming-'));
  const output = join(temporary, 'audio.cjs');
  await require('esbuild').build({
    entryPoints: [fileURLToPath(new URL('../packages/runtime/src/media/audio.ts', import.meta.url))],
    outfile: output, bundle: true, platform: 'node', format: 'cjs', logLevel: 'silent',
  });
  ({ AudioDecoder } = require(output));
  globalThis.AudioBuffer = class {
    constructor({ length, sampleRate, numberOfChannels }) {
      Object.assign(this, { length, sampleRate, numberOfChannels, duration: length / sampleRate });
    }
    copyToChannel() {}
    getChannelData() { return new Float32Array(this.length); }
  };
});

after(async () => {
  globalThis.AudioBuffer = originalAudioBuffer;
  if (temporary) await rm(temporary, { recursive: true, force: true });
});

function fixture() {
  const nodes = [];
  const bus = { input: {}, context: { currentTime: 0, sampleRate: 48000,
    createBufferSource() {
      const node = { stopped: false, disconnected: false,
        connect() {}, start(time) { this.startTime = time; }, stop() { this.stopped = true; },
        disconnect() { this.disconnected = true; },
      };
      nodes.push(node);
      return node;
    },
  } };
  const decoder = new AudioDecoder({ id: 'take', channels: 1, sampleRate: 48000 });
  // Feed decoded PCM at the same boundary as the media library, without a file.
  decoder.sink = { async *buffers(from) {
    for (let timestamp = from; timestamp <= from + 2; timestamp += 0.25) {
      yield { timestamp, duration: 0.25,
        buffer: new AudioBuffer({ length: 12000, sampleRate: 48000, numberOfChannels: 1 }),
      };
    }
  } };
  const options = { relativeFrom: 1, relativeTo: 1.5, trimStart: 0, trimEnd: 10,
    playbackRate: 1, currentTime: 1, relativeDelay: 0 };
  return { decoder, bus, nodes, options };
}

test('retiming cancels old audio even when the requested source is already buffered', async () => {
  for (const change of [{ trimStart: 0.5 }, { trimEnd: 1.3 }, { relativeDelay: 2 }, { playbackRate: 2 }]) {
    const { decoder, bus, nodes, options } = fixture();
    try {
      await decoder.playTo(bus, options);
      const oldNodes = [...nodes];
      assert(oldNodes.length > 0);
      await decoder.playTo(bus, { ...options, ...change });
      assert(oldNodes.every(n => n.stopped && n.disconnected), JSON.stringify(change));
      assert(nodes.length > oldNodes.length, 'the retained source must be scheduled again');
      const count = nodes.length;
      await decoder.playTo(bus, { ...options, ...change, currentTime: 1.01 });
      assert.equal(nodes.length, count, 'normal playback must reuse its scheduled buffers');
    } finally { decoder.reset(); }
  }
});

test('a decode finishing after a cut cannot schedule audio from before the cut', async () => {
  const { decoder, bus, nodes, options } = fixture();
  const normalSink = decoder.sink;
  const entered = Promise.withResolvers();
  const release = Promise.withResolvers();
  decoder.sink = { async *buffers() {
    entered.resolve();
    await release.promise;
    yield { timestamp: 1, duration: 0.25,
      buffer: new AudioBuffer({ length: 12000, sampleRate: 48000, numberOfChannels: 1 }),
    };
  } };
  try {
    const old = decoder.playTo(bus, options);
    await entered.promise;
    decoder.sink = normalSink;
    const current = decoder.playTo(bus, { ...options, relativeDelay: 5 });
    release.resolve();
    await Promise.all([old, current]);
    assert(nodes.length > 0);
    assert(nodes.every(n => n.startTime >= 6), 'a stale decode must never reach the output');
  } finally { release.resolve(); decoder.reset(); }
});
