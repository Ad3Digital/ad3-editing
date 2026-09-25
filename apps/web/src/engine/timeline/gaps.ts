/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { Or, trait } from 'koota';
import { AdjustmentLayer, ChildOf, Computed, Geometry, Group, Selected, isSequence, store } from '@diffusionstudio/runtime';
import { getDocumentEditor } from '../editor';
import { getEditHistory } from '../history';
import { moveEntityTo } from '../timing';
import { framesToPixels, getResolution, getTimelineScene } from './view';
import type { Entity, World } from 'koota';
import type { RowCursor } from './layout';
import type { TimelineSurfaceState } from './surface';

const NODES = Or(Geometry, Group, AdjustmentLayer);
export const GapSelection = trait({ sequence: null as Entity | null, start: 0, end: 0 });

export function clearGapSelection(world: World): void {
	for (const scene of world.query(GapSelection)) scene.remove(GapSelection);
}

/** Empty intervals between the union of occupied spans, never inside a clip. */
function sequenceGaps(world: World, sequence: Entity): { start: number; end: number }[] {
	const computed = store(world, Computed);
	const spans = [...world.query(NODES, ChildOf(sequence))]
		.map(entity => ({ start: computed.start[entity.id()] ?? 0, end: computed.end[entity.id()] ?? 0 }))
		.filter(span => span.end > span.start)
		.sort((a, b) => a.start - b.start);
	const gaps: { start: number; end: number }[] = [];
	let end = spans[0]?.end ?? 0;
	for (let i = 1; i < spans.length; i++) {
		const next = spans[i]!;
		if (next.start > end) gaps.push({ start: end, end: next.start });
		end = Math.max(end, next.end);
	}
	return gaps;
}

/** A fresh selection is required: never ripple stale coordinates or another scene. */
export function closeSelectedGaps(world: World): boolean {
	const scene = getTimelineScene(world);
	const gap = scene?.get(GapSelection);
	if (!gap) return false;
	clearGapSelection(world);
	if (world.query(Selected).length) return false;
	const sequence = gap.sequence;
	if (!sequence?.isAlive() || !isSequence(sequence)) return true;
	if (!sequenceGaps(world, sequence).some(span => span.start === gap.start && span.end === gap.end)) return true;
	const computed = store(world, Computed);
	// Ripple later content across the scene, including overlays and detached audio.
	// Move a container once, never both it and its descendants.
	const followers: { entity: Entity; start: number }[] = [];
	const collect = (parent: Entity): void => {
		for (const entity of world.query(NODES, ChildOf(parent))) {
			const start = computed.start[entity.id()] ?? 0;
			if (start >= gap.end) followers.push({ entity, start });
			else collect(entity);
		}
	};
	collect(scene!);
	const history = getEditHistory(world);
	history.beginGesture();
	try {
		for (const clip of followers) moveEntityTo(world, clip.entity, clip.start - (gap.end - gap.start));
	} finally {
		history.endGesture();
	}
	return true;
}

/** Registers only real gaps in this row, below the clips' handles. */
export function renderGaps(world: World, scene: Entity, surface: TimelineSurfaceState, sequence: Entity, row: RowCursor): void {
	const { ctx, pointer } = surface;
	if (!ctx || !pointer) return;
	const gaps = sequenceGaps(world, sequence);
	const resolution = getResolution(world, scene);
	let selected = scene.get(GapSelection);
	if (selected?.sequence === sequence && (!gaps.some(gap => gap.start === selected!.start && gap.end === selected!.end) || world.query(Selected).length)) {
		scene.remove(GapSelection);
		selected = undefined;
	}
	pointer.scope(`${sequence.id()}/gaps`);
	for (const gap of gaps) {
		const x = framesToPixels(gap.start, resolution);
		const width = framesToPixels(gap.end, resolution) - x;
		const { clicked, dragging } = pointer.region(x, 0, width, row.height, `${gap.start}-${gap.end}`);
		if (clicked) {
			getDocumentEditor(world).clearSelection();
			clearGapSelection(world);
			scene.add(GapSelection);
			scene.set(GapSelection, { sequence, ...gap });
			selected = scene.get(GapSelection);
		}
		// Starting a marquee in a gap remains possible.
		if (dragging && pointer.position && pointer.position.state !== 'idle') {
			clearGapSelection(world);
			const p = pointer.position;
			surface.marquee = { x: Math.min(p.currentX, p.initialX), y: Math.min(p.currentY, p.initialY), width: Math.abs(p.deltaX), height: Math.abs(p.deltaY) };
		}
		if (selected?.sequence !== sequence || selected.start !== gap.start || selected.end !== gap.end) continue;
		ctx.save();
		ctx.fillStyle = surface.colors.border.ring;
		ctx.globalAlpha = 0.2;
		ctx.fillRect(x, 0, width, row.height);
		ctx.globalAlpha = 1;
		ctx.strokeStyle = surface.colors.border.ring;
		ctx.strokeRect(x + 0.5, 0.5, Math.max(0, width - 1), row.height - 1);
		ctx.restore();
	}
}
