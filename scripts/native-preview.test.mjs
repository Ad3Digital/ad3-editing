import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const ffmpeg = process.env.AD3_FFMPEG_PATH || 'ffmpeg';
const binary = join(root, 'native/preview/target/release', process.platform === 'win32' ? 'ad3-preview.exe' : 'ad3-preview');
test('native preview: 2x, 5x, backward seeks, failure recovery and shutdown', { timeout: 60000 }, async () => {
  let source = process.env.AD3_PREVIEW_TEST_SOURCE;
  if (!source) {
    source = join(mkdtempSync(join(tmpdir(), 'ad3-native-')), 'motion.mp4');
    execFileSync(ffmpeg, ['-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=960x540:rate=60', '-t', '8', '-c:v', 'libx264', '-preset', 'ultrafast', '-g', '120', source], { windowsHide: true });
  }
  const worker = spawn(binary, [ffmpeg], { windowsHide: true, stdio: 'pipe' });
  let data = Buffer.alloc(0), serial = 0;
  const pending = new Map();
  worker.stdout.on('data', chunk => {
    data = Buffer.concat([data, chunk]);
    while (data.length >= 4) {
      const n = data.readUInt32LE(0);
      if (data.length < 4 + n) return;
      const header = JSON.parse(data.subarray(4, 4 + n));
      if (data.length < 4 + n + header.bytes) return;
      const pixels = Buffer.from(data.subarray(4 + n, 4 + n + header.bytes));
      pending.get(header.id)?.({ ...header, pixels }); pending.delete(header.id);
      data = data.subarray(4 + n + header.bytes);
    }
  });
  const frame = (time, options = {}) => new Promise(resolve => {
    const id = ++serial; pending.set(id, resolve);
    worker.stdin.write(JSON.stringify({ id, op: 'frame', session: 'test', source, width: 960, height: 540, time, ...options }) + '\n');
  });
  try {
    const started = performance.now(), hashes = new Set();
    for (const time of [0, ...Array.from({ length: 60 }, (_, i) => (i + 1) * 2 / 30), 1, 6, 2, 2 + 5 / 30, 2 + 10 / 30, 3]) {
      const result = await frame(time);
      assert.equal(result.error, null);
      assert.equal(result.pixels.length, 960 * 540 * 4);
      assert.ok(Math.abs(result.time - time) <= 1 / 30 + 0.001);
      hashes.add(createHash('sha256').update(result.pixels).digest('hex'));
    }
    assert.ok(hashes.size >= 58, `Only ${hashes.size} distinct moving frames`);
    const invalid = await frame(0, { source: 'https://invalid/video.mp4' });
    assert.ok(invalid.error);
    assert.equal((await frame(3.1)).error, null);
    worker.stdin.write(JSON.stringify({ id: 0, op: 'close', session: 'test' }) + '\n');
    assert.equal((await frame(0.5)).error, null);
    console.log(JSON.stringify({ distinctFrames: hashes.size, elapsedMs: Math.round(performance.now() - started) }));
  } finally {
    worker.stdin.end();
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { worker.kill(); reject(new Error('Native worker failed to shut down')); }, 5000);
      worker.once('exit', code => { clearTimeout(timer); code === 0 ? resolve() : reject(new Error(`Exit ${code}`)); });
    });
  }
});
