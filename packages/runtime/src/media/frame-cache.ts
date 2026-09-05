/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

type FrameCacheConfig = {
  pixels: number;
  count: number;
}


type Frame = VideoFrame | ImageBitmap;


export class FrameCache {
  public readonly atlas = new OffscreenCanvas(0, 0);
  public readonly atlasCtx = this.atlas.getContext('2d')!;
  public rotation = 0;
  public leftFrameIndex = 0;
  public rightFrameIndex: number;
  public lastInserted = -1;

  private tileWidth = 0;
  private tileHeight = 0;
  private columns = 0;
  private displayWidth = 0;
  private displayHeight = 0;
  private cachedRotation = 0;
  private readonly tiles = new Map<number, number>();
  private readonly freeTiles: number[] = [];

  public constructor(public readonly config: FrameCacheConfig) {
    this.rightFrameIndex = config.count - 1;
  }

  private resizeCaches(width: number, height: number) {
    if (width === this.displayWidth && height === this.displayHeight
      && this.rotation === this.cachedRotation) return;

    this.displayWidth = width;
    this.displayHeight = height;
    this.cachedRotation = this.rotation;
    const factor = Math.min(1, Math.sqrt(this.config.pixels / (width * height)));
    const sideways = this.rotation === 90 || this.rotation === 270;
    this.tileWidth = Math.max(1, Math.floor((sideways ? height : width) * factor));
    this.tileHeight = Math.max(1, Math.floor((sideways ? width : height) * factor));
    this.columns = Math.ceil(Math.sqrt(this.config.count));
    this.atlas.width = this.tileWidth * this.columns;
    this.atlas.height = this.tileHeight * Math.ceil(this.config.count / this.columns);
    this.atlasCtx.imageSmoothingQuality = 'high';
    this.tiles.clear();
    this.freeTiles.length = 0;
    for (let index = this.config.count - 1; index >= 0; index--) this.freeTiles.push(index);
  }

  private evictTiles(frameIndex: number) {
    if (this.freeTiles.length) return;
    let farthest: number | undefined;
    let distance = -1;
    for (const [frame, tile] of this.tiles) {
      if (frame < this.leftFrameIndex || frame > this.rightFrameIndex) {
        this.tiles.delete(frame);
        this.freeTiles.push(tile);
      } else if (Math.abs(frame - frameIndex) > distance) {
        farthest = frame;
        distance = Math.abs(frame - frameIndex);
      }
    }
    // Capacity stays fixed even when a caller gives a window larger than the atlas.
    if (!this.freeTiles.length && farthest !== undefined) {
      this.freeTiles.push(this.tiles.get(farthest)!);
      this.tiles.delete(farthest);
    }
  }

  public has(frameIndex: number) {
    return this.tiles.has(frameIndex);
  }

  /** Closest cached frame, preferring the earlier one on ties. */
  public findNearest(frameIndex: number, tolerance: number): number | undefined {
    if (this.tiles.has(frameIndex)) return frameIndex;
    let best: number | undefined;
    let bestDistance = tolerance + 1;
    for (const frame of this.tiles.keys()) {
      const distance = Math.abs(frame - frameIndex);
      if (distance < bestDistance || (distance === bestDistance && best !== undefined && frame < best)) {
        bestDistance = distance;
        best = frame;
      }
    }
    return best;
  }

  public insert(frame: Frame, index: number) {
    // Decoder preroll and outputs from abandoned seeks must not evict useful frames.
    if (index < this.leftFrameIndex || index > this.rightFrameIndex || this.has(index)) return;
    const width = frame instanceof VideoFrame ? frame.displayWidth : frame.width;
    const height = frame instanceof VideoFrame ? frame.displayHeight : frame.height;
    this.resizeCaches(width, height);
    this.evictTiles(index);
    const tileIndex = this.freeTiles.pop();
    if (tileIndex === undefined) return;
    this.tiles.set(index, tileIndex);

    const x = (tileIndex % this.columns) * this.tileWidth;
    const y = Math.floor(tileIndex / this.columns) * this.tileHeight;
    const sideways = this.rotation === 90 || this.rotation === 270;
    const drawWidth = sideways ? this.tileHeight : this.tileWidth;
    const drawHeight = sideways ? this.tileWidth : this.tileHeight;
    const ctx = this.atlasCtx;
    ctx.save();
    ctx.clearRect(x, y, this.tileWidth, this.tileHeight);
    ctx.translate(x + this.tileWidth / 2, y + this.tileHeight / 2);
    ctx.rotate(this.rotation * Math.PI / 180);
    ctx.drawImage(frame, -drawWidth / 2, -drawHeight / 2, drawWidth, drawHeight);
    ctx.restore();
    this.lastInserted = index;
  }

  public findTile(frameIndex: number) {
    const index = this.tiles.get(frameIndex);
    if (index === undefined) return;
    return {
      x: (index % this.columns) * this.tileWidth,
      y: Math.floor(index / this.columns) * this.tileHeight,
      width: this.tileWidth,
      height: this.tileHeight,
    };
  }

  public dispose() {
    this.atlas.width = 0;
    this.atlas.height = 0;
    this.tileWidth = 0;
    this.tileHeight = 0;
    this.columns = 0;
    this.displayWidth = 0;
    this.displayHeight = 0;
    this.lastInserted = -1;
    this.tiles.clear();
    this.freeTiles.length = 0;
  }
}
