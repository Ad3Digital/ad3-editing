/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { ANIMATION_TYPES } from "@diffusionstudio/reconciler";

import type { AnimationType as AnimationName } from "@diffusionstudio/jsx";
import type { AnimationType } from "@diffusionstudio/runtime";

export type AnimationOption = {
  name: AnimationName;
  label: string;
};

/**
 * Which nodes a group is offered on: a text animation needs glyphs to move,
 * a gain needs something to hear. A group without a kind fits every node.
 */
export type AnimationGroupKind = "text" | "audio";

export type AnimationGroup = {
  label: string;
  kind?: AnimationGroupKind;
  options: AnimationOption[];
};

/** The presets, grouped as the type select shows them. */
export const ANIMATION_GROUPS: AnimationGroup[] = [
  {
    label: "Fade",
    options: [
      { name: "fade", label: "Fade" },
      { name: "slideLeft", label: "Slide left" },
      { name: "slideRight", label: "Slide right" },
      { name: "slideUp", label: "Slide up" },
      { name: "slideDown", label: "Slide down" },
    ],
  },
  {
    label: "Scale",
    options: [
      { name: "grow", label: "Grow" },
      { name: "shrink", label: "Shrink" },
      { name: "spin", label: "Spin" },
      { name: "twist", label: "Twist" },
    ],
  },
  {
    label: "Blur",
    options: [{ name: "blur", label: "Blur" }],
  },
  {
    label: "Text",
    kind: "text",
    options: [
      { name: "appearWord", label: "Appear word" },
      { name: "appearChar", label: "Appear character" },
      { name: "scramble", label: "Scramble" },
    ],
  },
  {
    label: "Audio",
    kind: "audio",
    options: [{ name: "gain", label: "Volume" }],
  },
];

const ALL_OPTIONS = ANIMATION_GROUPS.flatMap((group) => group.options);

/** What the panel's plus authors: a fade, the one preset every node can play. */
export const DEFAULT_ANIMATION: AnimationOption = ALL_OPTIONS[0]!;

const BY_TYPE = new Map<AnimationType, AnimationOption>(
  ALL_OPTIONS.map((option) => [ANIMATION_TYPES[option.name]!, option]),
);

/**
 * The option for the type an `Animation` trait holds. Identity matters: the
 * type select compares its value against the groups it was given, so this
 * hands back the option itself rather than a copy.
 */
export function animationOption(type: AnimationType | undefined): AnimationOption {
  return (type === undefined ? undefined : BY_TYPE.get(type)) ?? DEFAULT_ANIMATION;
}
