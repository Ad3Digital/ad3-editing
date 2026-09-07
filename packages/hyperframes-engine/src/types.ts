/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

export type HyperframesTemplate = "title" | "lower-third" | "stat" | "custom";

export interface HyperframesSettings {
  template: HyperframesTemplate;
  title: string;
  subtitle: string;
  accent: string;
  background: string;
  duration: number;
  width: number;
  height: number;
  fps: number;
  transparent: boolean;
}

export interface HyperframesDraft {
  id?: string;
  name: string;
  settings: HyperframesSettings;
  /** The actual editable source, not a prompt or a generated placeholder. */
  html: string;
}

export interface HyperframesRenderResult {
  compositionId: string;
  /** Portable project-relative file or numbered PNG directory. */
  source: string;
  format: "mp4" | "png-sequence";
  width: number;
  height: number;
  fps: number;
  duration: number;
  revision: string;
}

export interface HyperframesComposition {
  id: string;
  name: string;
  settings: HyperframesSettings;
  html: string;
  revision: string;
  updatedAt: string;
  rendered?: HyperframesRenderResult;
}

export interface HyperframesJob {
  id: string;
  compositionId: string;
  state: "running" | "completed" | "failed" | "cancelled";
  /** Bounded human-readable CLI progress; never credentials or the environment. */
  log: string;
  result?: HyperframesRenderResult;
  error?: string;
}

export interface HyperframesRuntimeStatus {
  available: boolean;
  version: string;
  reason?: string;
}

export interface HyperframesEngineOptions {
  /** Staged runtime containing runtime.json, node_modules and browser/FFmpeg binaries. */
  runtimeDir: string;
  /** Electron's executable runs the child CLI with ELECTRON_RUN_AS_NODE=1. */
  nodePath: string;
}
