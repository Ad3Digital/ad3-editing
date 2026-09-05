/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

/**
 * Cutting a clip in two at the playhead. The clip is copied where it stands,
 * then the original is trimmed to end at the playhead and the copy to start
 * there, so the two halves play exactly what the one clip did. Everything
 * goes through the editor, so the file ends up saying the same thing the
 * canvas shows: an element beside the one it came from, with its own times.
 */

import { Sequence } from '@diffusionstudio/reconciler';
import {
	AdjustmentLayer,
	ChildOf,
	Computed,
	Geometry,
	Group,
	Selected,
	getActiveEntity,
	getNextName,
	getParentEntity,
	handOffDecoders,
	isGroup,
	isSequence,
	setPlayhead,
	store,
} from '@diffusionstudio/runtime';
import { Or } from 'koota';

import { getDocumentEditor } from './editor';
import { cloneFramesForSplit, clonePeaksForSplit } from './timeline';
import { moveEntityTo, trimIn, trimOut } from './timing';

import type { Entity, World } from 'koota';

/** The node kinds that sit on a timeline, and so are the ones a cut is about. */
const NODES = Or(Geometry, Group, AdjustmentLayer);

/**
 * What the cut applies to: the selection, or — with nothing selected — every
 * clip directly under the active scene. The scene is the editing context, so
 * it is never a target, even when it is what is selected.
 */
function splitTargets(world: World, scene: Entity): Entity[] {
	const selection = [...world.query(Selected, NODES)].filter((entity) => entity !== scene);
	if (selection.length > 0) return selection;

	return [...world.query(NODES, ChildOf(scene))];
}

/**
 * The clips the playhead actually cuts, with any sequence among the targets
 * opened up: a sequence is not a clip — its time is whatever its children
 * span — so the cut goes to whichever of them the playhead is over.
 *
 * `Computed.visibility` is what "the playhead is over it" means, since the
 * playback system already works it out for every node, ancestors included.
 * The bounds are then checked directly as well: a clip held visible through a
 * transition is not one the playhead is inside, and cutting it would leave a
 * half of nothing.
 */
function splitUnits(world: World, targets: Entity[], frame: number): Entity[] {
	const computed = store(world, Computed);
	const units = new Set<Entity>();

	const walk = (entity: Entity): void => {
		if (computed.visibility[entity.id()] !== 1) return;

		if (isSequence(entity)) {
			for (const child of world.query(NODES, ChildOf(entity))) walk(child);
			return;
		}

		const start = computed.start[entity.id()] ?? 0;
		const end = computed.end[entity.id()] ?? 0;
		if (frame <= start || frame >= end) return;

		units.add(entity);
	};

	for (const entity of targets) walk(entity);

	return [...units];
}

type PlayheadUnits = {
	scene: Entity;
	frame: number;
	units: Entity[];
};

/** Resolves the active scene, its playhead and every affected clip once. */
function unitsAtPlayhead(world: World): PlayheadUnits | null {
	const scene = getActiveEntity(world);
	if (scene === null) return null;

	const frame = store(world, Computed).localTime[scene.id()] ?? 0;
	return { scene, frame, units: splitUnits(world, splitTargets(world, scene), frame) };
}

type TrimSide = 'left' | 'right';

type ClipSnapshot = {
	entity: Entity;
	start: number;
	end: number;
};

/**
 * Pulls later clips on the same sequence row to the left by `removed`.
 * Direct scene children are separate timeline rows, so they never move each
 * other. Capturing every start before moving keeps gaps between followers.
 */
function rippleSequenceFollowers(
	world: World,
	clip: ClipSnapshot,
	removed: number,
	shifted: Set<Entity>,
): void {
	if (removed <= 0) return;
	const parent = getParentEntity(clip.entity);
	if (parent === null || !isSequence(parent)) return;

	const computed = store(world, Computed);
	const followers = [...world.query(NODES, ChildOf(parent))]
		.filter((entity) => entity !== clip.entity && !shifted.has(entity))
		.map((entity) => ({ entity, start: computed.start[entity.id()] ?? 0 }))
		.filter(({ start }) => start >= clip.end)
		.sort((a, b) => a.start - b.start);

	for (const follower of followers) {
		moveEntityTo(world, follower.entity, follower.start - removed);
		shifted.add(follower.entity);
	}
}

/**
 * Ripple-trims every affected timeline row. Left keeps the source frame under
 * the playhead, moves that retained tail back to the previous cut and pulls
 * every follower with it. Right ends the clip at the playhead and pulls every
 * follower back from the next cut.
 */
function trimSideAtPlayhead(world: World, side: TrimSide): Entity[] {
	const target = unitsAtPlayhead(world);
	if (target === null || target.units.length === 0) return [];

	const computed = store(world, Computed);
	const clips: ClipSnapshot[] = target.units.map((entity) => ({
		entity,
		start: computed.start[entity.id()] ?? 0,
		end: computed.end[entity.id()] ?? 0,
	}));
	const shifted = new Set(target.units);
	let previousCut = Number.NEGATIVE_INFINITY;

	for (const clip of clips) {
		if (side === 'left') {
			const removed = target.frame - clip.start;
			trimIn(world, clip.entity, target.frame);
			moveEntityTo(world, clip.entity, clip.start);
			rippleSequenceFollowers(world, clip, removed, shifted);
			previousCut = Math.max(previousCut, clip.start);
		} else {
			const removed = clip.end - target.frame;
			trimOut(world, clip.entity, target.frame);
			rippleSequenceFollowers(world, clip, removed, shifted);
		}
	}

	if (side === 'left' && Number.isFinite(previousCut)) {
		setPlayhead(world, target.scene, previousCut);
	}
	getDocumentEditor(world).select(target.units);
	return target.units;
}

/** Ripple-deletes from the previous cut through the playhead. */
export function trimLeftAtPlayhead(world: World): Entity[] {
	return trimSideAtPlayhead(world, 'left');
}

/** Ripple-deletes from the playhead through the next cut. */
export function trimRightAtPlayhead(world: World): Entity[] {
	return trimSideAtPlayhead(world, 'right');
}

/**
 * Cuts every clip the playhead is over in two. Returns the tail halves, which
 * are what the selection is left on: they are the new elements, and carrying
 * on from the cut is the usual next thing to do to them.
 */
export function splitAtPlayhead(world: World): Entity[] {
	const target = unitsAtPlayhead(world);
	if (target === null || target.units.length === 0) return [];

	const { frame, units } = target;

	const editor = getDocumentEditor(world);
	// Copied before anything is trimmed, so each copy is spelled from the
	// whole clip and still runs to the end the whole clip ran to.
	const pairs = editor.duplicateInPlace(units);

	for (const { original, copy } of pairs) {
		trimOut(world, original, frame);
		trimIn(world, copy, frame);
		handOffDecoders(world, original, copy);
		// The two halves play the same source at the same zoom, so the copy
		// starts with the waveform and thumbnails the whole clip had rather
		// than decoding them all over again.
		clonePeaksForSplit(original.id(), copy.id());
		cloneFramesForSplit(original.id(), copy.id());
	}

	// Two clips side by side under a scene are two layers of the timeline,
	// where a sequence's children are one row: the halves are wrapped so a
	// clip that was cut still reads as the one clip it was. A parent that
	// already groups its children (a group, or a sequence — every sequence is
	// a group) does that for them.
	for (const { original, copy } of pairs) {
		const parent = getParentEntity(original);
		if (parent === null || isGroup(parent)) continue;

		editor.wrap([original, copy], () => <Sequence name={getNextName(world, 'Sequence')} />);
	}

	const copies = pairs.map(({ copy }) => copy);
	if (copies.length > 0) editor.select(copies);

	return copies;
}
