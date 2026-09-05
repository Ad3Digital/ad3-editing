/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

/**
 * Helper for creating the render event detail
 */
export function createRenderEventDetail(progress: number, total: number, startTime: number) {
  const duration = performance.now() - startTime;
  const time = (duration / gte1(progress)) * (total - progress);
  const remaining = new Date(time);

  return { remaining, progress, total };
}

/**
 * Helper for making sure a number is greater than 1
 */
function gte1(num: number): number {
  if (num < 1) return 1;
  return num;
}
