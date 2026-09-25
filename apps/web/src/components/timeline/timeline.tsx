/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { For, Show, onCleanup, onMount } from 'solid-js';
import { toast } from 'somoto';
import { useWorld } from '@diffusionstudio/koota-solid';
import { FrameRate, framesToSeconds, getActiveEntity } from '@diffusionstudio/runtime';
import { droppedFiles, importFiles } from '@/engine/asset-actions';
import { insertAsset } from '@/engine/insert-asset';
import { insertAssetsInNewScene } from '@/engine/new-scene';
import { useLibrary } from '@/engine/library';
import { useTimeline } from '@/context/timeline';
import { ASSET_DRAG_TYPE } from '@/components/sidebar-left/folder-item';
import { useDerived } from '@/engine/hooks/use-derived';
import { PIN_COLORS, closePinPicker, pinControls } from '@/engine/timeline/pins';

/**
 * The timeline's canvas. What is drawn on it is the timeline system's
 * business (see `@/engine/timeline`); this is the element it draws on and
 * what dropping an asset onto it means.
 */
export function Timeline() {
  const world = useWorld();
  const timeline = useTimeline();
  const library = useLibrary();
  const pins = pinControls(world);
  const pickerOpen = useDerived(() => pins.pickerScene !== null && pins.pickerScene === getActiveEntity(world));
  const color = useDerived(() => pins.color);
  const dismissPicker = (event: PointerEvent) => {
    if (event.target instanceof Element && !event.target.closest('[data-pin-picker]')) closePinPicker(world);
  };
  onMount(() => document.addEventListener('pointerdown', dismissPicker));
  onCleanup(() => document.removeEventListener('pointerdown', dismissPicker));

  onMount(() => timeline.attachCanvas());
  onCleanup(() => timeline.detachCanvas());

  /**
   * Assets dropped on the timeline start where they were dropped, unlike
   * ones dropped on the canvas, which start at the playhead: the whole point
   * of aiming at a place on the timeline is to say when.
   *
   * With no scene to drop into, they get one of their own rather than
   * landing loose at the root, sized to the last of them that has a size
   * (see `insertAssetsInNewScene`).
   */
  const handleDrop = async (event: DragEvent) => {
    event.preventDefault();
    event.stopPropagation();

    const lib = library();
    if (!lib) return;

    const fps = world.get(FrameRate)?.value ?? 30;
    const start = framesToSeconds(Math.max(0, timeline.clientToFrame(event.clientX)), fps);

    // Read the transfer before the first await: it is gone by the time an
    // import resolves.
    const ids = event.dataTransfer?.getData(ASSET_DRAG_TYPE)?.split(',').filter(Boolean) ?? [];
    const files = droppedFiles(event);

    const assets = ids.map((id) => lib.get(id)).filter((asset) => asset != null);
    if (files.length) assets.push(...await importFiles(lib, files, ''));
    if (!assets.length) return;

    if (!getActiveEntity(world)) {
      if (!insertAssetsInNewScene(world, assets, { start })) {
        toast("Nothing to insert into", { description: "Open a project first." });
      }
      return;
    }

    for (const asset of assets) {
      if (!insertAsset(world, asset, { start })) {
        toast("Nothing to insert into", { description: "Open a scene first." });
      }
    }
  };

  const handleDragOver = (event: DragEvent) => {
    event.preventDefault();
    event.dataTransfer!.dropEffect = 'copy';
  };

  return (
    <div class="relative size-full">
      <canvas
        class="absolute inset-0"
        id="timeline-canvas"
        on:drop={handleDrop}
        on:dragover={handleDragOver}
      />
      <Show when={pickerOpen()}>
        <div
          data-pin-picker
          role="dialog"
          aria-label="Cores dos pins"
          class="absolute left-2 top-10 z-50 rounded-lg border border-input bg-background p-3 shadow-lg"
          onKeyDown={(event) => {
            event.stopPropagation();
            if (event.key === 'Escape') closePinPicker(world);
          }}
        >
          <div class="mb-2 flex items-center justify-between gap-4 text-xs">
            <span>Cores dos pins</span>
            <button type="button" aria-label="Fechar cores dos pins" onClick={() => closePinPicker(world)}>Fechar</button>
          </div>
          <div class="flex gap-2">
            <For each={PIN_COLORS}>{(swatch) => (
              <button
                type="button"
                class="size-6 rounded-full border-2 focus-visible:outline-2 focus-visible:outline-offset-2"
                style={{ 'background-color': swatch.hex, 'border-color': color() === swatch.hex ? 'white' : 'transparent' }}
                aria-label={swatch.label}
                aria-pressed={color() === swatch.hex}
                title={swatch.label}
                onClick={() => {
                  pins.color = swatch.hex;
                  closePinPicker(world);
                }}
              />
            )}</For>
          </div>
          <p class="mt-2 text-xs text-muted-foreground">Toque em ' para colocar um pin. Segure por 2s para escolher a cor.</p>
        </div>
      </Show>
    </div>
  );
}
