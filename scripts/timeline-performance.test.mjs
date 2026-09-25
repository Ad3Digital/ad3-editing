import assert from 'node:assert/strict';
import { before, test } from 'node:test';
import { mkdtemp, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
let createPointer, VideoExporter, VideoBuffer, VideoDecoderQueue;
before(async () => {
  const directory = await mkdtemp(join(tmpdir(), 'ad3-performance-tests-'));
  for (const [name, path] of [
    ['pointer', '../apps/web/src/engine/timeline/pointer.ts'],
    ['video', '../packages/runtime/src/media/video.ts'],
  ]) {
    await require('esbuild').build({
      entryPoints: [fileURLToPath(new URL(path, import.meta.url))],
      outfile: join(directory, `${name}.cjs`), bundle: true, platform: 'node', format: 'cjs',
      plugins: [{ name: 'assert-only', setup(build) {
        build.onResolve({ filter: /^@\/utils$/ }, () => ({ path: 'assert', namespace: 'fixture' }));
        build.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: 'export function assert(value, message) { if (!value) throw new Error(message); }' }));
        build.onLoad({ filter: /runtime[\\/]src[\\/]media[\\/]video\.ts$/ }, async ({ path }) => ({
          contents: await readFile(path, 'utf8') + '\nexport { VideoDecoderQueue };', loader: 'ts',
        }));
      } }],
    });
  }
  ({ createPointer } = require(join(directory, 'pointer.cjs')));
  ({ VideoExporter, VideoBuffer, VideoDecoderQueue } = require(join(directory, 'video.cjs')));
});

test('hit testing keeps the topmost target, passive targets and drag owner across frames', () => {
  globalThis.window = { devicePixelRatio: 1 };
  globalThis.DOMPoint = class { constructor(x, y) { Object.assign(this, { x, y }); } };
  const pointer = createPointer({
    canvas: { getBoundingClientRect: () => ({ left: 0, top: 0 }) },
    ctx: { getTransform: () => ({ transformPoint: point => point }) },
  });
  const draw = () => [
    pointer.scope('back').region(0, 0, 100, 100),
    pointer.scope('front').region(0, 0, 50, 50),
    pointer.scope('passive').region(0, 0, 50, 50, undefined, true),
  ];
  pointer.move({ clientX: 10, clientY: 10 });
  draw(); pointer.reset();
  const hovered = draw();
  assert.deepEqual(hovered.map(r => r.hovering), [false, true, true]);
  pointer.reset();
  pointer.down({ clientX: 10, clientY: 10, button: 0 });
  assert.equal(draw()[1].pressed, true); pointer.reset();
  pointer.move({ clientX: 80, clientY: 80 });
  const moved = draw();
  assert.equal(moved[0].hovering, true);
  assert.equal(moved[1].dragging, true);
  assert.equal(moved[0].dragging, false);
  pointer.reset();
  pointer.up({ clientX: 80, clientY: 80 });
  assert.equal(draw()[1].dragging, false);
});

test('finished export clips release their decoder iterator and retained frame', () => {
  let returned = 0;
  const decoder = Object.create(VideoExporter.prototype);
  decoder.iterator = { return: async () => { returned++; } };
  decoder.currentCanvas = { canvas: {} };
  decoder.idle(); decoder.idle();
  assert.equal(returned, 1);
  assert.equal(decoder.toBitmap(), null);
  assert.equal(decoder.iterator, null);
});

test('backward export seeks close the old iterator before starting another', async () => {
  const events = [];
  const decoder = Object.create(VideoExporter.prototype);
  Object.assign(decoder, {
    initialized: Promise.resolve(), errored: false, asset: { frameRate: 30 }, firstTimestamp: 0,
    currentCanvas: { timestamp: 5, canvas: {} },
    iterator: { return: async () => { events.push('closed'); } },
    canvasSink: { canvases(time) {
      events.push('opened');
      return { next: async () => ({ value: { timestamp: time, canvas: {} }, done: false }) };
    } },
  });
  await decoder.seekTo(30, 30);
  assert.deepEqual(events, ['closed', 'opened']);
  assert.equal(decoder.currentCanvas.timestamp, 1);
});

test('a paused jump into a cold clip paints a keyframe before the exact seek', () => {
  const decoder = Object.create(VideoBuffer.prototype);
  Object.assign(decoder, {
    packetSink: {}, errored: false, mode: 'alive', asset: { frameRate: 60 },
    currentFrame: -1, lastFrameIndex: 30000, lastSeekAt: -Infinity,
    touch() {}, scrubTo(frame) { this.scrubbed = frame; return true; },
    exactSeekTo() { throw new Error('cold scrub should use its keyframe first'); },
  });
  decoder.seekTo(3000, 30, true);
  assert.equal(decoder.scrubbed, 6000);
});

test('decoder output cannot bypass input backpressure, even after partial dequeue', async () => {
  const queue = new VideoDecoderQueue(() => {});
  let submitted = 0;
  const decoder = { state: 'configured', decodeQueueSize: 6,
    decode() { submitted++; this.decodeQueueSize++; },
    close() { this.state = 'closed'; },
  };
  queue.decoder = decoder;
  const pending = queue.decode({ type: 'delta', microsecondTimestamp: 100,
    toEncodedVideoChunk: () => ({}),
  });
  queue.handleOutput({ timestamp: 0, close() {} });
  await Promise.resolve();
  assert.equal(submitted, 0, 'delayed output is not free input capacity');
  decoder.decodeQueueSize = 5;
  queue.handleDequeue();
  await Promise.resolve();
  assert.equal(submitted, 0, 'a dequeue must recheck the limit');
  decoder.decodeQueueSize = 3;
  queue.handleDequeue();
  await pending;
  assert.equal(submitted, 1);
  assert.equal(decoder.decodeQueueSize, 4);
});

test('idling a clip wakes a blocked submit without feeding the closed decoder', async () => {
  const queue = new VideoDecoderQueue(() => {});
  let submitted = 0;
  queue.decoder = { state: 'configured', decodeQueueSize: 4,
    decode() { submitted++; }, close() { this.state = 'closed'; },
  };
  const pending = queue.decode({ type: 'delta', toEncodedVideoChunk: () => ({}) });
  queue.dispose();
  await pending;
  assert.equal(submitted, 0);
});
