/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

const AVATAR_MAX_SIZE = 256;

export async function cropAndResizeAvatar(file: File): Promise<Blob> {
  const bitmap = await createImageBitmap(file);
  const size = Math.min(bitmap.width, bitmap.height, AVATAR_MAX_SIZE);

  const canvas = new OffscreenCanvas(size, size);
  const ctx = canvas.getContext("2d")!;

  // Center-crop to square
  const sx = (bitmap.width - Math.min(bitmap.width, bitmap.height)) / 2;
  const sy = (bitmap.height - Math.min(bitmap.width, bitmap.height)) / 2;
  const sSize = Math.min(bitmap.width, bitmap.height);

  ctx.drawImage(bitmap, sx, sy, sSize, sSize, 0, 0, size, size);
  bitmap.close();

  return await canvas.convertToBlob({ type: "image/webp", quality: 0.8 });
}
