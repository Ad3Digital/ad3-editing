/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { AssetId } from "@diffusionstudio/runtime";
import { useQuery, useWorld } from "@diffusionstudio/koota-solid";
import { DEFAULT_HYPERFRAMES_SETTINGS, generateTemplate } from "@ad3/hyperframes-engine/templates";
import { For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";

import { Button } from "@/components/ui/button";
import { useProject } from "@/context/project";
import {
  cancelHyperframes,
  getHyperframesJob,
  listHyperframes,
  previewHyperframes,
  renderHyperframes,
  saveHyperframes,
  statusHyperframes,
} from "@/engine/hyperframes";
import { insertAssetAtPlayhead } from "@/engine/asset-actions";
import { useLibrary } from "@/engine/library";
import { forgetAssetMedia } from "@/engine/timeline/media";
import { forgetAssetPeaks } from "@/engine/timeline/peaks";
import { revealPath } from "@/lib/shell";

import type { Asset, AssetLibrary } from "@diffusionstudio/assets";
import type {
  HyperframesComposition,
  HyperframesDraft,
  HyperframesJob,
  HyperframesRenderResult,
  HyperframesRuntimeStatus,
  HyperframesSettings,
  HyperframesTemplate,
} from "@ad3/hyperframes-engine/types";

const JOB_IS_TERMINAL: Record<HyperframesJob["state"], boolean> = {
  running: false,
  completed: true,
  failed: true,
  cancelled: true,
};
const TEMPLATE_LABELS: Record<Exclude<HyperframesTemplate, "custom">, string> = {
  title: "Title",
  "lower-third": "Lower third",
  stat: "Stat",
};

function templateDraft(template: Exclude<HyperframesTemplate, "custom">): HyperframesDraft {
  const settings: HyperframesSettings = { ...DEFAULT_HYPERFRAMES_SETTINGS, template };
  return { name: TEMPLATE_LABELS[template], settings, html: generateTemplate(settings) };
}

function compositionDraft(composition: HyperframesComposition): HyperframesDraft {
  return {
    id: composition.id,
    name: composition.name,
    settings: { ...composition.settings },
    html: composition.html,
  };
}

/**
 * Render revisions are immutable. The library relinks their project-relative
 * source while preserving the user's asset path and every authored clip's
 * start, trims and transforms.
 */
async function linkRenderedAsset(
  library: AssetLibrary,
  result: HyperframesRenderResult,
): Promise<Asset> {
  const generation = { key: "hyperframes", id: result.compositionId };
  let asset = library.list().find((candidate) =>
    candidate.generation?.key === generation.key && candidate.generation?.id === generation.id,
  );

  if (asset) {
    forgetAssetMedia(asset.id);
    forgetAssetPeaks(asset.id);
    asset = await library.relink(asset, result.source, { frameRate: result.fps });
  } else {
    const imported = await library.import([result.source], { folder: "HyperFrames", generation, frameRate: result.fps });
    if (imported.failed.length) throw imported.failed[0]!.error;
    asset = imported.assets[0];
    if (!asset) throw new Error("The completed HyperFrames render produced no importable media.");
  }

  return library.update(asset, { generation });
}

export function HyperframesPanel() {
  const project = useProject();
  const library = useLibrary();
  const world = useWorld();
  const mediaEntities = useQuery(AssetId);
  const [status, setStatus] = createSignal<HyperframesRuntimeStatus | null>(null);
  const [compositions, setCompositions] = createSignal<HyperframesComposition[]>([]);
  const [draft, setDraft] = createSignal<HyperframesDraft>(templateDraft("title"));
  const [previewUrl, setPreviewUrl] = createSignal<string | null>(null);
  const [job, setJob] = createSignal<HyperframesJob | null>(null);
  const [renderedAsset, setRenderedAsset] = createSignal<Asset | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [loading, setLoading] = createSignal(false);
  let request = 0;
  let poll: number | undefined;

  const sourcePath = createMemo(() => draft().id ? `hyperframes/${draft().id}/index.html` : "Save to create hyperframes/<id>/index.html");
  const canUseEngine = createMemo(() => status()?.available === true);
  const linkedClipExists = createMemo(() => {
    const asset = renderedAsset();
    return !!asset && mediaEntities().some((entity) => entity.get(AssetId)?.value === asset.id);
  });

  const stopPolling = () => {
    window.clearTimeout(poll);
    poll = undefined;
  };

  const replaceComposition = (next: HyperframesComposition) => {
    setCompositions((current) => [next, ...current.filter((item) => item.id !== next.id)]);
  };

  const refresh = async (dir: string) => {
    const token = ++request;
    stopPolling();
    setJob(null);
    setRenderedAsset(null);
    setSaving(false);
    setPreviewUrl(null);
    setError(null);
    setLoading(true);
    try {
      const runtime = await statusHyperframes();
      if (token !== request) return;
      setStatus(runtime);
      if (!runtime.available) {
        setCompositions([]);
        return;
      }
      const found = await listHyperframes(dir);
      if (token !== request) return;
      setCompositions(found);
      const current = draft().id;
      const reopened = current ? found.find((item) => item.id === current) : undefined;
      if (reopened) setDraft(compositionDraft(reopened));
    } catch (cause) {
      if (token !== request) return;
      setStatus({ available: false, version: "", reason: (cause as Error).message });
      setCompositions([]);
    } finally {
      if (token === request) setLoading(false);
    }
  };

  createEffect(() => {
    const dir = project.dir();
    void refresh(dir);
  });

  onCleanup(() => {
    request += 1;
    stopPolling();
  });

  const updateDraft = (patch: Partial<HyperframesDraft>) => setDraft((current) => ({ ...current, ...patch }));
  const updateSettings = (patch: Partial<HyperframesSettings>) =>
    setDraft((current) => {
      const settings = { ...current.settings, ...patch };
      const generated = current.settings.template !== "custom" &&
        current.html === generateTemplate(current.settings);
      return { ...current, settings, html: generated ? generateTemplate(settings) : current.html };
    });

  const newComposition = (template: Exclude<HyperframesTemplate, "custom">) => {
    request += 1;
    stopPolling();
    setSaving(false);
    setJob(null);
    setRenderedAsset(null);
    setPreviewUrl(null);
    setError(null);
    setDraft(templateDraft(template));
  };

  const applyTemplate = (template: Exclude<HyperframesTemplate, "custom">) => {
    const settings = { ...draft().settings, template };
    setDraft((current) => ({ ...current, settings, html: generateTemplate(settings) }));
    setPreviewUrl(null);
  };

  const selectComposition = async (id: string) => {
    const token = ++request;
    stopPolling();
    setJob(null);
    setSaving(false);
    setRenderedAsset(null);
    setPreviewUrl(null);
    setError(null);
    try {
      const found = await listHyperframes(project.dir());
      if (token !== request) return;
      setCompositions(found);
      const composition = found.find((item) => item.id === id);
      if (!composition) throw new Error("This HyperFrames composition no longer exists.");
      // Reload the persisted source: a user's coding agent may have edited it
      // since the last panel visit, and choosing a composition must never apply
      // a template over that source.
      setDraft(compositionDraft(composition));
      if (composition.rendered?.revision === composition.revision && library()) {
        const asset = await linkRenderedAsset(library()!, composition.rendered);
        if (token === request) setRenderedAsset(asset);
      }
    } catch (cause) {
      if (token === request) setError((cause as Error).message);
    }
  };

  const saveCurrent = async (): Promise<HyperframesComposition | null> => {
    if (!canUseEngine()) return null;
    const token = request;
    setSaving(true);
    setError(null);
    try {
      const saved = await saveHyperframes(project.dir(), draft());
      if (token !== request) return null;
      replaceComposition(saved);
      setDraft(compositionDraft(saved));
      return saved;
    } catch (cause) {
      if (token === request) setError((cause as Error).message);
      return null;
    } finally {
      if (token === request) setSaving(false);
    }
  };

  const previewCurrent = async () => {
    const saved = await saveCurrent();
    if (!saved) return;
    const token = request;
    try {
      const preview = await previewHyperframes(project.dir(), saved.id);
      if (token === request) setPreviewUrl(preview.url);
    } catch (cause) {
      if (token === request) setError((cause as Error).message);
    }
  };

  const completeRender = async (next: HyperframesJob, token: number) => {
    if (!next.result) {
      setError("The render completed without media output.");
      return;
    }
    const currentLibrary = library();
    if (!currentLibrary) {
      setError("Open a project before importing the rendered HyperFrames media.");
      return;
    }
    try {
      const asset = await linkRenderedAsset(currentLibrary, next.result);
      if (token === request) setRenderedAsset(asset);
    } catch (cause) {
      if (token === request) setError((cause as Error).message);
    }
  };

  const receiveJob = async (next: HyperframesJob, token: number) => {
    if (token !== request) return;
    setJob(next);
    if (!JOB_IS_TERMINAL[next.state]) return;
    stopPolling();
    if (next.state === "completed") await completeRender(next, token);
    if (next.state === "failed") setError(next.error || "HyperFrames render failed.");
  };

  const pollJob = async (jobId: string, token: number) => {
    try {
      const next = await getHyperframesJob(project.dir(), jobId);
      await receiveJob(next, token);
      if (token === request && !JOB_IS_TERMINAL[next.state]) {
        poll = window.setTimeout(() => void pollJob(jobId, token), 800);
      }
    } catch (cause) {
      if (token === request) setError((cause as Error).message);
    }
  };

  const renderCurrent = async () => {
    const saved = await saveCurrent();
    if (!saved) return;
    const token = request;
    stopPolling();
    setPreviewUrl(null);
    try {
      const next = await renderHyperframes(project.dir(), saved.id);
      await receiveJob(next, token);
      if (token === request && !JOB_IS_TERMINAL[next.state]) {
        poll = window.setTimeout(() => void pollJob(next.id, token), 800);
      }
    } catch (cause) {
      if (token === request) setError((cause as Error).message);
    }
  };

  const cancelCurrent = async () => {
    const current = job();
    if (!current || JOB_IS_TERMINAL[current.state]) return;
    const token = request;
    try {
      await receiveJob(await cancelHyperframes(project.dir(), current.id), token);
    } catch (cause) {
      if (token === request) setError((cause as Error).message);
    }
  };

  const insertRendered = () => {
    const asset = renderedAsset();
    if (!asset || linkedClipExists()) return;
    insertAssetAtPlayhead(world, asset);
  };

  const revealSource = () => {
    const id = draft().id;
    if (!id) return;
    void revealPath(`${project.dir()}/hyperframes/${id}/index.html`).catch((cause) =>
      setError((cause as Error).message),
    );
  };

  return (
    <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div class="flex h-10 shrink-0 items-center justify-between border-b border-border px-4">
        <div class="text-xs font-450">HyperFrames</div>
        <Show when={loading()}><span class="text-xxs text-muted-foreground">Loading…</span></Show>
      </div>

      <Show
        when={canUseEngine()}
        fallback={
          <div class="p-4 text-xs text-muted-foreground">
            <p class="text-foreground">HyperFrames is unavailable</p>
            <p class="mt-1 leading-5">{status()?.reason ?? "Checking the local desktop runtime…"}</p>
          </div>
        }
      >
        <div class="min-h-0 flex-1 overflow-y-auto pb-4">
          <section class="border-b border-border p-4">
            <div class="mb-2 flex flex-col gap-2">
              <span class="text-xs font-450">Compositions</span>
              <div class="flex flex-wrap gap-1">
                <For each={Object.keys(TEMPLATE_LABELS) as Exclude<HyperframesTemplate, "custom">[]}>
                  {(template) => (
                    <Button variant="secondary" size="small" onClick={() => newComposition(template)}>
                      New {TEMPLATE_LABELS[template]}
                    </Button>
                  )}
                </For>
              </div>
            </div>
            <Show when={compositions().length} fallback={<p class="text-xxs text-muted-foreground">No saved compositions yet.</p>}>
              <div class="flex flex-col gap-1">
                <For each={compositions()}>
                  {(composition) => (
                    <Button
                      variant={draft().id === composition.id ? "on" : "ghost"}
                      size="small"
                      class="justify-start px-2"
                      onClick={() => void selectComposition(composition.id)}
                    >
                      {composition.name}
                    </Button>
                  )}
                </For>
              </div>
            </Show>
          </section>

          <section class="border-b border-border p-4">
            <div class="mb-2 text-xs font-450">Composition</div>
            <div class="grid grid-cols-2 gap-2">
              <label class="col-span-2 flex flex-col gap-1 text-xxs text-muted-foreground">Name
                <input class="h-7 rounded border border-border bg-background px-2 text-xs text-foreground" value={draft().name} onInput={(event) => updateDraft({ name: event.currentTarget.value })} />
              </label>
              <label class="flex flex-col gap-1 text-xxs text-muted-foreground">Title
                <input class="h-7 rounded border border-border bg-background px-2 text-xs text-foreground" value={draft().settings.title} onInput={(event) => updateSettings({ title: event.currentTarget.value })} />
              </label>
              <label class="flex flex-col gap-1 text-xxs text-muted-foreground">Subtitle
                <input class="h-7 rounded border border-border bg-background px-2 text-xs text-foreground" value={draft().settings.subtitle} onInput={(event) => updateSettings({ subtitle: event.currentTarget.value })} />
              </label>
              <label class="flex flex-col gap-1 text-xxs text-muted-foreground">Accent
                <input type="color" class="h-7 w-full rounded border border-border bg-background" value={draft().settings.accent} onInput={(event) => updateSettings({ accent: event.currentTarget.value })} />
              </label>
              <label class="flex flex-col gap-1 text-xxs text-muted-foreground">Background
                <input type="color" class="h-7 w-full rounded border border-border bg-background" value={draft().settings.background} onInput={(event) => updateSettings({ background: event.currentTarget.value })} />
              </label>
              <NumberField label="Duration (s)" value={draft().settings.duration} onChange={(duration) => updateSettings({ duration })} />
              <NumberField label="FPS" value={draft().settings.fps} onChange={(fps) => updateSettings({ fps })} />
              <NumberField label="Width" value={draft().settings.width} onChange={(width) => updateSettings({ width })} />
              <NumberField label="Height" value={draft().settings.height} onChange={(height) => updateSettings({ height })} />
              <label class="col-span-2 flex items-center gap-2 text-xs text-foreground">
                <input type="checkbox" checked={draft().settings.transparent} onInput={(event) => updateSettings({ transparent: event.currentTarget.checked })} />
                Transparent PNG sequence
              </label>
            </div>
            <div class="mt-3 flex flex-wrap gap-1">
              <For each={Object.keys(TEMPLATE_LABELS) as Exclude<HyperframesTemplate, "custom">[]}>
                {(template) => <Button variant="outline" size="small" onClick={() => applyTemplate(template)}>Apply {TEMPLATE_LABELS[template]} template</Button>}
              </For>
            </div>
            <p class="mt-2 text-xxs leading-4 text-muted-foreground">Settings update untouched templates. Edited HTML is preserved; applying a template explicitly replaces it.</p>
          </section>

          <section class="border-b border-border p-4">
            <div class="mb-1 text-xs font-450">HTML source</div>
            <p class="mb-2 break-all font-mono text-xxs text-muted-foreground">{sourcePath()}</p>
            <Button variant="outline" size="small" disabled={!draft().id} onClick={revealSource}>Reveal source</Button>
            <textarea
              aria-label="HyperFrames HTML source"
              class="min-h-48 w-full resize-y rounded border border-border bg-background p-2 font-mono text-xxs leading-4 text-foreground"
              value={draft().html}
              spellcheck={false}
              onInput={(event) => updateDraft({ html: event.currentTarget.value })}
            />
            <p class="mt-2 text-xxs leading-4 text-muted-foreground">This is the actual saved HTML. Your coding agent can edit the file at the path above; reopen the composition to reload it.</p>
          </section>

          <section class="p-4">
            <div class="flex flex-wrap gap-2">
              <Button disabled={saving()} onClick={() => void saveCurrent()}>Save</Button>
              <Button variant="secondary" disabled={saving()} onClick={() => void previewCurrent()}>Preview</Button>
              <Button variant="secondary" disabled={saving() || (job()?.state === "running")} onClick={() => void renderCurrent()}>Render</Button>
              <Show when={job()?.state === "running"}>
                <Button variant="destructive" onClick={() => void cancelCurrent()}>Cancel</Button>
              </Show>
              <Show when={renderedAsset()}>
                <Button variant="outline" disabled={linkedClipExists()} onClick={insertRendered}>
                  {linkedClipExists() ? "Linked clip updated" : "Insert at playhead"}
                </Button>
              </Show>
            </div>
            <Show when={job()}>
              {(current) => <p role="status" aria-live="polite" class="mt-3 whitespace-pre-wrap break-words text-xxs leading-4 text-muted-foreground">{current().log || current().state}</p>}
            </Show>
            <Show when={error()}>
              {(message) => <p role="alert" class="mt-3 text-xxs leading-4 text-destructive">{message()}</p>}
            </Show>
          </section>

          <Show when={previewUrl()}>
            {(url) => (
              <section class="border-t border-border p-4">
                <div class="mb-2 text-xs font-450">Preview</div>
                <iframe title="HyperFrames preview" class="aspect-video w-full rounded border border-border bg-black" sandbox="allow-scripts allow-same-origin" src={url()} onLoad={(event) => event.currentTarget.scrollIntoView({ block: "nearest", inline: "nearest" })} />
              </section>
            )}
          </Show>
        </div>
      </Show>
    </div>
  );
}

function NumberField(props: { label: string; value: number; onChange: (value: number) => void }) {
  return (
    <label class="flex flex-col gap-1 text-xxs text-muted-foreground">{props.label}
      <input
        type="number"
        min="1"
        class="h-7 rounded border border-border bg-background px-2 text-xs text-foreground"
        value={props.value}
        onInput={(event) => {
          const value = event.currentTarget.valueAsNumber;
          if (Number.isFinite(value) && value > 0) props.onChange(value);
        }}
      />
    </label>
  );
}
