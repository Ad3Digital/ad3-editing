/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { onMount, type JSX } from 'solid-js';
import { useEngineContext } from './context';

/**
 * Mounts the engine onto a canvas sized to fill its container, and starts
 * the tick loop. Must render under an EngineProvider. Camera gestures are a
 * separate concern: render a CameraController alongside it.
 */
export function EngineCanvas(): JSX.Element {
	const engine = useEngineContext();
	let canvasRef!: HTMLCanvasElement;
	let containerRef!: HTMLDivElement;

	onMount(() => {
		engine.mount(canvasRef);
		engine.start();
	});

	return (
		<div ref={containerRef} class="relative size-full">
			<canvas ref={canvasRef} class="absolute inset-0" />
		</div>
	);
}
