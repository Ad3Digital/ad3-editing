/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { mainBridge } from "@/lib/ipc";
import { MAIN_CHANNELS } from "@desktop/main-channels";

export function handleGetFullscreenState() {
  if (!window.desktop) return Promise.resolve(false);
  return mainBridge.call(MAIN_CHANNELS.WINDOW_IS_FULLSCREEN, undefined);
}

export function handleWindowScreenshot() {
  return () => mainBridge.call(MAIN_CHANNELS.WINDOW_CAPTURE, undefined);
}

export function handleWindowFullscreenChange(mutate: (fullscreen: boolean) => void) {
  return ({ fullscreen }: { fullscreen: boolean }) => mutate(fullscreen);
}
