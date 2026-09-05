/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

/**
 * Declarative generated assets. A `generate.*` call is pure: it validates its
 * options and returns an `AssetRef` value that can be passed wherever a source
 * is expected (`src`, `startFrame`, `endFrame`, `refs`). Nothing generates
 * until the mounted tree commits; refs never used by a mounted element are
 * dropped. Because dependencies are values (not string ids), reference cycles
 * are impossible by construction.
 */

export type AspectRatio = "1:1" | "4:3" | "3:4" | "16:9" | "9:16";

/** A path, URL, asset id, or another `generate.*` declaration. */
export type AssetInput = string | AssetRef;

export type GenerateImageOptions = {
  prompt: string;
  model?: string;
  aspectRatio?: AspectRatio;
  refs?: AssetInput[];
  seed?: number;
};

export type GenerateVideoOptions = {
  prompt: string;
  model?: string;
  aspectRatio?: AspectRatio;
  /** Whole seconds; default 5. */
  duration?: number;
  /** Generate audio alongside; models with the `audio` feature only. */
  audio?: boolean;
  /** Image used as the first frame. */
  startFrame?: AssetInput;
  /** Image used as the last frame; `end-frame` feature only. */
  endFrame?: AssetInput;
  seed?: number;
};

export type GenerateVoiceOptions = {
  /** The text to speak. */
  prompt: string;
  /** Voice id; default the first voice from `dapi voices`. */
  voice?: string;
  seed?: number;
};

export type GenerateAudioOptions = {
  prompt: string;
  model?: string;
  duration?: number;
  seed?: number;
};

export type AssetSpecInput =
  | ({ type: "image" } & GenerateImageOptions)
  | ({ type: "video" } & GenerateVideoOptions)
  | ({ type: "voice" } & GenerateVoiceOptions)
  | ({ type: "audio" } & GenerateAudioOptions);

/**
 * Opaque handle to a declared (not-yet-generated) asset. Obtained from
 * `generate.*`; consumed by `src` / `startFrame` / `endFrame` / `refs`.
 */
export class AssetRef {
  /** @internal — read via `getAssetSpec`. */
  readonly spec: AssetSpecInput;

  /** @internal — declare refs with `generate.*`, not `new AssetRef()`. */
  constructor(spec: AssetSpecInput) {
    this.spec = spec;
  }
}

export function isAssetRef(value: unknown): value is AssetRef {
  return value instanceof AssetRef;
}

/** An `AssetSpecInput` whose inputs are all strings — no nested declarations. */
export type FlatAssetSpec = AssetSpecInput & {
  refs?: string[];
  startFrame?: string;
  endFrame?: string;
};

/**
 * A declaration as data, for the edit protocol. An `AssetRef` itself cannot
 * travel as a prop value (see `isPropValue`: an object literal would not read
 * back as one), so an edit carries the spec in this tagged shape instead, and
 * the source writer spells it as the `generate.*` call that reproduces it.
 */
export type SerializedAssetRef = { $generate: FlatAssetSpec };

/**
 * The wire form of a declaration, or undefined when the spec references
 * other declarations: nested refs only exist in authored code, where the
 * call spelling them already is — the editor declares over library assets.
 */
export function serializeAssetRef(ref: AssetRef): SerializedAssetRef | undefined {
  const spec = ref.spec;
  const inputs = [
    ...(spec.type === "image" ? (spec.refs ?? []) : []),
    ...(spec.type === "video" ? [spec.startFrame, spec.endFrame] : []),
  ];
  if (inputs.some((input) => isAssetRef(input))) return undefined;

  // Without the undefined entries: they are not part of the declaration, and
  // the writer spells every entry it is handed.
  const entries = Object.entries(spec).filter(([, value]) => value !== undefined);
  return { $generate: Object.fromEntries(entries) as FlatAssetSpec };
}

const SPEC_TYPES = ["image", "video", "voice", "audio"];

export function isSerializedAssetRef(value: unknown): value is SerializedAssetRef {
  if (typeof value !== "object" || value === null || !("$generate" in value)) return false;
  const spec = (value as SerializedAssetRef).$generate;
  return (
    typeof spec === "object" && spec !== null &&
    SPEC_TYPES.includes(spec.type) && typeof spec.prompt === "string"
  );
}

/** Host-side accessor for a declaration's spec. */
export function getAssetSpec(ref: AssetRef): AssetSpecInput {
  return ref.spec;
}

function requirePrompt(value: unknown, kind: string): string {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`generate.${kind} requires a non-empty string prompt`);
  }
  return value;
}

function checkAssetInput(value: unknown, kind: string, option: string): void {
  if (value === undefined) return;
  if (typeof value === "string" && value.trim().length > 0) return;
  if (isAssetRef(value)) return;
  throw new Error(`generate.${kind}: ${option} must be a path, URL, asset id, or an AssetRef`);
}

export const generate = {
  image(opts: GenerateImageOptions): AssetRef {
    requirePrompt(opts.prompt, "image");
    for (const ref of opts.refs ?? []) checkAssetInput(ref, "image", "refs");
    return new AssetRef({ type: "image", ...opts });
  },

  video(opts: GenerateVideoOptions): AssetRef {
    requirePrompt(opts.prompt, "video");
    checkAssetInput(opts.startFrame, "video", "startFrame");
    checkAssetInput(opts.endFrame, "video", "endFrame");
    if (opts.duration !== undefined && (!Number.isInteger(opts.duration) || opts.duration <= 0)) {
      throw new Error("generate.video: duration must be a positive whole number of seconds");
    }
    return new AssetRef({ type: "video", ...opts });
  },

  voice(opts: GenerateVoiceOptions): AssetRef {
    requirePrompt(opts.prompt, "voice");
    return new AssetRef({ type: "voice", ...opts });
  },

  audio(opts: GenerateAudioOptions): AssetRef {
    requirePrompt(opts.prompt, "audio");
    if (opts.duration !== undefined && (!Number.isFinite(opts.duration) || opts.duration <= 0)) {
      throw new Error("generate.audio: duration must be a positive number of seconds");
    }
    return new AssetRef({ type: "audio", ...opts });
  },
};
