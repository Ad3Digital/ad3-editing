import type { VideoAsset } from '@diffusionstudio/assets';

type NativeFrame = { time: number; width: number; height: number; pixels: Uint8Array };
type NativeBridge = {
  frame(request: { session: string; source: string; time: number; width: number; height: number }): Promise<NativeFrame>;
  close(session: string): Promise<void>;
};
const bridge = () => (globalThis as typeof globalThis & { ad3NativePreview?: NativeBridge }).ad3NativePreview;
export const hasNativePreview = () => !!bridge();

/** Desktop preview only. Exports retain the full-resolution, frame-exact path. */
export class NativeVideoBuffer {
  public readonly backend = 'rust-ffmpeg';
  public mode: 'alive' | 'idle' | 'discarded' = 'alive';
  public errored = false;
  public initialized = Promise.resolve();
  public readonly canvas = new OffscreenCanvas(0, 0);
  public displayedTime = -1;
  private readonly ctx = this.canvas.getContext('2d')!;
  private readonly session = crypto.randomUUID();
  private desired = -1;
  private requested = -1;
  private pending = false;
  private generation = 0;
  private retryAt = 0;
  private nextReadAt = 0;
  private readonly width: number;
  private readonly height: number;

  constructor(public asset: VideoAsset, private readonly source: string) {
    const scale = Math.min(1, 960 / asset.width, 540 / asset.height);
    this.width = Math.max(2, Math.round(asset.width * scale / 2) * 2);
    this.height = Math.max(2, Math.round(asset.height * scale / 2) * 2);
  }
  seekTo(frame: number, frameRate: number, _scrubbing = false, _transport = 1): undefined {
    if (this.mode === 'discarded') return;
    this.mode = 'alive';
    this.desired = Math.max(0, Math.min(this.asset.duration - 1 / this.asset.frameRate, frame / frameRate));
    if (!this.pending && this.desired !== this.requested && performance.now() >= Math.max(this.retryAt, this.nextReadAt)) void this.read();
  }
  private async read() {
    const api = bridge();
    if (!api || this.mode !== 'alive') return;
    this.pending = true;
    this.nextReadAt = performance.now() + 31;
    const generation = this.generation;
    const time = this.desired;
    this.requested = time;
    try {
      const result = await api.frame({ session: this.session, source: this.source, time, width: this.width, height: this.height });
      if (generation !== this.generation || this.mode !== 'alive') return;
      // A drag can jump minutes while the native request is in flight. Never paint that old position.
      if (Math.abs(this.desired - time) > 0.5) return;
      if (this.canvas.width !== result.width || this.canvas.height !== result.height) {
        this.canvas.width = result.width; this.canvas.height = result.height;
      }
      this.ctx.putImageData(new ImageData(new Uint8ClampedArray(result.pixels), result.width, result.height), 0, 0);
      this.displayedTime = result.time;
      this.errored = false;
    } catch (error) {
      if (generation !== this.generation) return;
      if (!this.errored) console.error('Native preview failed; retrying', error);
      this.errored = true; this.requested = -1; this.retryAt = performance.now() + 250;
    } finally {
      this.pending = false;
    }
  }
  toBitmap() { return this.canvas.width ? this.canvas : null; }
  idle() {
    if (this.mode !== 'alive') return;
    this.mode = 'idle'; this.generation++; this.requested = -1;
    void bridge()?.close(this.session).catch(() => {});
  }
  dispose() { this.idle(); this.mode = 'discarded'; this.canvas.width = 0; this.canvas.height = 0; }
}
