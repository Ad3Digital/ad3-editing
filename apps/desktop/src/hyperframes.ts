/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { app } from "electron";
import { join } from "node:path";
import { HyperframesEngine } from "@ad3/hyperframes-engine";

import { getProject } from "./projects";

import type {
  HyperframesComposition,
  HyperframesDraft,
  HyperframesJob,
  HyperframesRuntimeStatus,
} from "@ad3/hyperframes-engine/types";

let engine: HyperframesEngine | undefined;
let disposing: Promise<void> | undefined;

/**
 * The engine is deliberately constructed only when the HyperFrames panel asks
 * for it. Development and packaged builds use the same staged runtime shape.
 */
function getEngine(): HyperframesEngine {
  if (engine) return engine;

  const runtimeDir = app.isPackaged
    ? join(process.resourcesPath, "hyperframes-engine")
    : join(app.getAppPath(), "hyperframes-engine");
  engine = new HyperframesEngine({ runtimeDir, nodePath: process.execPath });
  return engine;
}

/** Runtime availability does not require an open project. */
export function hyperframesStatus(): Promise<HyperframesRuntimeStatus> {
  return getEngine().status();
}

/** Refuse all project-scoped work unless the main project registry recognizes it. */
async function projectDir(dir: string): Promise<string> {
  const project = await getProject(dir);
  if (!project) throw new Error("Open a valid project before using HyperFrames.");
  return project.dir;
}

export async function listHyperframes(dir: string): Promise<HyperframesComposition[]> {
  return getEngine().list(await projectDir(dir));
}

export async function saveHyperframes(dir: string, draft: HyperframesDraft): Promise<HyperframesComposition> {
  return getEngine().save(await projectDir(dir), draft);
}

export async function previewHyperframes(dir: string, id: string): Promise<{ url: string }> {
  return getEngine().preview(await projectDir(dir), id);
}

export async function renderHyperframes(dir: string, id: string): Promise<HyperframesJob> {
  return getEngine().render(await projectDir(dir), id);
}

export async function hyperframesJob(dir: string, jobId: string): Promise<HyperframesJob> {
  return getEngine().job(await projectDir(dir), jobId);
}

export async function cancelHyperframes(dir: string, jobId: string): Promise<HyperframesJob> {
  return getEngine().cancel(await projectDir(dir), jobId);
}

/** Idempotent shutdown for the worker, CLI children, browser and preview server. */
export function disposeHyperframes(): Promise<void> {
  if (!engine) return Promise.resolve();
  disposing ??= engine.dispose().finally(() => {
    engine = undefined;
    disposing = undefined;
  });
  return disposing;
}
