/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// Moving into a sequence never trims its neighbours. New sequence creation
// retains its separate, authored precedence rule below.
import { AdjustmentLayer, ChildOf, Computed, Geometry, Group, Sequential, getParentNode, isGroup, store } from '@diffusionstudio/runtime';
import { Or } from 'koota';

import { getDocumentEditor } from './editor';
import { moveEntityTo, trimIn, trimOut } from './timing';

import type { DocumentEditor } from './editor';
import type { Entity, World } from 'koota';

/** Places a dropped selection in the nearest free space, preserving all lengths and offsets. */
export function resolveDragJoins(world: World, dragged: Entity[]): void {
	const computed = store(world, Computed);
	const moving = new Set(dragged);
	const clips = dragged.map(entity => ({
		entity,
		start: computed.start[entity.id()] ?? 0,
		end: computed.end[entity.id()] ?? 0,
	}));
	const forbidden: { start: number; end: number }[] = [];
	let minimum = Number.NEGATIVE_INFINITY;
	for (const clip of clips) {
		minimum = Math.max(minimum, -clip.start);
		const parent = getParentNode(clip.entity);
		if (!parent?.has(Sequential) || clip.end <= clip.start) continue;
		for (const sibling of world.query(Or(Geometry, Group, AdjustmentLayer), ChildOf(parent))) {
			if (moving.has(sibling)) continue;
			const start = computed.start[sibling.id()] ?? 0;
			const end = computed.end[sibling.id()] ?? 0;
			if (end > start) forbidden.push({ start: start - clip.end, end: end - clip.start });
		}
	}
	// Open intervals: touching endpoints leave a valid exact-fit position.
	forbidden.sort((a, b) => a.start - b.start);
	const merged: typeof forbidden = [];
	for (const span of forbidden) {
		const last = merged[merged.length - 1];
		if (last && span.start < last.end) last.end = Math.max(last.end, span.end);
		else merged.push(span);
	}
	let delta = Math.max(0, minimum);
	for (const span of merged) {
		if (delta <= span.start || delta >= span.end) continue;
		delta = span.start >= minimum && delta - span.start < span.end - delta
			? span.start : span.end;
		break;
	}
	if (delta === 0) return;
	for (const clip of clips) moveEntityTo(world, clip.entity, clip.start + delta);
}


/**
 * Settles a sequence that has just been made, where there is no dropped clip
 * to be authoritative — every child merely happened to overlap on the canvas.
 * Precedence goes by start frame instead: the earlier clip keeps what it has
 * and the later one gives way.
 *
 * Each child is authoritative over all the ones after it, and every pass
 * trims all of them the same way, so their starts stay in order and the
 * "dropped inside a sibling" split cannot come up here — only edge trims and
 * removals.
 */
export function resolveNewSequenceOverlaps(world: World, sequence: Entity): void {
	if (!sequence.has(Sequential)) return;

	const editor = getDocumentEditor(world);
	const computed = store(world, Computed);

	const children = [...world.query(Or(Geometry, Group), ChildOf(sequence))]
		.sort((a, b) => (computed.start[a.id()] ?? 0) - (computed.start[b.id()] ?? 0));

	// Nothing to protect: there is no clip the user is holding, and the
	// recursion into a group has no leaves to skip.
	const ignore = new Set<Entity>();

	for (let i = 0; i < children.length; i++) {
		const entity = children[i]!;
		// An earlier authoritative clip may have trimmed or removed this one.
		if (!entity.isAlive()) continue;

		const occStart = computed.start[entity.id()];
		const occEnd = computed.end[entity.id()];
		if (occStart === undefined || occEnd === undefined || occEnd <= occStart) continue;

		for (let j = i + 1; j < children.length; j++) {
			const sibling = children[j]!;
			if (!sibling.isAlive()) continue;
			resolveEntityOverlap(world, editor, sibling, occStart, occEnd, ignore);
		}
	}
}

/**
 * Settles one entity against the span `[occStart, occEnd)` something else has
 * taken. A group has no time of its own to give up, so its leaves are settled
 * and its bounds follow from what is left of them.
 */
function resolveEntityOverlap(
	world: World,
	editor: DocumentEditor,
	entity: Entity,
	occStart: number,
	occEnd: number,
	ignore: Set<Entity>,
): void {
	const computed = store(world, Computed);

	const start = computed.start[entity.id()];
	const end = computed.end[entity.id()];
	if (start === undefined || end === undefined) return;

	// Touching edges abut, they do not overlap.
	if (end <= occStart || start >= occEnd) return;

	if (isGroup(entity)) {
		if (start >= occStart && end <= occEnd) {
			editor.remove(entity);
			return;
		}

		const children = [...world.query(Or(Geometry, Group), ChildOf(entity))]
			.filter((child) => !ignore.has(child));
		for (const child of children) {
			resolveEntityOverlap(world, editor, child, occStart, occEnd, ignore);
		}

		// Emptied of everything it held, the group is nothing on its own.
		if (world.query(Or(Geometry, Group), ChildOf(entity)).length === 0) {
			editor.remove(entity);
		}
		return;
	}

	const startCovered = start >= occStart;
	const endCovered = end <= occEnd;

	if (startCovered && endCovered) {
		editor.remove(entity);
	} else if (!startCovered && endCovered) {
		// Covered from its out point back: keep the head, ending at the drop.
		trimOut(world, entity, occStart);
	} else if (startCovered && !endCovered) {
		// Covered from its in point on: keep the tail, starting at the drop.
		trimIn(world, entity, occEnd);
	} else {
		// The drop landed inside it, so what is left is a head and a tail —
		// the same cut `splitAtPlayhead` makes, around a span rather than a
		// frame. Copied before either half is trimmed, so the copy is spelled
		// from the whole clip and still runs to the end it ran to.
		const [pair] = editor.duplicateInPlace([entity]);

		trimOut(world, entity, occStart);
		if (pair) trimIn(world, pair.copy, occEnd);
	}
}
