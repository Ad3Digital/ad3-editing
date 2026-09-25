/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

/**
 * Moving and trimming clips. A gesture lives on the entities it touches — the
 * snapshot traits say a drag is in flight and what it started from — rather
 * than in a variable here, so a clip scrolled off screen and back is still
 * being dragged, and so the drag survives the frame it began on.
 *
 * Every write goes through the editor's time props, so dragging a clip in the
 * timeline is the same edit as typing its start into the inspector.
 */

import {
	AdjustmentLayer,
	ChildOf,
	ClipDragOrigin,
	Computed,
	Geometry,
	Group,
	KeyframeDragOrigin,
	Selected,
	TrimDragOrigin,
	getParentNode,
	isSequence,
	findAssetDuration,
	store,
} from '@diffusionstudio/runtime';
import { Or } from 'koota';

import { clamp } from '@/utils';
import { getDocumentEditor } from '../editor';
import { getEditHistory } from '../history';
import { clearGapSelection } from './gaps';
import { authoredTime, moveEntityTo, trimIn, trimOut } from '../timing';
import { findSnapDelta, findSnapFrame } from './snapping';
import { framesToPixels, getResolution, getTimelineScene, pixelsToFrames } from './view';

import type { Entity, World } from 'koota';
import type { TimelineSurfaceState } from './surface';

const NODES = Or(Geometry, Group, AdjustmentLayer);

/** Which edge of a clip a trim is holding. */
export type TrimEdge = 'in' | 'out';

/** Applies one shared delta even to selected clips outside the viewport. */
export function updateDragGestures(world: World, surface: TimelineSurfaceState): void {
	const position = surface.pointer?.position;
	const scene = getTimelineScene(world);
	if (scene && position && position.state !== 'idle' && world.query(NODES, ClipDragOrigin).length) {
		applyClipDrag(world, surface, getResolution(world, scene));
	}
	if (position && position.state !== 'idle' && position.state !== 'lifted') return;
	let ended = false;
	for (const entity of world.query(NODES, ClipDragOrigin)) {
		entity.remove(ClipDragOrigin);
		ended = true;
	}
	for (const entity of world.query(NODES, TrimDragOrigin)) {
		entity.remove(TrimDragOrigin);
		ended = true;
	}
	for (const keyframe of world.query(KeyframeDragOrigin)) keyframe.remove(KeyframeDragOrigin);
	if (ended) getEditHistory(world).endGesture();
}

/** Snapshots the whole selection before any member moves. */
export function beginClipDrag(world: World, entity: Entity): void {
	const editor = getDocumentEditor(world);
	clearGapSelection(world);
	if (!entity.has(Selected)) editor.select(entity);
	const selection = new Set(world.query(NODES, Selected));
	const computed = store(world, Computed);
	getEditHistory(world).beginGesture();
	for (const member of selection) {
		let parent = getParentNode(member);
		while (parent && !selection.has(parent)) parent = getParentNode(parent);
		if (parent) continue; // Selected descendants travel with their parent.
		member.add(ClipDragOrigin);
		member.set(ClipDragOrigin, {
			authored: authoredTime(world, member, 'start') ?? 0,
			start: computed.start[member.id()] ?? 0,
			end: computed.end[member.id()] ?? 0,
		});
	}
}

function applyClipDrag(world: World, surface: TimelineSurfaceState, resolution: number): void {
	const moving = new Set(world.query(NODES, ClipDragOrigin));
	const computed = store(world, Computed);
	let min = Number.NEGATIVE_INFINITY;
	let max = Number.POSITIVE_INFINITY;
	for (const member of moving) {
		const origin = member.get(ClipDragOrigin)!;
		min = Math.max(min, -origin.start);
		const parent = getParentNode(member);
		if (!parent || !isSequence(parent)) continue;
		for (const sibling of world.query(NODES, ChildOf(parent))) {
			if (moving.has(sibling)) continue;
			const start = computed.start[sibling.id()] ?? 0;
			const end = computed.end[sibling.id()] ?? 0;
			if (end <= start) continue;
			if (end <= origin.start) min = Math.max(min, end - origin.start);
			if (start >= origin.end) max = Math.min(max, start - origin.end);
		}
	}
	const offset = pixelsToFrames(draggedPixels(surface), resolution);
	const snap = findSnapDelta(world, resolution, offset);
	const wanted = offset - (snap?.delta ?? 0);
	const delta = clamp(wanted, min, max);
	if (snap && delta === wanted) surface.snapX = framesToPixels(snap.frame, resolution);
	for (const member of moving) {
		moveEntityTo(world, member, member.get(ClipDragOrigin)!.start + delta);
	}
}

/** Notes where `entity`'s edges are, so a trim can be measured from them. */
export function beginTrim(world: World, entity: Entity): void {
	clearGapSelection(world);
	getEditHistory(world).beginGesture();
	const computed = store(world, Computed);
	const eid = entity.id();

	entity.add(TrimDragOrigin);
	entity.set(TrimDragOrigin, {
		start: computed.start[eid] ?? 0,
		end: computed.end[eid] ?? 0,
	});
}

/**
 * Moves the edge being held to where the pointer has taken it, within what
 * the clip can actually do: never past its other edge, and never past the end
 * of what it has to play.
 */
export function applyTrim(
	world: World,
	surface: TimelineSurfaceState,
	entity: Entity,
	edge: TrimEdge,
	resolution: number,
): void {
	const origin = entity.get(TrimDragOrigin)!;
	const offset = pixelsToFrames(draggedPixels(surface), resolution);

	const [min, max] = trimBounds(world, entity, edge, origin);
	const wanted = (edge === 'in' ? origin.start : origin.end) + offset;

	// Snapped only where the snap is somewhere the edge could have gone
	// anyway; otherwise it would look like it stuck and then slipped.
	const snapped = findSnapFrame(world, resolution, wanted);
	const frame = clamp(snapped !== null && snapped >= min && snapped <= max ? snapped : wanted, min, max);

	if (snapped !== null && frame === snapped) surface.snapX = framesToPixels(frame, resolution);

	if (edge === 'in') trimIn(world, entity, frame);
	else trimOut(world, entity, frame);
}

/** Source limits and stationary neighbours bound an edge throughout the trim. */
function trimBounds(
	world: World,
	entity: Entity,
	edge: TrimEdge,
	origin: { start: number; end: number },
): [min: number, max: number] {
	let min = edge === 'in' ? 0 : origin.start + 1;
	let max = edge === 'in' ? origin.end - 1 : Number.POSITIVE_INFINITY;
	const parent = getParentNode(entity);
	if (parent && isSequence(parent)) {
		const computed = store(world, Computed);
		for (const sibling of world.query(NODES, ChildOf(parent))) {
			if (sibling === entity) continue;
			const start = computed.start[sibling.id()] ?? 0;
			const end = computed.end[sibling.id()] ?? 0;
			if (end <= start) continue;
			if (edge === 'in' && end <= origin.start) min = Math.max(min, end);
			if (edge === 'out' && start >= origin.end) max = Math.min(max, start);
		}
	}

	const duration = findAssetDuration(world, entity);
	if (duration === null) return [min, max];

	const computed = entity.get(Computed);
	const rate = computed?.playbackRate || 1;
	// The scene frame the source starts at, and the one it runs out at.
	const sourceStart = computed?.origin ?? 0;

	if (edge === 'in') min = Math.max(min, sourceStart);
	else max = Math.min(max, sourceStart + duration / rate);

	return [min, max];
}

/** How far the pointer has come since the press, in pixels. */
function draggedPixels(surface: TimelineSurfaceState): number {
	const position = surface.pointer?.position;
	return position && position.state !== 'idle' ? position.deltaX : 0;
}

/** Whether a gesture is currently moving `entity`. */
export function isDragging(entity: Entity): boolean {
	return entity.has(ClipDragOrigin) || entity.has(TrimDragOrigin);
}
