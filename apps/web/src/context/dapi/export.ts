/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { Computed, FrameRate, getActiveEntity } from "@diffusionstudio/runtime";

import { getDefaultExportTemplate } from "@/components/sidebar-right/inspector/export-templates";
import { renderScene } from "@/context/render";
import { ElectronWritableFileHandle } from "@/lib/electron-file-writable";
import { assert } from "@/utils/common";

import type { EditorSession } from "./session";

export type ExportRequest = {
  output: string;
  resolution?: number;
  fps?: number;
  bitrate?: number;
};

/** Renders the active scene directly to an absolute path supplied by dapi. */
export function handleExport(requireSession: () => EditorSession) {
  return async (request: ExportRequest) => {
    const { world, project, engine } = requireSession();
    const scene = getActiveEntity(world);
    assert(scene, "No active scene to export.");
    assert(request.output.toLowerCase().endsWith(".mp4"), "dapi export currently supports .mp4 output.");

    const template = getDefaultExportTemplate();
    const projectFps = world.get(FrameRate)?.value ?? 30;
    const fps = request.fps ?? projectFps;
    const resolution = request.resolution ?? template.video?.resolution ?? 1080;
    const target = new ElectronWritableFileHandle(request.output);
    const result = await renderScene(engine, {
      scene,
      target,
      dir: project.dir(),
      config: {
        ...template,
        format: "mp4",
        video: {
          ...template.video,
          codec: "avc",
          fps,
          resolution,
          ...(request.bitrate ? { bitrate: request.bitrate } : {}),
        },
        audio: { ...template.audio, codec: "aac" },
      },
    });

    if (result.type === "error") {
      await target.dispose();
      throw result.error;
    }

    const duration = (scene.get(Computed)?.duration ?? 0) / projectFps;
    return { path: request.output, duration, fps, resolution, format: "mp4" as const };
  };
}
