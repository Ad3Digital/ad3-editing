/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { trait } from 'koota';
import { Computed, FrameRate, Time, getActiveEntity } from '@diffusionstudio/runtime';
import { toast } from 'somoto';
import { ProjectConfig } from '../traits';
import { sceneConfigKey } from '../project-config';
import { RULER_HEIGHT } from './config';
import { framesToPixels, getResolution, getScrollX } from './view';
import type { Entity, World } from 'koota';
import type { TimelineMarker } from '../project-config';
import type { TimelineSurfaceState } from './surface';

export const PIN_COLORS = [
	{ value: 'yellow', label: 'Amarelo', hex: '#F59E0B' },
	{ value: 'blue', label: 'Azul', hex: '#3B82F6' },
	{ value: 'green', label: 'Verde', hex: '#10B981' },
	{ value: 'pink', label: 'Rosa', hex: '#EC4899' },
	{ value: 'purple', label: 'Roxo', hex: '#A855F7' },
	{ value: 'orange', label: 'Laranja', hex: '#F97316' },
	{ value: 'cyan', label: 'Ciano', hex: '#06B6D4' },
	{ value: 'red', label: 'Vermelho', hex: '#EF4444' },
] as const;

// Transient controls belong to the world; pin data belongs to ProjectConfig.
export const PinControls = trait(() => ({
	color: '#EC4899',
	pressedAt: null as number | null,
	pressedScene: null as Entity | null,
	pickerScene: null as Entity | null,
}));
const EMPTY_PINS: readonly TimelineMarker[] = [];
export const PIN_HOLD_MS = 2000;

export function pinControls(world: World) {
	if (!world.has(PinControls)) world.add(PinControls);
	return world.get(PinControls)!;
}

export function scenePins(world: World, scene: Entity): readonly TimelineMarker[] {
	const key = sceneConfigKey(scene);
	return key ? world.get(ProjectConfig)?.markers()[key] ?? EMPTY_PINS : EMPTY_PINS;
}

export function addPinAtPlayhead(world: World): void {
	const scene = getActiveEntity(world);
	if (!scene) return;
	const config = world.get(ProjectConfig);
	if (!config?.ready() || !sceneConfigKey(scene)) {
		toast.error('Aguarde o carregamento do projeto para adicionar pins.');
		return;
	}
	const fps = world.get(FrameRate)?.value ?? 30;
	const frame = Math.max(0, Math.round(scene.get(Computed)?.localTime ?? 0));
	const pins = [...scenePins(world, scene), { time: frame / fps, color: pinControls(world).color }];
	void config.setSceneMarkers(scene, pins).catch(() => toast.error('Não foi possível salvar os pins do projeto.'));
}

export function clearPins(world: World): void {
	const scene = getActiveEntity(world);
	const config = world.get(ProjectConfig);
	if (!scene || !config?.ready()) return;
	void config.setSceneMarkers(scene, []).catch(() => toast.error('Não foi possível salvar os pins do projeto.'));
}

export function closePinPicker(world: World): void {
	pinControls(world).pickerScene = null;
}

export function onPinPressed(world: World): void {
	const state = pinControls(world);
	state.pressedScene = getActiveEntity(world);
	state.pressedAt = state.pressedScene === null ? null : world.get(Time)?.now ?? 0;
}

export function onPinLifted(world: World): void {
	const state = pinControls(world);
	if (state.pressedAt !== null && state.pressedScene === getActiveEntity(world)) {
		if ((world.get(Time)?.now ?? 0) - state.pressedAt >= PIN_HOLD_MS) state.pickerScene = state.pressedScene;
		else addPinAtPlayhead(world);
	}
	state.pressedAt = null;
	state.pressedScene = null;
}

export function updatePinHold(world: World, held: Set<string>): void {
	const state = world.get(PinControls);
	if (!state) return;
	const scene = getActiveEntity(world);
	if (state.pickerScene !== scene) state.pickerScene = null;
	if (!held.has("'") || held.has('mod') || state.pressedScene !== scene) {
		state.pressedAt = null;
		state.pressedScene = null;
		return;
	}
	if (state.pressedAt !== null && (world.get(Time)?.now ?? 0) - state.pressedAt >= PIN_HOLD_MS) {
		state.pickerScene = scene;
		state.pressedAt = null; // Consume once; release and key repeat cannot add a pin.
	}
}

/** Colored flags follow timeline zoom and scroll; they never render into the video. */
export function renderPins(world: World, scene: Entity, surface: TimelineSurfaceState): void {
	const { ctx } = surface;
	if (!ctx) return;
	const resolution = getResolution(world, scene);
	const scroll = getScrollX(world, scene) * resolution;
	const fps = world.get(FrameRate)?.value ?? 30;
	ctx.save();
	for (const pin of scenePins(world, scene)) {
		const x = framesToPixels(pin.time * fps, resolution) - scroll;
		if (x < -5 || x > surface.layout.width + 5) continue;
		ctx.fillStyle = pin.color;
		if (!surface.minimized) {
			ctx.globalAlpha = 0.3;
			ctx.fillRect(x, RULER_HEIGHT, 1, surface.layout.height - RULER_HEIGHT);
		}
		ctx.globalAlpha = 1;
		ctx.beginPath();
		ctx.moveTo(x - 5, 2);
		ctx.lineTo(x + 5, 2);
		ctx.lineTo(x + 5, 11);
		ctx.lineTo(x, 16);
		ctx.lineTo(x - 5, 11);
		ctx.closePath();
		ctx.fill();
	}
	ctx.restore();
}
