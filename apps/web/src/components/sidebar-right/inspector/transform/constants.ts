/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { ConstraintType } from "@diffusionstudio/runtime";

export type Constraint = {
  key: ConstraintType;
  label: string;
};

export type AnchorPosition = {
  x: number;
  y: number;
  icon: string;
  tooltip: string;
};

export const anchorPositions: AnchorPosition[] = [
  { x: 0, y: 0, icon: 'anchor-align-top-left', tooltip: 'Align top left' },
  { x: 0.5, y: 0, icon: 'anchor-align-top-center', tooltip: 'Align top center' },
  { x: 1, y: 0, icon: 'anchor-align-top-right', tooltip: 'Align top right' },
  { x: 0, y: 0.5, icon: 'anchor-align-left', tooltip: 'Align left' },
  { x: 0.5, y: 0.5, icon: 'anchor-align-center', tooltip: 'Align center' },
  { x: 1, y: 0.5, icon: 'anchor-align-right', tooltip: 'Align right' },
  { x: 0, y: 1, icon: 'anchor-align-bottom-left', tooltip: 'Align bottom left' },
  { x: 0.5, y: 1, icon: 'anchor-align-bottom-center', tooltip: 'Align bottom center' },
  { x: 1, y: 1, icon: 'anchor-align-bottom-right', tooltip: 'Align bottom right' },
];

export const horizontalConstraints: Constraint[] = [
  { key: ConstraintType.MIN, label: 'Left' },
  { key: ConstraintType.MAX, label: 'Right' },
  { key: ConstraintType.CENTER, label: 'Center' },
  { key: ConstraintType.STRETCH, label: 'Left & right' },
  { key: ConstraintType.SCALE, label: 'Scale' },
];

export const verticalConstraints: Constraint[] = [
  { key: ConstraintType.MIN, label: 'Top' },
  { key: ConstraintType.MAX, label: 'Bottom' },
  { key: ConstraintType.CENTER, label: 'Center' },
  { key: ConstraintType.STRETCH, label: 'Top & bottom' },
  { key: ConstraintType.SCALE, label: 'Scale' },
];
