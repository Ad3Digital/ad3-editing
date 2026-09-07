/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { MAIN_CHANNELS } from "@desktop/main-channels";
import { mainBridge } from "@/lib/ipc";

import type {
  HyperframesComposition,
  HyperframesDraft,
  HyperframesJob,
  HyperframesRuntimeStatus,
} from "@ad3/hyperframes-engine/types";

export type { HyperframesComposition, HyperframesDraft, HyperframesJob, HyperframesRuntimeStatus };

const browserStatus: HyperframesRuntimeStatus = {
  available: false,
  version: "",
  reason: "HyperFrames rendering is available in the desktop app.",
};

/** The browser app exposes the panel but never attempts to launch a local runtime. */
export function statusHyperframes(): Promise<HyperframesRuntimeStatus> {
  if (!window.desktop) return Promise.resolve(browserStatus);
  return mainBridge.call(MAIN_CHANNELS.HYPERFRAMES_STATUS, undefined);
}

export function listHyperframes(dir: string): Promise<HyperframesComposition[]> {
  return mainBridge.call(MAIN_CHANNELS.HYPERFRAMES_LIST, { dir });
}

export function saveHyperframes(dir: string, draft: HyperframesDraft): Promise<HyperframesComposition> {
  return mainBridge.call(MAIN_CHANNELS.HYPERFRAMES_SAVE, { dir, draft });
}

export function previewHyperframes(dir: string, id: string): Promise<{ url: string }> {
  return mainBridge.call(MAIN_CHANNELS.HYPERFRAMES_PREVIEW, { dir, id });
}

export function renderHyperframes(dir: string, id: string): Promise<HyperframesJob> {
  return mainBridge.call(MAIN_CHANNELS.HYPERFRAMES_RENDER, { dir, id });
}

export function getHyperframesJob(dir: string, jobId: string): Promise<HyperframesJob> {
  return mainBridge.call(MAIN_CHANNELS.HYPERFRAMES_JOB, { dir, jobId });
}

export function cancelHyperframes(dir: string, jobId: string): Promise<HyperframesJob> {
  return mainBridge.call(MAIN_CHANNELS.HYPERFRAMES_CANCEL, { dir, jobId });
}
