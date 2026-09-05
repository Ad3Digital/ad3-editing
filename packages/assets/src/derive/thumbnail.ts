/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// The thumbnail of an image or video file: scaled to a width, its aspect
// ratio kept, encoded as WebP. For a video it is the frame shortly after the
// start. The decoder does the scaling (mediabunny's CanvasSink for video,
// createImageBitmap for images), so the picture is encoded once, as it comes.

import { ALL_FORMATS, BlobSource, CanvasSink, Input } from 'mediabunny';

/** The width the asset bar shows; what the cache stores without a variant. */
export const DEFAULT_THUMBNAIL_WIDTH = 300;

/** How far into a video the thumbnail frame is taken from, in seconds. */
const VIDEO_THUMBNAIL_OFFSET = 0.3;

const WEBP_QUALITY = 0.7;

/** A WebP thumbnail of `file`, `width` wide, or null when it is neither an image nor a video with a picture. */
export async function deriveThumbnail(file: Blob, mimeType: string, width = DEFAULT_THUMBNAIL_WIDTH): Promise<Blob | null> {
	if (mimeType.startsWith('image/')) return imageThumbnail(file, width);
	if (mimeType.startsWith('video/')) return videoThumbnail(file, width);
	return null;
}

async function imageThumbnail(file: Blob, width: number): Promise<Blob | null> {
	const bitmap = await createImageBitmap(file, { resizeWidth: width, resizeQuality: 'high' });
	try {
		const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
		const ctx = canvas.getContext('bitmaprenderer');
		if (!ctx) throw new Error('Could not create a bitmaprenderer context');
		ctx.transferFromImageBitmap(bitmap);
		return await encode(canvas);
	} finally {
		bitmap.close();
	}
}

async function videoThumbnail(file: Blob, width: number): Promise<Blob | null> {
	const input = new Input({ formats: ALL_FORMATS, source: new BlobSource(file) });
	try {
		const track = await input.getPrimaryVideoTrack();
		if (!track) return null;
		const sink = new CanvasSink(track, { width });
		const first = await track.getFirstTimestamp();
		const wrapped = (await sink.getCanvas(first + VIDEO_THUMBNAIL_OFFSET)) ?? (await sink.getCanvas(first));
		if (!wrapped) return null;
		return await encode(wrapped.canvas);
	} finally {
		input.dispose();
	}
}

function encode(canvas: HTMLCanvasElement | OffscreenCanvas): Promise<Blob | null> {
	if (canvas instanceof OffscreenCanvas) return canvas.convertToBlob({ type: 'image/webp', quality: WEBP_QUALITY });
	return new Promise((resolve) => canvas.toBlob(resolve, 'image/webp', WEBP_QUALITY));
}
