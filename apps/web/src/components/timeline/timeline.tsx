/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { For, Show, createSignal, onCleanup, onMount } from 'solid-js';
import { toast } from 'somoto';
import { useWorld } from '@diffusionstudio/koota-solid';
import { FrameRate, Selected, Source, framesToSeconds, getActiveEntity } from '@diffusionstudio/runtime';
import { droppedFiles, importFiles } from '@/engine/asset-actions';
import { insertAsset } from '@/engine/insert-asset';
import { insertAssetsInNewScene } from '@/engine/new-scene';
import { useLibrary } from '@/engine/library';
import { useTimeline } from '@/context/timeline';
import { ASSET_DRAG_TYPE } from '@/components/sidebar-left/folder-item';
import { useDerived } from '@/engine/hooks/use-derived';
import { PIN_COLORS, closePinPicker, pinControls, hitPin, removePin } from '@/engine/timeline/pins';
import { TimelineSurface } from '@/engine/timeline/surface';
import { getTimelineScene } from '@/engine/timeline/view';
import { ProjectConfig } from '@/engine/traits';
import { getDocumentEditor } from '@/engine/editor';
import type { Entity } from 'koota';

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
  const [clipMenu, setClipMenu] = createSignal<{ x: number; y: number; entities: Entity[] } | null>(null);
  const changeClipColor = (color: string | null) => {
    const entities = clipMenu()?.entities;
    setClipMenu(null);
    if (entities) void world.get(ProjectConfig)?.setClipColors(entities, color).catch(() => toast.error('Não foi possível salvar a cor dos clipes.'));
  };
  const contextMenu = (event: MouseEvent) => {
    const surface = world.get(TimelineSurface);
    const scene = getTimelineScene(world);
    const rect = surface?.canvas?.getBoundingClientRect();
    if (!scene || !rect) return;
    const pin = hitPin(world, scene, event.clientX - rect.left, event.clientY - rect.top);
    if (pin >= 0) {
      event.preventDefault(); event.stopPropagation(); setClipMenu(null); removePin(world, scene, pin); return;
    }
    const id = surface?.pointer?.hitAt(event.clientX, event.clientY)?.split('/')[0];
    const entity = world.query(Source).find(e => String(e.id()) === id);
    if (!entity || event.clientY - rect.top < 30) return;
    event.preventDefault(); event.stopPropagation();
    if (!entity.has(Selected)) getDocumentEditor(world).select(entity);
    const entities = entity.has(Selected) ? [...world.query(Selected, Source)] : [entity];
    setClipMenu({ x: Math.max(0, Math.min(rect.width - 240, event.clientX - rect.left)), y: Math.max(0, Math.min(rect.height - 145, event.clientY - rect.top)), entities });
    queueMicrotask(() => document.querySelector<HTMLButtonElement>('[data-clip-color-picker] button')?.focus());
  };
  const pickerOpen = useDerived(() => pins.pickerScene !== null && pins.pickerScene === getActiveEntity(world));
  const color = useDerived(() => pins.color);
  const dismissPicker = (event: PointerEvent) => {
    if (event.target instanceof Element && !event.target.closest('[data-pin-picker]')) closePinPicker(world);
    if (event.target instanceof Element && !event.target.closest('[data-clip-color-picker]')) setClipMenu(null);
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
        on:contextmenu={contextMenu}
      />
      <Show when={clipMenu()}>{(menu) => (
        <div data-clip-color-picker role="dialog" aria-label="Cor dos clipes" class="absolute z-50 w-60 rounded-lg border border-input bg-background p-3 shadow-lg"
          style={{ left: `${menu().x}px`, top: `${menu().y}px` }}
          onKeyDown={event => { event.stopPropagation(); if (event.key === 'Escape') setClipMenu(null); }}>
          <div class="mb-3 text-xs">Cor do clipe{menu().entities.length > 1 ? ` (${menu().entities.length} selecionados)` : ''}</div>
          <div class="flex gap-1.5">
            <For each={PIN_COLORS}>{swatch => (
              <button type="button" class="size-5 rounded-full border border-white/30" style={{ 'background-color': swatch.hex }}
                aria-label={`Clipe ${swatch.label}`} title={swatch.label} onClick={() => changeClipColor(swatch.hex)} />
            )}</For>
          </div>
          <button type="button" class="mt-3 text-xs text-muted-foreground" onClick={() => changeClipColor(null)}>Restaurar cor padrão</button>
        </div>
      )}</Show>
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
          <p class="mt-2 text-xs text-muted-foreground">' adiciona ou remove no cursor. Arraste para mover; botão direito remove. Segure por 2s para escolher a cor.</p>
        </div>
      </Show>
    </div>
  );
}
