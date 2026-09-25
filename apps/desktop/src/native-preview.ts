import { app, ipcMain } from 'electron';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { existsSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';

const CHANNEL = 'ad3:native-preview';
type Request = { session: string; source: string; time: number; width: number; height: number };
type Frame = { time: number; width: number; height: number; pixels: Uint8Array };
type Header = { id: number; time: number; width: number; height: number; bytes: number; error?: string };
let child: ChildProcessWithoutNullStreams | null = null;
let serial = 0;
let data: Buffer = Buffer.alloc(4);
let offset = 0;
let phase: 'size' | 'header' | 'pixels' = 'size';
let incoming: Header | null = null;
const pending = new Map<number, { resolve: (frame: Frame) => void; reject: (e: Error) => void; timer: ReturnType<typeof setTimeout> }>();
const sessions = new Set<string>();
const watched = new Set<number>();
const executable = () => join(app.getAppPath(), 'dist', process.platform === 'win32' ? 'ad3-preview.exe' : 'ad3-preview');
export const nativePreviewAvailable = () => process.env.AD3_NATIVE_PREVIEW !== '0' && existsSync(executable());

function fail(error: Error) {
  for (const item of pending.values()) { clearTimeout(item.timer); item.reject(error); }
  pending.clear();
}
function receive(chunk: Buffer) {
  let cursor = 0;
  while (cursor < chunk.length) {
    const count = Math.min(data.length - offset, chunk.length - cursor);
    chunk.copy(data, offset, cursor, cursor + count); offset += count; cursor += count;
    if (offset !== data.length) return;
    offset = 0;
    if (phase === 'size') {
      const size = data.readUInt32LE(0);
      if (!size || size > 8192) throw new Error('Invalid native preview header');
      data = Buffer.allocUnsafe(size); phase = 'header';
    } else if (phase === 'header') {
      incoming = JSON.parse(data.toString()) as Header;
      if (!Number.isInteger(incoming.bytes) || incoming.bytes < 0 || incoming.bytes > 1280 * 1280 * 4) throw new Error('Invalid native preview frame');
      data = Buffer.allocUnsafe(incoming.bytes); phase = 'pixels';
      if (incoming.bytes) continue;
      finishFrame();
    } else {
      finishFrame();
    }
  }
}
function finishFrame() {
  const header = incoming!;
  const item = pending.get(header.id);
  if (item) {
    pending.delete(header.id); clearTimeout(item.timer);
    if (header.error) item.reject(new Error(header.error));
    else if (header.bytes !== header.width * header.height * 4) item.reject(new Error('Invalid native frame dimensions'));
    else item.resolve({ time: header.time, width: header.width, height: header.height, pixels: data });
  }
  incoming = null; data = Buffer.alloc(4); phase = 'size'; offset = 0;
}
function ensureWorker() {
  if (child) return child;
  const filename = process.platform === 'win32' ? 'ffmpeg.exe' : 'ffmpeg';
  const staged = join(process.resourcesPath, 'hyperframes-engine', 'ffmpeg', filename);
  const ffmpeg = process.env.AD3_FFMPEG_PATH || (existsSync(staged) ? staged : filename);
  const worker = spawn(executable(), [ffmpeg], { windowsHide: true, stdio: 'pipe' });
  child = worker; data = Buffer.alloc(4); offset = 0; phase = 'size'; incoming = null;
  worker.stdout.on('data', (chunk: Buffer) => {
    if (child !== worker) return;
    try { receive(chunk); } catch (error) { stopNativePreview(); console.error(error); }
  });
  worker.stderr.on('data', (chunk: Buffer) => console.error('[native preview]', chunk.toString().trim()));
  worker.stdin.on('error', () => {});
  worker.on('error', (e) => { if (child === worker) { child = null; fail(e); } });
  worker.on('exit', () => { if (child === worker) { child = null; sessions.clear(); fail(new Error('Native preview worker stopped')); } });
  return worker;
}
function closeSession(session: string) {
  if (!sessions.delete(session)) return;
  child?.stdin.write(JSON.stringify({ id: 0, session, op: 'close' }) + '\n');
}
export function stopNativePreview() {
  for (const session of [...sessions]) closeSession(session);
  const worker = child; child = null;
  worker?.stdin.end();
  // EOF makes Rust close and reap every FFmpeg child before exiting.
  fail(new Error('Native preview stopped'));
}
export function setupNativePreview() {
  ipcMain.handle(CHANNEL, async (event, op: string, request: Request) => {
    if (!nativePreviewAvailable() || event.senderFrame !== event.sender.mainFrame) throw new Error('Native preview unavailable');
    if (!request || typeof request.session !== 'string' || request.session.length > 100) throw new Error('Invalid preview session');
    const owner = event.sender.id;
    const session = `${owner}:${request.session}`;
    if (op === 'close') { closeSession(session); return; }
    if (op !== 'frame' || typeof request.source !== 'string' || !isAbsolute(request.source)
      || !Number.isFinite(request.time) || request.time < 0
      || !Number.isInteger(request.width) || !Number.isInteger(request.height)
      || request.width < 1 || request.height < 1 || request.width > 1280 || request.height > 1280
      || request.width * request.height > 921600) throw new Error('Invalid preview request');
    if (pending.size >= 24) throw new Error('Native preview request limit');
    if (!watched.has(owner)) {
      watched.add(owner);
      const cleanup = () => { for (const key of [...sessions]) if (key.startsWith(`${owner}:`)) closeSession(key); };
      event.sender.on('did-start-navigation', (_e, _url, inPlace, mainFrame) => { if (mainFrame && !inPlace) cleanup(); });
      event.sender.once('destroyed', () => { cleanup(); watched.delete(owner); });
    }
    const worker = ensureWorker();
    const id = ++serial;
    sessions.add(session);
    return new Promise<Frame>((resolve, reject) => {
      const timer = setTimeout(() => { pending.delete(id); closeSession(session); reject(new Error('Native preview timed out')); }, 5000);
      pending.set(id, { resolve, reject, timer });
      worker.stdin.write(JSON.stringify({ ...request, id, session, op: 'frame' }) + '\n');
    });
  });
}
