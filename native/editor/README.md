# AD3 Editing — Rust migration

This is the native editor implementation under construction. The objective is
to replace the complete Electron/TypeScript application with Rust while retaining
the agent/human project workflow. **The conversion is not complete.** The existing
desktop release is still the production application. This crate neither wraps
Electron nor calls the TypeScript editor to perform its implemented operations.

## Run

Requires Rust 1.98 or newer, a native C/C++ linker and FFmpeg/FFprobe on PATH.
`AD3_FFMPEG_PATH` and `AD3_FFPROBE_PATH` override the codec executable paths.
FFmpeg must support file-loaded filter options (`-/filter_complex`).

```sh
cargo run --locked --manifest-path native/editor/Cargo.toml --bin dapi-rust -- --help
cargo run --locked --manifest-path native/editor/Cargo.toml --features desktop --bin ad3-editing -- /path/to/project
```

Twelve runnable projects are included: `examples/static` exercises source editing;
`examples/modules` exercises imported components/JSON; `examples/reactive` animates
rectangles and text through the native ticker, signals, `For` and `Show`;
`examples/inspect` exposes editable controls from an imported component module;
`examples/motion` combines slide/fade/text presets with an animated color track;
`examples/vectors` exercises native rounded shapes, transforms and group opacity;
`examples/media` combines native image/video composition with generated test assets;
`examples/text` exercises wrapped text, variable font weights, italic ranges and rotation;
`examples/paints` exercises animated gradients, glyph outlines and screen blending;
`examples/masks` exercises rounded/intersecting masks and a text reveal;
`examples/effects` combines fractional shadows, animated hue rotation and masked blur;
`examples/control-flow` exercises `Switch`/`Match` priority, fallback and live values.
Pass the full path to a folder to the native executable.

The desktop application opens an existing project folder or source entry, shows
its scene and timeline, selects clips, edits properties, moves/cuts/deletes clips,
saves to JSX and provides undo/redo. Preview composition runs on a separate worker
with one request in flight; seeking coalesces and obsolete revisions are discarded.
The first preview implementation is CPU based and **silent**. Real-time mixed audio,
persistent native decoders and full interactive parity remain work to do.

The project source remains the document. `package.json` `main` takes precedence,
followed by `index.tsx`, `index.ts`, `index.jsx`, `index.js`. No alternate project
JSON is introduced. `assets.yml` resolves linked sources by library path or id.
Rendered media never overwrites an existing output or original footage.

## Implemented native path

- `source.rs`: Oxc TSX parsing, constant properties and local constants, source
  byte spans, missing-ID stamping on edits, split/trim/ripple/move/remove/set, active
  scene switching, undo/redo, OS writer lock, optimistic source comparison and
  atomic source replacement. Unrelated source bytes stay unchanged.
- `document.rs`: authored node tree including every existing composition tag;
  original property values and source addresses.
- `evaluator.rs`: persistent Rust-hosted ECMAScript modules using Boa and Oxc.
  Function components, expressions, arrays, fragments, relative TS/JS imports,
  JSON modules, reexports, top-level await and installed ESM packages with exact
  `exports` entries can materialize a composition. No package is installed by the
  application and no Node/Electron process participates.
- `jsx.rs`, `props.rs`, `materialize.rs`: native JSX lowering with lazy props,
  reactive spreads/children and original file/attribute byte spans. Materializing
  a frame preserves mounted components and produces an independent Rust tree.
- `reactive.rs`, `control.rs`: Rust signals, memos, effect dependency tracking,
  batching, cleanup scopes, `createRoot`, `onMount`, explicit dependency `on`
  (single/array sources, previous inputs/results and deferred first call), `untrack`, `createContext`/
  `useContext` and scoped Providers, `mergeProps`,
  `splitProps`, `children`, keyed `For`, positional `Index`, `Show`, `Switch`/`Match`, and `useTicker`.
  Offline holds are bounded to 30 seconds and can be cancelled during mounting.
- `inspect.rs`, `source_inspect.rs`: binding-aware `@inspect` compilation, live
  signals, typed control metadata and literal initializer edits in the original
  imported module. The desktop previews changes and offers Save/Restore; saving
  checks every opened module for external changes and atomically replaces one
  file under the project writer lock. Undo/redo retains the whole source snapshot.
- `project.rs`, `encoder.rs`, `audio.rs`: shared preview/capture lifecycle,
  fresh offline mounts using the opened module snapshot, frame-by-frame reactive
  export, bounded encoder pipe, progress, and a 48 kHz stereo mixer in Rust.
  FFmpeg decodes and time-stretches sources once per path/rate; Rust applies clip
  windows, source clocks, reactive gain and summation at sample boundaries.
- `timing.rs`: 30 fps authoring units, output-rate frame quantization, source
  trims, both out-edge caps, natural source durations, nested clocks, container
  spans/clipping, workareas and dB buses. The original fallback is 480 frames.
- `transport.rs`: seeks, J/K/L shuttle tiers, looping and decoder generations.
- `geometry.rs`: native affine matrices, resolved local transforms, world/group
  bounds, camera resolution, inherited culling and point/quad selection kernels.
  Original-function reference fixtures cover rotation, simultaneous shear, flips,
  default/explicit anchors, singular matrices, group exclusions and selection
  boundaries. The compositor now uses these kernels for scene traversal.
- `compositor.rs`: native CPU rasterization with tiny-skia for scenes made
  of rectangles, text, images, videos, groups, sequences and audio nodes. Shared preview/capture/export
  draw lists apply position/offset, rotation, uniform/axis scale, inherited opacity,
  rounded corners, scene clipping and timed rectangle masks. Mask stacks cut the
  holder's fills/strokes/media/text and descendants; multiple masks intersect.
  Group bounds follow the original parent-first
  traversal using previous child transforms. Offline frames/PNG preserve alpha;
  H.264 frames explicitly composite over black. Realtime scene previews use the
  stage background. Media fit and clipping use the same native transformed geometry.
  Native composition does not invoke FFmpeg filters for stacking or drawing.
- `font_catalog.rs`, `fonts.rs`, `text.rs`, `text_layout.rs`: bundled Inter normal/italic variable
  fonts, system font discovery, HarfRust shaping, Skrifa outlines and native glyph
  composition. ASCII-space wrapping, explicit newlines, font weight/size/style,
  letter spacing, casing, UTF-16 style ranges, overlapping colors and all four
  vertical baselines follow the existing text path. Text sizes update after
  transform traversal, preserving the next-frame pivot/group behavior. Rounded
  authored wrap widths stay separate from animated computed dimensions.
- `paints.rs`: solid/linear/radial paint stacks, gradient stops, solid strokes and
  all 16 existing blend modes. Styles share their holder's geometry and do not
  enter the timeline or group bounds. Text ranges replace the corresponding
  fill/stroke list; gradients span the whole text box across words. Shapes draw
  strokes after fills; text completes its stroke pass before drawing fills.
- `effects.rs`: ordered sRGB blur/brightness/contrast/grayscale/hue rotation/
  invert/saturate/sepia filters, inherited per drawing operation, plus shape and
  text shadows. Native temporary rasters retain off-canvas source pixels needed
  by blur and shadows; masks apply to their final output.
- `texture.rs`: Rust PNG/JPEG/WebP/GIF/BMP/TIFF decoding, EXIF orientation,
  premultiplied alpha and a shared 128 MiB/256-entry decoded texture LRU.
  FFmpeg supplies source video RGBA, including display rotation and normalization
  of non-square pixels. Composition/source frame quantization, positive timestamps,
  last-frame clamping, contain/cover/fill and reverse seeks feed the same draw list.
  Cached frames are immutable and changed source files invalidate their entries.
- `media.rs`: native image probing, local FFprobe metadata, linked asset resolution,
  video frames and waveform. Matroska content duration excludes positive start offsets.
- `render.rs`: shared frame/output interface and the existing FFmpeg composition
  planner retained for compatibility tests and unsupported-node backend selection: rectangles, basic unwrapped text,
  video/image fitting, stacking, opacity, trimmed/rate-adjusted audio, mixing,
  workarea MP4 export and PNG/RGBA capture.
- `color.rs`: original packed-RGB parsing, its 43 named colors, RGB/HSL syntax,
  numeric colors, hex normalization and channel interpolation with JavaScript
  integer wrapping. Rendering uses this parser; embedded alpha is discarded as
  in the original reconciler, while opacity remains independent.
- `easing.rs`, `text_motion.rs`: native Bezier/steps/spring curves and deterministic
  character/word reveals and scrambling, checked against original-source
  fixtures.
- `motion.rs`: per-frame sampling of all 14 preset formulas and numeric/color
  keyframes, source-local clocks, delays, input/output windows, child ordering,
  inherited style clocks and uniform-scale rules. Sampling derives a new tree
  from authored values each frame, so reverse seeks do not accumulate changes.
  Preview/export integrate leaf/group fade/slide, text reveals/scramble and supported
  position/color/opacity/gain tracks. Native vector/media/text scenes also render grow/shrink,
  spin/twist and numeric transform/size/corner tracks. Blur presets and style targets
  still require their pending graphics implementations; unsupported render
  properties fail explicitly. Group opacity inherits per draw, as in the original
  Canvas renderer, including retention of inherited alpha when a computed
  assignment is outside [0,1], following the
  [Canvas contract](https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-globalalpha).
  Authored primitive sizes round before motion; vector leaf size keyframes may
  produce fractional dimensions. The native canvas is allocated before motion,
  with dimensions rounded independently to even pixels as in the original encoder.
- `main.rs`: JSON CLI for inspection, context, structural checks, source edits,
  media inspection, capture and export; no running Electron application required.
- `desktop.rs`: native egui canvas preview, timeline, inspector, source editing,
  cancellable export and export process cleanup when the window closes.

The static reader preserves source spans for editing. When a project needs
module execution, the native evaluator opens it for inspection/rendering; source
general JSX instance edits are refused until multi-file source mapping and
instance unrolling exist. Annotated `@inspect` variables can already be changed
in the inspector or through `controls`/`control` CLI commands.
The `solid-js` exports listed above are implemented in Rust. Other Solid APIs
(including stores and resources) are still pending.
Context Providers attach their values to their own reactive owner scopes.
Nested overrides stay local to their descendants; an undefined value uses the
context default, while null/false/zero remain explicit values. As in the existing
Solid host, Provider values are read untracked during mounting: pass a signal
accessor or an object with reactive getters when consumers need live updates.
Drawing callbacks, browser/HTML APIs, timers/fetch, asset-generation hooks,
CommonJS, package export wildcards/arrays and browser remapping remain unsupported.
Functions in composition properties,
duplicate IDs and cyclic data fail explicitly. Rendering also rejects properties
and node kinds that are not implemented. A successful parse/evaluation is not a
claim of rendering parity.

The materialized tree contains JSON data: optional object fields whose value is
`undefined` are omitted, and undefined array entries/children become `null`
(children are ignored). This normalization never rewrites the authored source.

The native CLI has explicit project arguments during migration; its command-line
and IPC parity with the installed `dapi` remains pending.

Native ripple edits use `ripple <project> <scene> <left|right> <time> [ids...]`.
Q/E and the desktop toolbar invoke the same source operation at the playhead.
With no clip selected, visible clips on every scene row are trimmed; sequences
are opened to their visible children. Left keeps the source frame under the
playhead and returns to the latest previous cut; right keeps the head. Later
clips move only within the affected sequence, preserving intervening gaps.
These edits use the existing source transaction and undo/redo path. General
dynamic JSX instance edits still require the pending multi-file source mapping.

## Full conversion acceptance ledger

Desktop timeline keyboard actions include Ctrl+A (select visible direct stage
children, including scenes, matching the original editor),
A (select the remaining reel from the earliest selected editing unit, or the
playhead when nothing is selected; nested sequences open, groups remain units),
Enter (select visible children one level down, looking through sequences;
hidden and inactive clips are skipped, and leaves keep their selection),
Backslash (select containing groups/scenes, skipping sequences and stopping
at the stage; shared parents are selected once),
D (advance one frame at the native 30 fps timebase) and S (seek back one second,
clamped to zero),
Delete/Backspace (close the selected gap or delete the clip selection), Ctrl+Z (undo),
Ctrl+Shift+Z/Ctrl+Y (redo). Text-field focus protects project actions.
Ctrl+Shift+H hides every selected element when any is visible, or reveals
the selection when all are hidden, in one undoable source transaction.
Ctrl+B cuts at the playhead through the same transaction as W and the split
button. Escape clears selection and gap/marquee gestures and closes the marker
palette, cancelling a pending marker hold or drag.
Ctrl+D duplicates selected subtrees in one undoable transaction and selects
the copies. Selected descendants of selected containers are copied once;
copies receive fresh IDs and remapped internal `syncTo` links, retain external
links, and omit `active`/`selected`. Copies of sequence children land immediately
after their containing sequence, matching the original editor.
The source-only CLI `duplicate <project> <ids...>` saves the same operation
and returns the new top-level IDs in `copies`, without probing media. Empty
selections and unknown IDs are rejected before writing the project.
Ctrl+G groups the selection in one source transaction and selects the new
group. Only subtree roots sharing the first selected root's parent enter it;
selected masks are excluded and names follow the next `Group N` counter across
the authored document. A mask-only selection leaves the source untouched.
members retain file order and authored props, and the group replaces their
first sibling slot without adding a transform. Composition undo restores the
original source.
The CLI `group <project> <ids...>` returns the new group's ID in `group`;
empty selections and unknown IDs fail before writing. Native pixel tests
compare grouped and original rotated/timed elements across five frames.
`Compositor::edit_geometry` exposes resolved transform values, frame bounds,
and local/world matrices from the last successful frame for forthcoming
transform editing and container dissolution. Failed frame preparation keeps
the previous snapshot; successful frames replace it entirely and reset clears
it. Container dissolution and its keyframe writeback remain pending.
`TransformValues::bake_container` ports the dissolve calculation: compose the
container and child matrices with the child's pivot folded in, decompose,
restore offset and flip, and round position/rotation/scale like the original.
It retains calculated skew for runtime use; integration with source edits,
keyframe updates and the ungroup command remains pending.
`SourceDocument::bake_container_transform` now writes the calculated
position/rotation/scale changes and synchronizes existing keyframe tracks.
It chooses uniform or per-axis scale spelling, removes the replaced scale
props, and returns the baked values including runtime skew. Identity bakes
preserve source; container dissolution and runtime skew application remain
pending.
`SourceDocument::dissolve_container` now releases group/sequence children
into the container's sibling slot, bakes spatial transforms through sequence
leaves, and converts start/source bounds/rate to the new parent's clock.
It returns released IDs and baked runtime values. A native pixel test covers
a translated group with offset/sourceIn/rate across multiple frames. Desktop
integration, scene dissolution and runtime skew application remain pending.
The property inspector now synchronizes an existing keyframe track when
applying a value. `SourceDocument::set_at_playhead` resolves the nearest timed
ancestor's source clock, including nested offsets/rates, and updates a point
on that 30 fps frame while retaining its easing, or inserts a fresh point.
Untracked properties remain ordinary edits; no animation track is created.
The inspector's Adicionar/remover keyframe button uses the selected property
and JSON value for the current source frame, creating a track when needed.
Toggling an existing point removes it; removing the last point removes its
track. Batch selection changes share one composition undo transaction, and
insertion preserves existing text and expands self-closing targets.
Remover animação da propriedade removes the selected property's entire
keyframe track, preserving the authored property and other tracks. Removing
an absent track leaves the source untouched; batch removal shares one undo.
The inspector lists the selected property's keyframes in scene-time order.
Clicking a point seeks the playhead and loads its JSON value for editing.
Source-clock conversion includes nested playback rates and source offsets;
the listing itself does not modify the document.
Each listed point has an easing preset selector and a descriptor field with
Aplicar curva, supporting the native cubicBezier/steps/spring descriptors.
Applying linear removes the explicit easing override. Draft fields reset on
snapshot replacement, including undo and project reload; each applied curve
uses composition history.
Each point also has an editable scene-time field and Mover ponto button.
Moving converts through the enclosing node's source clock, snaps to its 30 fps
source frame, clamps negative source times to zero, and preserves value/easing.
Editing the draft time does not write until Mover ponto is clicked.
The native timeline renders keyframe tracks as property rows, including
tracks nested in paints/strokes and other non-timed clip descendants,
with diamond handles. Clicking a point seeks its scene time, selects its
owning clip and loads the property/value in the inspector. Keyframe hit areas
do not start a clip move or empty-space marquee; click navigation does not
write source. Dragging a diamond previews its new position, snaps/clamps in
the owner's source clock, and writes one undoable time edit on release.
Escape cancels a pending point drag without writing. Nested track rows show
their target ID; clicking selects that target in the inspector and uses its
enclosing clip's clock. Timed descendants retain their own rows.
Click-selected keyframes are highlighted; Delete/Backspace removes that
point while retaining its owning clip. Removing the last point also removes
the empty track, and composition undo restores it. Escape, clip clicks and
keyboard selection navigation clear point selection.
Starting an empty-space marquee/click or a clip move also clears point
selection, so Delete after deselecting cannot remove an old keyframe.
Shift/Ctrl/Command clicking diamonds toggles multiple point selection.
Delete/Backspace removes all selected points in one undoable transaction,
including their tracks when the last points go. Ordinary clicks replace the
point selection; toggling its last member off leaves no deletion target.
Dragging any selected point moves the selected keyframes together by the
scene-time displacement, snapping/clamping through each owner's source clock.
The batch saves once on release and retains point selection; one undo restores
all times. Dragging an unselected point starts a new single-point selection.
Preview replies now carry the native renderer's edit geometry with their
generation and frame time. The desktop caches only current-generation geometry
and invalidates it when preview state changes; this supplies the same sampled
positions/group bounds as the displayed frame for forthcoming dissolution UI.
The native compositor and ProjectRenderer now accept finite per-node runtime
skew overrides. They affect local/world matrices and pixels without adding
unsupported JSX props, and the resulting edit geometry includes them.
Compositor reset clears overrides.
Desktop preview requests now include runtime skew, and successful composition
edits keep a matching undo/redo stack for it. Failed edits leave both source
and transient state intact; deleting nodes removes their overrides, undo
restores them, and opening a project clears transient state.
Desktop export now captures runtime skew at job start and passes it to
`ProjectRenderer::export_with_runtime`. Each native frame uses those overrides;
legacy-compositor export rejects them rather than silently dropping them.
A real FFmpeg regression decodes plain/skewed MOV exports and verifies changed
pixels with unchanged source.
Ctrl+Shift+G and Desagrupar seleção now dissolve selected groups/sequences/nested scenes,
select released children and commit source plus transient skew in one history
entry. The action pauses playback and waits for current-generation frame
geometry if necessary; a preview failure cancels it. A desktop regression
covers rotation under nonuniform scale, pending-frame behavior and undo/redo.
Nested scene dissolution bakes its sampled transform through child sequences
and preserves inherited timing/rates. Removing a scene also removes its own
clipping and fill, matching container dissolution in the original editor.
Root scenes cannot dissolve into the stage, which requires scene children.
Ctrl+Alt+Enter (or `dapi-rust sequence <project> <ids...>`) wraps selected sibling subtree roots in a new named sequence,
selects it, and resolves overlaps by start-time precedence in one transaction.
Earlier clips remain intact; later clips are edge-trimmed or removed when fully
covered, recursively settling group leaves. Undo restores the original source.
Ctrl+Alt+Shift+Enter dissolves only selected sequences, leaving other selected
containers intact. The pending preview request retains this restriction, and
the released clips keep their timing and share one undoable source transaction.
Ctrl+Enter (or `dapi-rust scene <project> <time> <ids...>`) wraps selected siblings in a new named scene, using their current
local geometry to round the bounding edges outward. Sequence children are
measured through the sequence; positioned members shift by the new scene's
corner with existing keyframe tracks synchronized. The action waits for a
current preview frame, preserves clip clocks, selects the scene and supports
one-step undo/redo. No scene fill is introduced.
The inspector also offers Criar cena com a seleção and Retirar sequências da
seleção. Nested wrapping inside a shifted/rate-scaled sequence is covered by
clock, clip-window and pixel regressions.
`SourceDocument::copy_selection` retains a frozen authored subtree snapshot;
`paste_selection` assigns fresh IDs, remaps internal `syncTo` links across copied
roots, removes active/selected flags and supports an insertion anchor or empty
self-closing destination. Pasting into the source sequence redirects to its
parent after that sequence. The snapshot survives source deletion.
Desktop Ctrl+C/X/V and native egui Copy/Cut/Paste events use this clipboard
when text inputs do not own keyboard focus. Paste targets the selected container,
the position after a selected leaf, or the active scene when nothing is selected.
Copies become selected; cut and paste each have one undoable source transaction.
The authored clipboard is local to the editor session and survives undo.
The inspector exposes Copiar/Recortar/Colar. Reloading the same project retains
the snapshot; opening a different project clears it, preventing relative media
paths from being interpreted against another project directory.
`audio_sync::align_envelopes` ports the original 500 Hz envelope correlation:
radix-2 FFT, overlap-normalized Pearson scores, one-second minimum overlap,
0.1 confidence floor and parabolic peak refinement. It validates samples and
bounds FFT memory before allocation. Tests cover offset signs, gain differences,
silence and insufficient overlap. `extract_envelope` decodes the primary audio
stream locally through FFmpeg into bounded PCM, preserving separate channel
energy, and reduces it to 500 Hz RMS bins. Cancellation covers probing, decoding
and envelope reduction. `dapi-rust media sync <audio> <reference>` returns the
measured offset and confidence. A real phase-opposed stereo WAV check verifies
a known 0.4-second recording offset despite gain differences. Timeline `syncTo`
placement is resolved by the linked timeline API described below.
`audio_sync::place_recording` implements the placement calculation independently:
fractional recording origin, source-trim precedence, inferred reference-window
intersection and natural-duration cap. It retains recording identity, parent,
gain and mute state, and rejects closed windows. Integration of this calculation
with linked timeline resolution uses `Timeline::resolve_with_sync_offsets`:
it resolves dependencies before followers, handles forward references, retains
fractional origins and refits containing groups/scenes. Cycles, absent references,
non-recording targets and missing measurements fail explicitly. The standard
desktop snapshot, renderer and CLI loaders measure `syncTo` recordings locally.
Renderer measurements are cached by both source-file versions and reused across
frames; changed files invalidate the measurement and failed measurements are not
cached. Reactive/motion frame resolution reuses the offsets while recomputing
placement against the current reference window. A real WAV project check covers
context resolution, PNG capture and completed MP4 export without source mutation.
Cancellation also reaches the FFT bit reversal, butterfly passes, spectral
multiplication and lag search, so alignment can stop during CPU work. A cancelled
alignment does not cache a measurement and a later retry remains usable.
Linked clip bounds snap to output frames while their audio origin keeps its
subframe precision. Natural source duration caps even an explicit sourceOut,
matching the original runtime's final time-range computation. Regressions cover
24/30/60 fps and an authored out point beyond the recording's end.
Source split/trim regressions also cover linked recordings: split halves retain
their common measured origin and reference, and an edited half keeps its new
window after linked timeline resolution without shifting its source clock.
Manual movement freezes the linked clip's current frame placement and source
window before removing syncTo, so reload does not undo the drag. Moving a group
also freezes its linked descendants, while a zero-distance move retains links.
These authored edits use the existing timeline frame precision; automatic sync
retains subframe origin precision until manually placed. Tests cover direct moves,
container moves at 2x rate, parent/child selection deduplication and reopening.
The mixer leaves silence between a rounded visual in point and fractional
recording origin when source zero lies later. It no longer fails with negative
preroll for this normal synchronized-audio case. A real WAV alignment at about
0.402 seconds (visual start 0.4) completes MOV export with 90 frames at 30 fps.
The permanent FFmpeg regression decodes the exported audio and checks silence
before recording onset and audible energy afterward, using opposing stereo
channels so envelope extraction cannot accidentally depend on mono downmixing.
Timeline Ctrl+=/Ctrl++ and Ctrl+- now zoom by the original 1.25 factor within
the existing slider limits, without editing project source. Text focus retains
priority over these document shortcuts.
Preview Ctrl+0 displays the sampled scene at its authored pixel dimensions,
rendered at its full resolution, with two-axis scrolling when it exceeds the
panel. Ctrl+1 restores the fitted preview and its 360-pixel-height render proxy.
Switching modes invalidates pending frames so an older render cannot replace
the newly requested resolution. Neither mode changes composition or export
resolution.
Arrow keys nudge positioned selection members in their own parent's space by
1 pixel, or 10 with Shift. The source operation samples motion at the playhead,
rounds the new position like JavaScript, and synchronizes existing tracks in
the same undoable transaction; duplicate selection IDs are processed once.
The CLI exposes the same edits through `nudge <project> <time> <dx> <dy>
<ids...>`, `keyframe <project> <id> <prop> <time> <json>` (toggle), and
`remove-track <project> <id> <prop>`. Nudge and keyframe use scene time and
resolve the target's source clock; track removal needs no media probing.
J/K/L shuttle and W/Q/E cut/ripple shortcuts require unmodified keys, matching
the original editor and avoiding edits from Ctrl/Shift/Alt combinations.
Navigation D/S, Enter and Backslash accepts Shift/Alt but excludes Ctrl/Command;
forward selection A accepts Shift and excludes Alt/Ctrl/Command.
Buttons and keys share the same source transactions; selected containers are deleted once.
Bracket shortcuts `]`/`[` bring the selection to front/back within each parent,
preserving the selected sibling order. Nested selections restack deepest-first
in one source transaction, and composition undo restores the original source.
CLI `restack <project> <front|back> <ids...>` uses the same source operation.
Native pixel regressions verify that restacking changes the overlapping color
and sending the element back restores the original composited result.
Inspector Ocultar seleção and Silenciar seleção controls write `hidden`/`muted`
to the selected nodes in one source transaction, preserve selection and use
composition undo/redo. Their checkbox state follows the primary selected node.

Drag from empty timeline space to select intersecting clips with a marquee.
Timeline markers load from `package.json` under `diffusion.markers`, keyed by
scene id. M toggles a marker on the playhead frame; Alt+M clears the active
scene's markers. Colored flags follow timeline zoom/scroll and do not enter
exports. Writes preserve other metadata and use native file replacement with
external-change checks.
Native configuration writers serialize with an OS lock before checking the
saved snapshot, so concurrent native saves reject the stale writer.
Native configuration also reads/updates `diffusion.clipColors` by full source
key (`file:locator`), validates six-digit hex colors and preserves unrelated
metadata. Timeline rows resolve authored IDs to project-relative source keys
and display saved colors; selection uses its highlight, and hidden/muted clips
show state labels. Inspector Cor dos clipes applies the shared eight-color
palette to the selection or restores defaults, persisting metadata without
editing TSX when IDs already exist. For unstamped static clips, IDs are saved
through composition history before color metadata is written, and colors remain
addressable after reopening. Dynamic instance writeback remains pending.
Headless egui rendering verifies the saved color reaches the widget.
`ProjectConfig::export_settings` reads per-scene authored format, template,
video/audio flags, codecs and numeric settings from `diffusion.export`, ignoring
invalid field types independently. Desktop export loads the scene's saved fps
and container into its editable fps/destination fields; ordinary composition
edits preserve manual overrides. Other encoder settings remain pending.
Explicit project open/reload rereads saved export preferences even when the
active scene and project path remain the same.
Salvar ajustes stores the chosen destination extension and fps for the active
scene, preserving its other encoder settings, markers and other scenes.
Exportar áudio controls whether the output includes sound, defaults to enabled
and loads/saves `audio.enabled` per scene. Disabling it skips native mixing or
mutes the export-only graph timeline; composition properties are unchanged.
FFprobe regressions verify video-only MP4, MOV and WebM from an audible source.
The MP4/MOV/WebM selector updates the extension while retaining the destination
directory and basename, and is disabled during an export.
Without saved preferences desktop export defaults to MP4/30 fps. Unsupported
saved containers retain their requested extension and fail explicitly.
Nonempty authored format names are preserved even when the native encoder does
not support them; desktop preflight rejects them before starting an export job.
A `.mov` destination
now produces a QuickTime container with H.264 video and AAC audio. Both native
frame export and workarea export retain the chosen container in temporary files;
FFprobe regressions verify QuickTime branding, dimensions, frame count and audio.
WebM destinations use VP9 video and Opus audio in both the native frame pipe and
FFmpeg graph export. Temporary containers retain the requested format, and
FFprobe regressions verify WebM codecs with and without audio. Workarea export
reads the active scene's saved `video.bitrate` in bits per second, using bitrate
rate control for H.264 and VP9 in both the native frame pipe and graph backend.
Absent overrides use the original editor's 10 Mbit/s target; invalid saved bitrates fail
before creating output. The saved `audio.bitrate` also controls AAC/Opus encoding
in workarea exports, with the original editor's 128 kbit/s default when absent.
Disabled audio ignores its bitrate setting. A real noisy-audio regression checks
that 32 and 192 kbit/s requests change the measured AAC stream bitrate, and that
Opus export succeeds without altering project metadata. Saved `video.codec` now
selects AVC/HEVC in MP4/MOV, VP8/VP9 in WebM, or AV1 in MP4/WebM,
including both workarea backends.
Missing codecs retain the container's native default. Incompatible/unknown codec
settings fail before output creation instead of silently selecting another
codec. A real VP8 export is checked with FFprobe and retains the saved settings.
HEVC uses libx265 with worker pools disabled, one frame thread and the `hvc1`
container tag. FFprobe verifies HEVC MP4/MOV, the tag, resized dimensions and
frame count while saved settings remain unchanged. AV1 uses libaom with two
threads and row multithreading; real MP4/WebM exports verify codec and dimensions.
AV1 in MOV is rejected before output creation because the installed FFmpeg MOV
muxer explicitly disallows it. Saved `audio.codec` now selects AAC in MP4/MOV
or MP3 in MP4/MOV or Opus in MP4/WebM. Output-rate validation follows the chosen codec, including
Opus's restricted input rates when exporting MP4. FFprobe verifies explicit AAC
and Opus selections, including mono Opus in MP4. Incompatible codec/container
settings fail before output creation; disabled audio ignores codec settings.
Additional audio codecs remain pending.
Saved FLAC audio is supported in MP4/Ogg with 32-bit integer encoding, mono/stereo,
and integer output rates from 8 to 192 kHz. FFmpeg's MP4 FLAC support requires
`-strict -2`; this is applied only for FLAC. The lossless codec determines its
bitrate independently of the saved lossy bitrate target. A real 96 kHz MP4
regression checks FLAC codec, 32-bit precision, sample rate and channel count.
Lossless exports ignore even invalid saved lossy bitrate values; real MP4 and
audible Ogg regressions verify this without rewriting project settings.
MP3 uses libmp3lame and validates its supported 8–48 kHz sample rates. Real MP4
and MOV workareas verify MP3 codec, output rate and channel count with FFprobe.
Saved `video.enabled: false` exports an audio-only MP4/MOV/WebM workarea.
The capture world still samples each frame for dynamic audio gain and timing,
but never constructs graphics frame plans or starts a video encoder. Disabled
video ignores codec, bitrate and resolution settings. Audio-only reports use
zero video dimensions/frame count, while progress counts composition samples.
Both tracks disabled fails before output creation. A real regression verifies
one audio stream, audible PCM and unchanged source/settings in all three formats.
Ogg is available for audio-only Opus/Vorbis/FLAC export. The desktop format picker enables
audio and disables video when selecting Ogg; manually re-enabling video is
rejected before starting a job. The export preserves the destination directory
and filename stem. A real Ogg regression verifies an audible Opus stream;
Saved Vorbis also works in WebM exports with video. Real Ogg/WebM audio-only
regressions verify audible Vorbis streams and unchanged metadata; the video
workarea regression verifies Vorbis codec, sample rate and mono channel count.
Ogg video and other Ogg audio codecs remain pending.
The desktop export bar now includes Export video alongside Export audio. Video
selection loads/saves per scene and survives source edits. Unsaved toggles are
passed directly to the export worker without changing stored preferences. The
real desktop regression exports a silent audio-only MOV with video disabled and
an irrelevant invalid height, then verifies that both tracks disabled is rejected.
An audio-only live-capture regression trims [0.2,1.2], samples a ticker-driven
0 to -12 dB change at source time 0.7, and checks the decoded RMS ratio around
the resulting midpoint. Cancelling during composition sampling creates no output
and leaves no owned render jobs, with source and settings unchanged.
CLI `export <project> <output> [fps]` now reads the active root scene's saved FPS
and audio-enabled preference. An explicit FPS argument overrides the saved rate;
absent preferences retain 30 fps and enabled audio. Preference selection does not
resolve a 30 fps timeline first, so a saved 120 fps one-frame workarea remains
valid. The real CLI regression verifies one frame at 120 fps, two frames with a
240 fps argument, disabled audio, and rejection of an invalid saved rate.
HEVC MOV first encodes into a temporary MP4 and remuxes without re-encoding;
direct libx265-to-MOV encoding reproduced a heap corruption crash in the installed
Windows FFmpeg build. Native silent export and graph export use the same remux
path; exports with audio already remux the temporary video during audio muxing.
The installed libx265 also showed intermittent heap corruption with small raw
frames after the MOV workaround. Its assembly optimizations are disabled for
now; the full video-codec regression passed three consecutive runs with this
configuration. This favors stable encoding at the cost of HEVC throughput.
The default-bitrate regression inspects H.264's x264 SEI settings to verify
`rc=abr` and `bitrate=10000` with and without package.json, rather than assuming
the measured bitrate of a static clip equals its requested target. Low-level
single-frame/graph exports outside ProjectRenderer retain their quality defaults.
Workarea export also honors saved `audio.sampleRate`, defaulting to 48 kHz.
The mixer and graph timing retain their 48 kHz clock; FFmpeg resamples the output
to the selected encoder rate. AAC supports its standard 7350–96000 Hz rates;
Opus accepts 8, 12, 16, 24 or 48 kHz input, while its decoded stream clock remains
48 kHz. Unsupported/noninteger rates fail before output creation, and disabled
audio ignores the setting. FFprobe checks actual AAC output at 44.1 and 32 kHz.
Saved `audio.numberOfChannels` controls mono/stereo output in both workarea
backends, defaulting to two channels. The stereo mixer is downmixed by FFmpeg for
mono encoding; FFprobe checks mono AAC/Opus and stereo AAC. Surround composition
remains pending and channel counts above two fail explicitly before output is
created. Disabled audio ignores the channel setting.
Saved `video.resolution` selects the export height in pixels, retaining the scene
aspect ratio and the native compositor's even-dimension normalization. Every
sampled frame and the graph export use the selected size; the report reflects
the encoded dimensions. Missing resolution uses the original editor's 1080p
default, including projects without package.json. Pixel regressions request small
export heights explicitly. Invalid values
fail before creating output. FFprobe checks resized H.264 and VP9 workareas.
The desktop export bar exposes the height in pixels, defaults to 1080 and loads
it per scene. Save settings persists it alongside FPS/audio without replacing
other encoder metadata. Composition edits preserve an unsaved height; export
uses the current value through an explicit override without saving it. The real
desktop worker regression reloads height 16, exports an unsaved height 24 and
checks 48x24 output while the stored height remains 16.
The desktop codec picker offers Automatic plus the codecs supported by the
selected container. It loads/saves per scene, preserves unsaved choices across
source edits, and clears incompatible choices when changing format. Export uses
the current choice without saving it; incompatible choices fail before starting
the worker. A real desktop export chooses unsaved VP8 and verifies its output
while the saved MOV codec remains AVC. Typed ExportOverrides carries these
transient export choices to the shared renderer.
The desktop audio-codec picker offers Automatic and the supported codecs for
each container. It loads/saves per scene, preserves draft choices across source
edits and clears incompatible choices on format changes. Unsaved audio choices
override the saved codec only in the export's in-memory configuration, so bitrate
and sample-rate validation follow the actual chosen codec without rewriting
package.json. The real desktop test exports unsaved MP3 while saved MOV audio
remains AAC.
The desktop also selects mono or stereo output per scene. Saving writes
audio.numberOfChannels; an unsaved choice travels through ExportOverrides and
overrides the stored channel count without changing package.json. The real
desktop export test verifies a mono MP3 stream while saved audio remains stereo.
The native audio sample-rate picker filters its standard rates by the selected
codec, loads/saves audio.sampleRate per scene and exports unsaved rate choices
through ExportOverrides. The real desktop test probes a 44100 Hz mono MP3 export;
the Rust mixer keeps its internal 48000 Hz clock and FFmpeg resamples the output.
Changing the container or audio codec preserves compatible sample rates and
replaces an incompatible draft rate with 48000 Hz. For example, moving from
44100 Hz AAC to automatic WebM/Opus selects 48000 Hz immediately.
The native video bitrate control uses Mbps, defaults to 10 Mbps and saves
video.bitrate per scene. Unsaved choices travel through ExportOverrides without
rewriting project settings. The real desktop test inspects the H.264 encoder's
SEI options to verify a 2 Mbps target while the saved target remains 10 Mbps.
The native audio bitrate control uses kbps and defaults to 128 kbps. It saves
audio.bitrate per scene and passes unsaved choices through ExportOverrides.
The control is disabled for FLAC and FLAC exports ignore lossy bitrate overrides.
The real desktop test probes a 64 kbps MP3 stream while saved audio remains
128 kbps, and the preference test saves a separate 96 kbps choice.
The native Presets menu ports all nine choices from the web export templates:
HD, Full HD, 2K, 4K, YouTube Shorts/4K, Instagram Reels, TikTok and web HD.
Applying a preset replaces draft export settings with its codec, height and
bitrate plus default 30 fps, stereo/48 kHz/128 kbps audio and enabled tracks.
It preserves the destination directory/name, changes the extension and writes
project preferences only when Save is pressed. Presets are unavailable during
an active export. Tests cover replacing an audio-only draft, saving web settings,
preserving composition source and ignoring changes during an export.
The native export panel shows the active workarea duration and an approximate
size from enabled tracks' target bitrates (duration × bits/s ÷ 8). Unlike the
web heuristic, it does not multiply a fixed bitrate by resolution, FPS or codec
factors: these affect quality at that target, rather than changing the requested
bitrate. FLAC displays variable size because its compression depends on content.
Container overhead and encoder rate control can change the final file size.
Native exports show a progress bar, elapsed time and an approximate remaining
render time once frames are advancing. Reaching the final frame displays
Finalizing while encoding/muxing and publishing finish; cancellation displays
Cancelling until the worker reports its result. Progress events after completion
cannot overwrite the final status. The UI refreshes the timer during quiet jobs.
Save frame PNG captures the active scene at the timeline playhead using authored
canvas resolution, independently of the preview proxy and video export height.
It includes draft inspect controls and runtime skew, freezes the source snapshot
and writes asynchronously to the export destination with its extension changed
to .png (or frame.png for an empty destination). Existing files are preserved.
Capture and video export cannot run together; shutdown joins the capture worker.
The desktop test decodes the PNG to verify full canvas dimensions and the selected
instant's blue frame, then confirms capturing a red frame cannot replace it.
PNG capture now accepts cancellation throughout scene setup, native rasterization
and publication. The desktop offers Cancel capture and signals cancellation when
closing before joining the worker. FFmpeg graph capture writes to an owned job
file, observes cancellation with a bounded process wait and publishes atomically
only after success. A regression verifies cancelled capture creates no PNG and
that the same plan can be captured successfully after clearing cancellation.
Desktop export/capture workers convert Rust unwinding panics into a reported
failure and still send their completion event. The UI clears the active job,
joins its thread and allows another operation instead of staying busy forever.
Ordinary render errors/cancellation retain their original messages. An injected
worker-panic regression covers both jobs; process aborts remain unrecoverable.
The native toolbar selects among top-level scenes in editable TSX projects.
Activation saves the active flag as one undoable source transaction and refreshes
the timeline, workarea, preview and per-scene export preferences. Selection from
the previous scene is cleared. Scene activation is disabled while exporting or
capturing, and for module graphs awaiting generated-instance source mapping.
An interaction regression switches scenes, checks saved flags and preferences,
and verifies undo restores the exact source before redo switches back.
New scene creates Full HD, portrait, square or 4K scenes in editable projects.
Each starts with a five-second workarea and a black solidPaint, receives unique
scene/paint IDs, becomes active and is selected in one source-history transaction.
The source API also supports expanding a self-closing stage and validates canvas
dimensions/duration before writing. Tests check a rendered black frame, unique
IDs across two scenes, undo/redo and rejection of invalid dimensions/durations.
The desktop opens an empty stage as a project with no active timeline, so New
scene can create its first scene. Until a scene is active, PNG capture, export
and preference saving are disabled; the canvas prompts the user to create or
choose a scene. Returning to an empty stage through undo clears the previous
preview. A regression loads a self-closing stage, creates its first scene and
checks undo/redo restore the empty/active states and exact source.
Delete scene removes the active scene and its contents as one undoable source
transaction, retaining other scenes. The editor returns to having no active
scene until one is selected; deleting the last scene leaves an editable empty
stage. Tests cover selecting a remaining scene, restoring the exact source with
undo, and deleting/restoring the last scene. The command is disabled during
capture/export and for projects without editable source mapping.
Duplicate scene copies the active scene's authored subtree, remaps every ID and
internal syncTo link, activates/selects the copy and preserves descendant runtime
skew overrides. The source and skew copies share one undo/redo transaction.
Existing per-scene package metadata is not rewritten by source duplication; the
copy starts with default export preferences until its settings are saved.
Tests check unique descendant IDs, one active scene, preserved workarea/skew and
exact source restoration with undo before redo restores the copied skew.
The native toolbar edits workarea start/end seconds with Apply range. Applying
validates the interval and resolved frame boundaries, saves one undoable source
transaction, updates playback bounds and export duration/size, and clamps the
playhead into the new range. Clip positions and source clocks are preserved.
Tests cover playback clamping, undo/redo, unchanged clip timing and rejection of
negative, empty, non-finite and sub-frame intervals before source publication.
Start/end at cursor mark workarea endpoints from the playhead, each as one
undoable operation. Empty ranges are rejected. PNG capture at the exclusive
workarea end now samples the same last visible frame as the preview rather than
an empty frame past the scene. The PNG pixel regression exercises this boundary.
The preview worker also reports unwinding panics through its normal frame reply
and drops the mounted renderer after any failed request. The next request rebuilds
scene state on the same live worker/channel, avoiding reuse of a partially sampled
renderer. A regression submits an invalid runtime transform then verifies the
next request returns the expected red pixels and edit geometry.
Rename scene edits the active scene's display name in one undoable transaction,
preserving its durable ID, clip timing and per-ID project settings. The selector
shows scene names with IDs in its choices and falls back to IDs for unnamed
scenes. The toolbar wraps its controls on narrower windows. Tests cover Unicode
and quoted names, empty-name rejection and exact source/name restoration on undo.
Scene format offers horizontal Full HD, portrait, square and 4K canvas sizes.
Width and height change in one undoable transaction; scene ID, workarea and all
clip properties remain unchanged. Tests render the resized canvas, verify clip
properties/timing and undo both dimensions back to the exact original source.
Insert adds a rectangle, circle or text to the active scene at the playhead.
The circle uses a square rect with a half-size corner radius. Each visual gets
a unique ID, is selected and is saved in one undoable transaction. Self-closing
scenes expand to contain the element, while existing content is preserved.
Tests create all three visuals, verify start times, render the resulting scene
and undo each insertion back to the exact original source before redo.
Text/textRange selection exposes a multiline content editor with Apply text.
Content is written as an escaped JSX string expression, preserving newlines,
quotes, braces, ampersands and Unicode. Child ranges/paints remain in the source,
and self-closing text expands to contain the new text. Applying is one undoable
transaction; selecting/reloading another element resets the content draft.
Tests cover special characters, child range preservation, rendering and undo/redo.
The text inspector also switches between automatic sizing and a fixed box.
Fixed sizing authors both rounded dimensions from the current preview geometry;
missing or stale preview geometry is rejected rather than guessing glyph sizes.
Automatic sizing removes width/height and their direct keyframe tracks, preserving
other tracks, text ranges and styling. Each switch is one undoable transaction;
undo/redo restores the exact source, including removed size animation.
Typography exposes line spacing (`leading`) as a percentage from 0% to 1200%,
matching the original inspector. Applying 100% removes the authored override;
zero follows the original renderer's default-spacing fallback. Updates reject
nonfinite/out-of-range input and mixed selections atomically, preserve text
ranges, and restore exact source through undo/redo. The regression measures a
multiline text's increased height at 200% and its original size after reset.
The font-weight picker exposes the original nine CSS weights (100 through 900)
and writes numeric TSX values. Existing `normal` and `bold` aliases display as
400 and 700 without changing source until Apply. The numeric control also allows
intermediate CSS weights from 1 to 1000 for variable fonts. The native font
catalog filters the picker by the resolved family's static faces and variable
`wght` interval. CSS fallback lists use the first installed named family;
unresolved families retain the original all-weights fallback. Results and the
catalog are cached across inspector frames; Refresh fonts clears both. Discovery
uses the existing bounded font reader and cancellation. Tests cover bundled
variable Inter, isolated static faces, family fallback and cancellation.
Temporary hover previews remain open parity work.
The native CLI now exposes `dapi-rust fonts` without an open project or running
desktop. It accepts `-f/--family`, variadic `-w/--weight`, `-s/--style`, positive
`-l/--limit`, and `-n/--names-only`. Default output is one JSON object per family
with `family` and `variants` (`weight`, `style`, CSS `local()` source); names-only
emits one plain family name per line. Filtering is case-insensitive for family
names and applies to variants before the family limit. Unknown options, missing
values, invalid styles and invalid limits fail before font discovery. Discovery
uses Rust font metadata on each supported host and includes embedded Inter,
including variable weight stops, rather than invoking platform shell scripts.
The command's flag/output tests also exercise real bundled italic variants.
The native `contact_sheet` module now ports the original sheet planner: largest
frame area, then fewest blank cells, then wider rows, within 2576x1456 pixels
without upscaling. It balances sheet counts (13 frames become 7+6), places cells
with the original margins/gutters, and supplies compact frame-clock filenames
and time spans. Geometry was checked against execution of the original
TypeScript planner, including its 9- and 12-frame results that differ from the
reference documentation table. Native sheet composition now draws captures in
request order over opaque grey gutters, rescales to the planned cells and stamps
compact timecodes with white text and rounded black outlines. Tests cover alpha
composition, frame order, scaling, labels, cancellation and invalid layouts.
Canvas font/subpixel parity remains unverified. The native `grab` module now
resolves explicit/negative times and evenly sampled windows, applies the four
quality budgets, and decodes/composes one sheet at a time through a callback.
Tests include a real changing-color video, requested order, compact spans,
source preservation, cancellation and stopping after a consumer failure.
`dapi-rust media grab <video>` now writes contact sheets and emits JSON Lines
with compact timecode spans and absolute PNG paths. It accepts variadic `--time`
(including negatives), `--count`, count-window start/end, quality, per-sheet
limits, uncapped requests, output directories and explicit project lookup.
Existing PNGs now support overwrite through a complete candidate and atomic
replacement; failed replacements retain recovery files. Existing targets must
be regular files of at most 64 MiB, and an output resolving to the source video
is rejected. Real-video tests overwrite an earlier output and confirm PNG
decoding, source preservation and cancellation before replacement.
`--separate` (alias `-S`) now writes individual frames without gutters
or labels, defaults to small quality, and rejects per-sheet options. Repeated
timecodes share one file within a run while retaining their JSON output entries.
Real-video tests check output dimensions, pixels and duplicate references.
The native `frame_triage` state machine now ports visual-state selection:
normalized block-luma distance, encoder-noise deadzone, waiting for transitions
to settle, a four-second calmest-frame fallback and novelty-based output limits.
Tests cover static/repeated samples, local changes, transitions and perpetual
motion; key selection results were compared with execution of the original
TypeScript algorithm using injected fingerprints. Native pixel fingerprints now
use the original 32x32 luma grid and 8x8 bilinear taps per block, texel-center
coordinates, clamp-to-edge sampling and Rec.709 weights. Tests check constant
colors, local two-block changes, tiny images, bilinear edges and cancellation.
GPU external-texture/color conversion parity is unverified; this extraction
currently runs on the CPU. `media grab --auto` now scans at 2fps, applies this
selection and writes sheets or separate frames; count sets the maximum (default
30), with optional start/end windows. Real-video tests cover changed/static
states and output limits. The auto scan now uses one sequential FFmpeg process,
discards intermediate frames and delivers selected frame indices with bounded
pixel buffers. It resolves scan positions through the video
clock, skips repeated presentation timestamps and returns content times relative
to the first video frame. Real VFR tests cover irregular holds, waiting for a
distinct settled frame and windows that begin inside an already presented frame.
Interactive persistent decoding, broader container timestamp parity and GPU
performance remain work to do. The native `video_clock` module now reads actual
best-effort presentation timestamps through bounded streaming FFprobe output,
checks order/source changes and resolves the last presented frame at a requested
time. Unit tests cover repeated/negative/irregular timestamps and invalid or
oversized metadata; real CFR and VFR fixtures retain their decoded timestamps.
This clock is wired into auto selection and sequential frame decoding. Real VFR
tests verify selected indices/pixels and process shutdown on consumer failure.
The decoder now seeks before the first requested frame using a midpoint in the
preceding presentation interval, retaining global frame indices after the seek.
Negative or unusable intervals fall back to decoding from the beginning.
Tests compare sought H.264 B-frame pixels against a full sequential decode,
including a positive track offset. Grab batches, auto selection and filmstrips
now pass their existing clock into the decoder, which verifies its source
version before reuse. A test decodes a sought H.264 frame with FFprobe unavailable
and rejects a clock belonging to another source. FFprobe still scans the full
clock initially; caching across separate commands and long-clip performance
remain open. The codec timeout is 120 seconds and the clock limit is one million frames.
Realtime playback remains open.
The native `audio_stream` producer now decodes stereo 48kHz float PCM in blocks
of at most 1024 sample frames, with content-time seeks/windows and pitch-preserving
tempo changes through FFmpeg. It checks packet alignment, finite samples,
source versions, cancellation and consumer errors without buffering the whole
recording. Tests cover arbitrarily split reads and short tails; a real stereo
WAV validates resampling, exact window lengths and a 2x playback-rate window.
This PCM producer feeds the playback mixer; output device and synchronized
transport integration remain pending, with a 120-second codec timeout.
The native `audio_session` now owns one producer worker and one queued PCM block,
keeping the codec alive across pulls. Each pull verifies source version and
continuous sample position; drop/cancel disconnects backpressure before joining.
Real-WAV tests cover the exact window sample count, repeated end-of-stream,
early drop, cancellation and invalid rates. This session has the same codec
timeout and feeds the playback mixer; physical audio output remains pending.
The native `playback_audio` mixer now lazily starts sessions for up to 128 voices,
mixes consecutive stereo blocks by explicit sample-frame windows and preserves
silence before/after clips. Gain and mute can change between blocks; muted voices
continue their sample clocks. Decode/cancel failures close sessions and require
rebuilding the mixer. Real-WAV tests verify overlap beginning mid-block, mute /
unmute, gain changes and trailing silence. Normal-speed streaming bypasses the
tempo filter to avoid altering short-window samples at 1x. `from_timeline` plans
voices from a scene and playhead, preserving fractional source origins, playback
rates, inherited gain/mute and silence before source zero. Muted voices stay
scheduled for later unmuting. `sync_levels` applies evaluated timeline gain,
mute and hidden flags between blocks, validating every voice before mutation.
Timing/source edits require rebuilding the mixer. Connecting these APIs to the
desktop transport, per-block reactive evaluation and physical output remain pending.
The Windows-only `windows_audio::WindowsOutput` now opens the default WinMM
device for 48 kHz stereo PCM16 and submits bounded packets synchronously on a
worker. Buffers remain allocated until unprepare confirms driver release;
cancellation/timeouts reset playback, and unreleased driver buffers are retained
on terminal errors. The confirmed sample clock advances only after completion.
A real-device silence test covers three packets and cancellation. This initial
blocking sink is not yet wired to the desktop transport; continuous queued
output, device selection and latency compensation remain pending.
The Windows `playback_session` joins mixer and output on an owned worker,
publishes only confirmed played sample frames, reports asynchronous completion
and stops/joins on drop. A remaining-workarea frame count bounds playback,
including a final partial packet. A real-device silence test verifies a 2055-frame
tail, repeated completion polling and early stop. Desktop transport wiring and
evaluated level updates during a running session remain pending.
On Windows the desktop now starts this session for normal-speed playback and
advances the playhead from confirmed audio samples. Pausing, seeking and document
generation changes discard the old session; completion stops at the workarea end
or restarts from its beginning when looping. Device errors stop transport and
surface in status. Media probing occurs on the worker. Shuttle speeds retain
the wall clock without audio. This initial integration uses the snapshot timeline;
reactive per-block gain evaluation, continuous device queuing and GUI/audio sync
validation remain pending.
Windows output now keeps up to three prepared PCM packets queued in the device.
The playback worker fills available slots before waiting for the oldest packet,
then replenishes the queue; the playhead still counts completed packets only.
Queue capacity errors preserve the submitted clock, short tails drain fully,
and drop resets/unprepares queued buffers before closing. Real-device tests
verify queue bounds, submitted-versus-completed clocks, replenishment and tails.
This replaces the initial single-packet playback loop. Decoder stalls can still
underrun the queue; latency compensation and audiovisual GUI validation remain open.
Project playback now mounts a cancellable ProjectRenderer on the audio worker,
plans voices from its initial evaluated timeline and samples it at each submitted
block's scene time before synchronizing gain/mute. This applies authored motion
and reactive bus levels through the same evaluation path as preview/export.
Static-project real-device tests verify evaluated workarea playback. Source/window
changes during evaluation still require a mixer rebuild; live inspector overrides,
runtime skews and dynamic voice membership are not yet propagated to this worker.
Desktop playback now supplies initial inspector values before voice planning and
sends changed values through a latest-only mailbox. The worker applies them to
its live evaluator before sampling each subsequent PCM block; unchanged values
are not resent. Already queued audio retains its prior evaluated values (up to
three blocks). Visual compositor skews remain visual; audio uses timeline media
sync offsets. Dynamic source/window changes and inspector-to-audible automation
validation still remain open.
Each evaluated audio block now reconciles voice membership, source paths, clip
windows and playback rates. Unchanged voices retain their decoder and partially
consumed PCM packet; changed/removed voices cancel their previous sessions.
New voices seek to the current submission sample rather than restarting source
audio at their original in point. Replanning validates the full new plan before
mutation and leaves the transport sample clock unchanged. Tests cover decoder
packet retention, changed sources, removal and gain/mute updates; dynamic real
media/GUI validation remains pending.
The real stereo WAV fixture now validates dynamic audio replanning after sample
49152: adding a voice seeks past the source's silent first second, hiding the
original preserves the remaining voice's partial packet, and replacing its WAV
changes the output polarity at the current source position. Uneven block lengths
verify consecutive transport sample positions through all three operations.
GUI validation and audible inspector automation still remain pending.
Desktop pause/seek now requests audio cancellation without joining a running
worker on the UI thread. Retired sessions are polled until their thread finishes,
then joined and released; at most four retiring sessions may accumulate before
starting another is deferred. Repaints continue during cleanup. A barrier test
verifies cancellation returns while initialization is blocked and no device opens
after the cancelled initializer returns. Application destruction still joins
remaining owned workers.
The real-WAV fixture now also samples a scene volume keyframe track through
ProjectRenderer and reconciles the mixer before every PCM block. It compares
inherited dB values against the project's 30 fps motion clock and verifies each
output stereo sample against the expected linear gain, including the partial
tail. This verifies authored gain automation in PCM; inspector overrides and
end-to-end audiovisual GUI behavior still need direct validation.
AudioSession now renews its decoder after 90 seconds of wall time, before the
120-second codec deadline. It resumes from the delivered PCM frame count,
retains the original source window/rate, renumbers packets onto the continuous
session clock and verifies source identity before/after renewal. The real-WAV
test forces two renewals and verifies 9600 total frames and stereo amplitudes.
Wall-clock playback beyond two minutes and tempo-filter continuity at non-unit
rates still need direct validation; renewal may temporarily stall the device queue.
The headless desktop integration test now exercises the real Windows output:
playhead advancement matches confirmed audio frames, seek changes the session
generation/origin, pause retires and cleans workers without moving the playhead,
workarea completion stops at its end, and looping resets to its start. All these
operations preserve authored source. This covers desktop transport logic with a
real device; visible GUI/video synchronization and audible media remain unverified.
The playback mixer now accepts a forward transport speed and scales clip windows
around the original playhead while multiplying each source's playback rate.
Source offsets remain unchanged at that origin; replanning uses the same mapping.
Unit tests verify 2x transport with a 2x clip yields a 4x decode rate and half the
remaining output frames, including fractional source timing. The desktop shuttle
has not yet been connected to these speed-aware APIs.
Forward desktop shuttle now starts a speed-aware audio session. Remaining device
frames divide by transport speed, reactive evaluation advances in scene time,
and confirmed device frames multiply by speed when updating the playhead.
Reverse shuttle continues with silent wall-clock playback. The real-device
desktop transport test now verifies 2x shuttle starts the accelerated session
and advances the playhead at twice its confirmed PCM rate. Audible accelerated
media and tempo-filter renewal remain to be validated.
The real-WAV mixer test now validates 2x transport from source second 1.2:
unchanged replanning retains correct samples, the remaining 0.8 scene seconds
occupy exactly 19200 output frames, and subsequent output is silent. This exposed
an extra frame from floating-point ceil at an exact boundary; mixer and session
frame conversion now snap only tiny roundoff near an integer before ceiling.
General fractional source precision remains intact. Audible GUI playback and
accelerated decoder renewal still need validation.
The evaluated-project real-device session test now covers 1x, 2x and 5x:
a 0.1-second workarea completes at exactly 4800, 2400 and 960 device frames.
After the playback changes, the default desktop suite passes 103 unit and 295
integration tests (24 hardware/media tests excluded); targeted session tests are
also run explicitly with the device enabled.
Windows output now enumerates named Unicode devices, reports channel capability
and queries the actual stereo PCM16 48 kHz format rather than relying on legacy
format flags. `open_device(Some(id))` selects an enumerated output; `None` keeps
the Windows mapper default. A real-device test verifies explicit selection and
seven-frame playback, and rejects invalid indices. Indices are session-local;
desktop selection UI and stable endpoint persistence remain pending.
The canvas TextureStore now resolves video textures to actual presentation
timestamps and decodes by frame index using this shared decoder. A bounded LRU
retains up to eight source-versioned clocks within 16 MiB; its bitmap cache
reuses pixels when different requests land on the same held VFR frame. Tests
check held-frame identity/pixels and preserve existing alpha, SAR, rotation,
source-change and raster-limit coverage. Initial clock admission verifies actual
dimensions, and FFprobe excludes frame side data from timestamp CSV output.
On a video cache miss, TextureStore now reads ahead at most four consecutive
frames, reducing the batch for a 32 MiB pixel budget (one large frame remains
allowed within the decoder limit). Those frames enter the existing bounded LRU
only after decoding and source-version checks succeed. A real-video test disables
both codec executables after the first request and retrieves a later frame from
the prefetched cache; cancellation and reverse cache hits remain valid.
TextureStore now retains an LRU pool of at most four forward video sessions
across consecutive missing batches. Session admission budgets native pixel and
index buffers at 128 MiB; one larger supported session can remain alone. Codec
internal allocations and the separate bitmap/clock caches are outside this
estimate. A source/version change, backward request or large forward seek replaces that source's
session, while cancellation/drop closes the pool. Tests alternate two real
videos with both codec executables disabled after startup, then admit five
sources and verify the four-session limit. Requests still serialize session
reads. Small forward gaps of up to 32 frames now discard intervening frames
inside the same decoder session, avoiding restarts when preview sampling skips
source frames. Tests cover skipped-frame identity, backward/large-gap rejection,
continued reads and a skipped preview request with codec executables disabled.
Asynchronous playback scheduling and realtime audio remain open.
The native `video_session` module now owns a forward decoder worker and a
one-frame bounded queue. It keeps one codec process alive across reads, verifies
source versions/frame order and disconnects the queue before joining on drop
or cancellation. Real H.264 tests cover seeking into a session, subsequent
frames, end-of-stream, early drop and cancellation. The shared codec controller
still imposes a 120-second codec lifetime. TextureStore renews sessions at the
next missing-frame request once their age reaches 90 seconds, leaving headroom
for decoder shutdown/startup. A deterministic real-video test ages a session,
requests its next consecutive batch and verifies a new worker with correct
pixels; another source keeps its existing session. This is preventive renewal,
not recovery from arbitrary codec errors. Complete playback scheduling and
realtime audio remain pending.
Contact-sheet grabs now also resolve requested times through the presentation
clock and decode their unique frame indices in one process per sheet. Cells
retain request order and repeated requests reuse a decoded frame. A real VFR
test requests positions inside held frames in reversed/repeated order and
checks the resulting cell pixels. Separate-frame grabs now use the same clock
and sequential decoding in batches of at most 12 requests. Batch pixel storage
is capped at 64 MiB, excluding decoder buffers and caller-retained output.
They preserve requested order and reuse repeated frames within each batch;
real VFR tests verify held-frame pixels in reversed/repeated requests. Each
batch uses the shared decoder's timestamp seek when applicable; clock caching
and broader seek validation remain pending.
The sequential decoder is also checked against H.264 with verified B-frames
and irregular timestamps remuxed to a five-second track offset. Tests retain
presentation-order pixels and content-relative automatic picks in that case;
this is not proof of all codec/container timestamp behavior.
Frame interpolation,
extreme aspect ratios and broader container/time parity need further checks.

`dapi-rust media filmstrip <local-video>` (alias `film`) now renders a PNG grid
with native timestamp rulers. It supports `-s/--start`, `-e/--end`, `-x/--scale`
and `-o/--output`; omitted output uses a unique system-temp filename. JSON output
contains the absolute `path`. Sampling, grid sizes, scale clamping, cover fitting
and ruler spacing follow the original runtime; media windows use its clamping
behavior. Source times use the native CLI time syntax (frame suffixes at 30 fps).
Existing output is refused, partial writes are cleaned up, and cancellation
reaches probing, decoding, ruler drawing and output writing. Tests create a real
changing-color video, decode/fit it, draw rulers, write/reopen PNG and confirm
that a second attempt preserves the existing output. `--project <directory>`
resolves library paths and asset IDs through the existing `assets.yml`, sharing
the same local source resolver as scene loading. Only the requested media is
probed; unrelated broken references do not prevent a filmstrip. Tests cover
path/ID lookup and verify the video and manifest remain byte-for-byte unchanged.
Automatic open-project IPC context, full original time syntax and persistent
interactive decoder parity remain open. Filmstrip grids now use one sequential
FFmpeg process and actual presentation timestamps, painting each decoded frame
into all matching cells without retaining a frame cache. The stream applies
cover scaling/cropping before crossing the codec boundary. Tests compare its
cover pixels with the existing single-frame decoder. Long-video seeks and
broader VFR/container behavior remain pending.
Filmstrips decode directly to thumbnail-sized RGBA using the shared bounded
FFmpeg runner, avoiding full-resolution Rust buffers and decoded-frame cache
entries. Display metadata controls cover fitting; the codec scales/crops before
returning bytes. Source-version checks still run before and after decoding, and
the existing source/target pixel limits remain enforced. The spatial regression
checks that a square thumbnail crops contrasting side bands. Exact subpixel
resampling parity with the original Canvas path remains to be verified.
The real-video regression also remuxes a landscape MP4 with a 90-degree display
matrix, verifies portrait display dimensions and checks top/bottom colors in
the decoded portrait thumbnail. The fixture uses FFmpeg's `-display_rotation`
input option; the installed FFmpeg must support it for this ignored test.
Selecting a fill/color property exposes an RGB picker alongside its JSON draft.
`dapi-rust media waveform <audio-or-video>` now uses native Rust peak sampling,
waveform drawing, silence overlays and timestamp rulers. It shares the filmstrip
window/scale/output/project options and returns `path` plus `silences` in source
seconds. The earlier positional `<file> <output.png>` form remains supported.
PCM is consumed in bounded blocks while stderr is drained independently;
cancellation/timeouts stop and reap the codec. Channels and source sample rate
are preserved, including energy confined to one stereo channel. Peak values use
the original square-root amplitude mapping, and silence uses its threshold and
minimum duration. Real WAV tests verify stereo energy, a one-second silence,
window trimming, cancellation and unchanged source bytes. Broader container and
timestamp parity remains open; this does not supply real-time editor audio.
Named, numeric and CSS colors use the native color parser; picked values become
hex color strings. Apply retains the existing multi-selection, keyframe-at-playhead
and undo behavior. Opacity remains a separate property, matching the renderer.
The Insert menu accepts local absolute paths or paths relative to the project
for image, video and audio clips, inserts at the playhead and selects the result
in one undoable transaction. Visual media retains its natural display dimensions
and is centered inside the scene; audio uses a centered 500x150 inspector box
and keeps its probed source duration. Missing files are rejected before edits.
Files are referenced in place. A PNG regression checks relative paths containing
spaces/braces, rendered red pixels and exact source restoration on undo.
Imported clips receive numbered names derived from the filename without its
extension, matching the web editor. Numbering considers named elements across
the project, and batch imports advance it without changing stable clip IDs.
The inspector exposes a name field and Rename action for a single selected
element. Renames preserve clip identity and timing, support Unicode and JSX
special characters, and participate in undo/redo. Empty names and multi-selection
are rejected; source modules and active media jobs disable this control.
Image/video elements and their paint nodes expose Cover, Contain and Fill in
the inspector. A fit change applies to selected media in one undoable edit;
mixed selections containing non-media elements fail without partial writes.
The media inspector also accepts a replacement local path for one selected
image, video, audio or media paint. It writes only `src`, preserving identity,
name, placement, authored timing and animation; intrinsic duration follows the
new asset. Relative paths resolve against the project. Missing files leave the
source unchanged, and replacement participates in undo/redo.
Removing intrinsic media from a selected image/video converts it to a rect,
preserving its ID, authored geometry, non-media properties and children. The
resolved timing is materialized when start/end were inferred from media. Media
source, fit, trims, sync and audio-only properties are removed; files remain
untouched, and undo restores the original element exactly.
The desktop action materializes missing width/height from the probed display
dimensions before conversion, so media using its intrinsic size retains that
size as a rect. Existing dimensions and child paints remain unchanged.
The native inspector inserts linear/radial gradients and edits their overall
opacity and each color stop's color, opacity and offset at the playhead. Existing
tracks are updated through the same keyframe-aware source editing pipeline.
Adding a stop creates a unique ID at offset 0.5 with white color; individual
stops or the entire gradient can be removed without deleting the owning element.
Each action is one undoable source transaction. Drafts refresh on seeking and
remain stable while editing at a paused playhead; module-backed source and
active media jobs disable these controls.
The Masks section inserts a named rectangular mask as a child of the selected
visual holder, offset by 20 px on both axes. A current preview supplies the
computed holder size; authored dimensions are the fallback, and a holder
without a box receives a 500x500 mask. New names follow the project's highest
existing `Mask N` suffix. Insertion selects the mask for normal transform and
timing editing. Rows select, duplicate or remove masks, with source undo/redo.
Mask clipping follows the original time window semantics: hiding a mask does
not disable its clip, and its opacity/paints do not affect the clipping path.
Dropping local image, video or audio files inserts the entire batch in one
undoable transaction. In an empty project, the first file also creates the
active scene: the last visual asset supplies its display dimensions, matching
the web editor; an audio-only batch uses a 1920x1080 canvas. The workarea covers
the longest asset duration, with five seconds for assets without a duration.
Visual assets dropped into a new scene retain their natural display dimensions
and are centered individually, including negative offsets for oversized assets.
Undo restores the empty project, and an invalid batch leaves the source intact.
VP9 graph export accepts odd dimensions (verified at 33x17); H.264 graph export
still rejects them. Native canvas capture retains its existing even-size
normalization, so a 33x17 authored canvas exports at 34x18 through that backend.
Explicit graph resizing now uses the same six-decimal resolution and rounds both
output dimensions to even values. It retains authored dimensions across repeated
resize requests: 33x17 resized to height 17 becomes 34x18, then height 34 becomes
66x34. A real H.264 graph export verifies the normalized 34x18 output.
An FFmpeg desktop integration test saves/reloads MOV at 23.976 fps, runs the
actual export worker and verifies QuickTime branding, fractional frame rate and
12 frames for its half-second workarea, with the TSX source unchanged.
The timeline offers the original eight-color palette and a Marcar button;
the chosen color applies to new markers (pink by default).
Markers can be dragged on the ruler with 30 fps snapping and save on release;
clicking a flag seeks to its exact frame without editing marker data.
Escape cancels the drag until the pointer is released, without saving the move.
Right-click a flag and choose Remover marcador to remove just that point;
other points/scenes are preserved and stale marker lists are rejected.
External marker changes detected during a drag prevent a stale write.
Tap apostrophe to toggle a marker; hold it for two seconds to open the color
picker without toggling on release. Ctrl/Command or text-field focus cancels
the pending hold. Marker edits are separate from composition undo history.

Shift/Ctrl preserves the gesture's initial selection; dragging in either
direction works. Sequence gaps are excluded from the selection hit regions.
Escape cancels the marquee and clears the selection. Mouse-wheel/scrollbars
remain available for navigation.

Native sequence rows show empty intervals between the union of child spans.
Click an interval and use Delete or "Fechar intervalo" to close it. Its original
coordinates are revalidated before saving; stale gaps and gaps in another scene
are rejected. Later scene content, including overlays and detached audio, moves
left once per container. The CLI shares this transaction through
`close-gap <project> <scene> <sequence> <in> <out>`.

Native W/playhead cuts resolve the selected scene's visible editing units. A
selected sequence opens into its clips; no selection cuts all visible scene
rows. Selected groups carry their descendants once. Cuts exactly at an edge
leave source and history unchanged. The CLI shares this operation through
`cut <project> <scene> <time> [ids...]`; saves and undo/redo use one transaction.

The source of requirements is `reference/jsx/`, `reference/`, the existing desktop
implementation and the runtime/encoder/assets/HyperFrames packages. Nothing in
this ledger is waived because a smaller example renders successfully.

| Area | Current implementation | Required before replacement |
| --- | --- | --- |
| Source/project model | Static TSX edits; native ES modules, reactive props, signals/effects, For/Index/Show/Switch/Match and ticker; original file spans; live `@inspect` and imported initializer writeback | Remaining package/Solid/browser APIs, generation hooks, loop unrolling and general multi-file JSX edits; complete last-good-render behavior |
| Timeline | Source/timeline clocks, workareas, cuts/trims/moves, ripple Q/E, Shift/Ctrl multi-selection, joint drag and marquee selection, nested rates, transport | Range selection, clipboard/grouping, transitions, keyframe tracks, remaining timeline gestures/colors/toggles and established constraints |
| Graphics | Native transformed/rounded vectors, images, video, wrapped text/ranges, solid/gradient paint stacks, solid strokes, blend modes, timed rect masks, shape/text shadows and the eight existing filter effects | GPU compositor and interactive filter performance, complete font fallback/scripts/emoji and synthetic styles, remaining transforms/constraints, non-rect mask targets, media/HTML/surface/shader paints, remaining text styling and style hierarchies, HTML, surfaces, WGSL and adjustment layers |
| Audio | Offline retiming and Rust stereo mixing with per-frame reactive/preset/keyframe gain | Real-time output and seek/rate re-anchoring, solo/bus hierarchy, sample-accurate gain automation, syncTo and precise behavior across all cuts/rates |
| Animation/captions | Native preset/keyframe sampling integrated with preview/export, including vector/media/text transforms, blur and supported paint/shadow/filter properties | Remaining style targets, sequence targeting, transition behavior, track editing UI, transcript parsing and every caption preset |
| Media/assets | Rust raster image codecs/EXIF, FFprobe/video RGBA, linked YAML sources and bounded shared decoded texture cache | Hash/manifest lifecycle, import/relink/move/folders, SVG/remaining formats, color profiles/HDR, proxies and persistent decoders/cache tiers, VFR/container timestamp parity, frame directories, remote resolution/modifiers, transcripts |
| CLI/IPC | Native explicit-project commands | Existing dapi flags/output/contracts, open/context IPC, logs, fonts, screenshots, filmstrips/contact sheets and all applicable local commands |
| HyperFrames | Existing separate package untouched | Rust save/template/preview/render/cancel engine, project isolation, immutable outputs and linked timeline updates |
| Desktop | Basic native editor | All inspector/canvas interactions, shortcuts, native pickers/menus, project dashboard/create/rename/duplicate, watcher/reload, session persistence, accessible UI QA |
| Export | H.264/AAC MP4/MOV and VP9/Opus WebM, reactive frame sampling, progress/cancellation and fresh mounts; PNG capture | Remaining containers/codecs, alpha/sequences, complete animation/audio parity and error recovery, high resolution and broader fractional fps comparisons |
| Delivery | Local Cargo crate | Windows/macOS builds and packaging, bundled native tools/fonts/licenses, install/launch/upgrade checks and removal of Electron/Node/TypeScript application dependency |

Project format migration and authoring API compatibility must be addressed by
the native evaluator rather than silently flattening or rewriting dynamic source.
The existing JavaScript project language is user-authored content; implementing
its host/evaluator in Rust is compatible with a Rust application.

## Verification

The shadows/effects increment passed 50 unit tests, 112 ordinary integration
tests and all four real FFmpeg integration tests (166 total), Clippy with desktop
enabled and warnings denied, and the desktop build. Both new reference generators
passed `--check`. The actual native CLI captured `examples/effects` at 0 and
0.75 seconds and exported a 640x360, 12 fps, 12-frame H.264 video. Captures,
decoded first video frame and the real egui framebuffer were visually inspected;
CLI warnings and the desktop error log were empty. These are gates for the
implemented increment; the full conversion acceptance ledger remains open.

```sh
cargo fmt --manifest-path native/editor/Cargo.toml -- --check
cargo test --locked --manifest-path native/editor/Cargo.toml
cargo test --locked --manifest-path native/editor/Cargo.toml --features desktop
cargo test --locked --manifest-path native/editor/Cargo.toml --test integration rendering:: -- --ignored
cargo build --locked --manifest-path native/editor/Cargo.toml --features desktop
```

The desktop-feature integration run also exercises the real timeline widgets
with headless egui pointer/key events. It checks click/additive/marquee selection,
gap clicks and gap-to-marquee gestures, desktop cut/save/undo/redo, keyboard
selection/deletion/history and protection while a text field has focus.
Regions are obtained from the rendered widget layout rather than hardcoded
screen coordinates. These tests do not replace OS-window visual/accessibility QA.

The explicit FFmpeg integration tests create their own video/audio fixtures. They
check workarea duration, dimensions/fps, source motion, the overlaid color,
audible dB gain, reactive movement/gain, frozen imports, repeated exports,
complete-frame durations, capture, cancellation/reaping, temporary-file cleanup, missing
streams and refusal to overwrite. Source tests cover UTF-8 and
comments, split/trim at altered rate, inherited timing/gain, unsupported dynamic
modules, JavaScript text coercion, atomic save, validated undo/redo and external
edit conflicts. A debug framebuffer smoke test also opened the native editor and
captured the real egui window. These are gates for
the implemented subset; they do **not** prove complete editor parity.

The vector stage passed 79 ordinary tests and all three FFmpeg integration tests,
Clippy for every target with desktop enabled, and the desktop build. Fourteen
vector regressions cover transforms, parent-first group history, scene switching,
incompatible reactive nodes/properties, cancellation, per-draw alpha, rounded
clipping, odd/fractional dimensions, repeated resizing and exclusive PNG output.
A real CLI run captured `examples/vectors` with deliberately unavailable codec
executables, then exported and probed 72 H.264 frames at 960x540/24 fps (3 seconds).
Its PNG, decoded MP4 frame and actual egui window were visually inspected.

The following media stage passed 90 ordinary tests and all four FFmpeg integration
tests, Clippy for every target with desktop enabled, and the desktop build.
Media regressions cover fit, alpha, transforms, rounded clipping, negative destination
sizes, JPEG EXIF, shared ownership/eviction, file changes, cancellation, wide images,
unsupported image formats, source clocks and real export frames. Real fixtures also
cover non-square pixels combined with display rotation and a positive Matroska
timestamp offset. A source replacement after frame planning must invalidate even
a cached video frame; resampling reprobes it and recovers. A decoded frame larger
than its probed dimensions is rejected at the output pipe. Cancellation reaches
image probing and FFprobe as well as texture decoding. These CFR fixtures do not
establish VFR parity.
Missing average FPS is covered by a real one-frame Matroska fixture with its
optional DefaultDuration removed (`avg_frame_rate=0/0`). Probing measures packet
count/content duration when available, then uses a valid nominal rate if needed.
Metadata-dependent video clocks resolve during pixel rendering, and a regression
verifies recovery from an invalid clock without remounting the scene.
`tests/reference/generate-media-fit.mjs --check` compares 24 fit cases to the
actual original render function, including fractional, negative and zero boxes.
The real CLI captured `examples/media` at two times and exported 48 H.264 frames
at 960x540/24 fps (2 seconds). Both captures, a decoded MP4 frame and the actual
egui window were visually inspected; the UI error log was empty.

The native text stage passed 124 ordinary tests (40 unit and 84 integration),
all four real FFmpeg integration tests, Clippy for all targets with desktop
enabled, and the desktop build. Text regressions cover UTF-16 and overlapping
ranges, fractional indices, wrapping/baselines, authored versus animated sizes,
next-frame transforms, hidden text, temporary font failures and cancellation.
Font tests cover variable weights, pinned Inter priority, bounded discovery/read
budgets, collection limits, cache eviction/shared source bytes and a real Windows
junction escaping a discovery root.
The committed fixtures contain 192 cases executed against the original
tokenization/layout functions and 1536 Canvas font measurements using the same
bundled fonts in Electron 43.1.1 / Chromium 150.0.7871.114. Advances allow at most
0.125 px difference; font box metrics match exactly. The development-only
generators under `tests/reference/` record source/font hashes; ordinary native
tests consume those snapshots without a JavaScript runtime.
The real CLI captured `examples/text` at two times and exported 48 H.264 frames
at 960x540/24 fps (2 seconds), without warnings. Both PNG captures, the decoded
first MP4 frame and the actual egui window were visually inspected. The UI error
log was empty, and independent critical review approved this increment.

The paint/stroke/blend increment passed 140 ordinary tests (45 unit and 95
integration), all four real FFmpeg tests, Clippy with warnings denied for every
desktop target, and the desktop build. Its 288 original-function gradient cases
cover geometry and ordered stops; 63 Chromium Canvas cases check linear/radial
gradients and all blend modes. Pixel comparisons allow two levels in premultiplied
RGB and one alpha level, accounting for dithering/quantization near transparency.
Eleven integration regressions cover paint order, range replacement, inherited
blend/line/alpha state, aliases, animated scales, hidden stops, tiny strokes and text-box
gradient coordinates. Three further unit tests cover node/depth budgets,
pre-cancel history preservation and oversized raster-buffer rejection.
The real CLI captured `examples/paints` at two times and exported 48 H.264 frames
at 1024x576/24 fps (2 seconds), without warnings. PNGs, the decoded MP4 frame and
the real egui window were visually inspected; the UI error log was empty.
Independent critical review approved this increment after the complete checks.

The mask stage passed 47 unit tests, 104 ordinary integration tests and all four
real FFmpeg tests (155 total), formatting, Clippy with warnings denied across
all targets with desktop enabled, and the desktop build. Its original-runtime
oracle covers 56 cases and 3904 pixel samples, including 3768 uniform
neighborhoods, plus timing and matrix comparisons. Regressions cover hidden
masks, independently timed windows, reverse seeks, reactive mask toggles,
group bounds, sibling restoration, nested intersections, text/image/video
clipping, cancellation and clip working-memory/work limits.
`examples/masks` was captured at 0 and 1 seconds and exported to H.264 at
960x540, 24 fps, 48 frames (2 seconds), without warnings. Both captures, the
decoded first video frame and the real egui window were visually inspected;
the UI error log was empty.

`@inspect` coverage includes semantic binding/shadowing, escaped identifiers,
UTF-8 initializer spans, VM garbage collection/remounts, imported-module edits,
whole-graph conflict detection and undo/redo. Windows save tests cover creation
time/named streams, zero-byte originals, locked files, concurrent replacement,
late writes through displaced handles and exclusive backup disposal. A real CLI
smoke changed two imported initializers, checked all other source bytes and
captured the result; the native UI displayed those controls and the same frame.

Numeric fixtures compare 220 easing samples, 273 text-motion results, 339 color
parses, 51 color interpolations and 672 complete preset samples from the original
ECS function, including both phases, large overshoots and JS bitwise
wrapping. Rust tests read committed JSON and need no JavaScript runtime. The
development-only `tests/reference/generate-motion.mjs` regenerates these fixtures
from the original TS implementation and installed anime.js/colord; it requires
Node 24 and the repository's development dependencies. Adapted algorithms retain
their MIT notices under `licenses/`.

Geometry has 180 transform/camera/culling cases, 45 group cases and 36 quad
comparisons from the original math, transform and interaction functions, plus
explicit singular/anchor/edge tests. Its development-only generator requires
Node 24 and records hashes of the original sources with normalized line endings.
Regenerate with `node native/editor/tests/reference/generate-geometry.mjs` from
the repository root. Before accepting changes to those originals or this port,
run `node native/editor/tests/reference/generate-geometry.mjs --check`: it executes
the original functions again and compares both results and source hashes with
the committed fixture. The ordinary Rust tests consume that snapshot offline;
they do not themselves check for changes to the TypeScript originals.

For compatibility reviews, follow the whole source path: public JSX props in
`packages/reconciler/src/document.ts`, then their runtime consumer. The Rust
tree retains authored prop names (`x`, `scaleX`, stroke `width`); ECS paths such
as `position.x` and `stroke.width` are internal adapter outputs, not another
accepted authoring syntax. Existing fallbacks are intentional compatibility:
unknown preset types become fade, non-`out` phases become in, and unrecognized
easing descriptors use linear interpolation. Tests cover these source-defined
fallbacks; reviews should include the adapter before proposing stricter input
rules or additional aliases.

Integration modules share one test executable to avoid linking the VM multiple
times on Windows. On a memory-constrained build machine, use `CARGO_BUILD_JOBS=2`.
The development profile omits debug symbols for the editor package only, keeping
assertions and overflow checks enabled. To debug the editor with symbols, pass
`--config 'profile.dev.package.ad3-editor.debug=2'` to Cargo. Dependencies retain
their normal profile, avoiding a full cache rebuild when changing this setting.
Dependency symbols can still produce Windows PDB files; this setting does not
disable all symbol linking.
To omit PDB generation during local Windows UI iteration, use
`cargo rustc --locked --manifest-path native/editor/Cargo.toml --features desktop --bin ad3-editing -- -C link-arg=/DEBUG:NONE`.
This explicit linker override leaves cached dependency compilation intact; omit
it when a Windows debugging session needs the PDB.
For repeated Windows development/tests, setting `AD3_NATIVE_PDB=0` applies that
linker override to this package's debug targets through its Rust build script.
It does not change dependency compilation flags or disable runtime checks.
Leave it unset for normal PDB generation; release settings are unaffected.
Source saves preserve Windows metadata through
[ReplaceFileW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew),
retaining a recoverable backup when replacement cannot complete.
If an external editor changes the target between the preflight check and Windows
replacement, the displaced source is retained and the save reports a conflict
with its backup path; reload and reconcile the two versions before continuing.
Verification and backup deletion hold one exclusive Windows file handle, so an
outside writer with a still-open handle also causes retention. Recovery after a
failed replacement always retains the original backup, even after restoring the
target name. Conflict recovery is explicit; it never overwrites a newer target.
An exclusive-create failure never deletes the pre-existing candidate pathname.
Failed source candidates are retained when still present, with the path reported
for recovery. Source candidates and concurrent render jobs include a process-local
sequence so a coarse or repeated Windows timestamp cannot reuse another operation's
name. Render directories are reserved with atomic creation and collision retries.
Unix saves
preserve permission bits and refuse ownership changes; concurrent external
replacement guarantees, broader Unix ACL/xattr
and packaging verification remains pending. Native numeric controls currently
require finite values, as does the materialized composition data model.

Module evaluation tests cover typed function components, map/conditionals,
fragments, JSON, installed ESM packages, exact export conditions, reexports,
top-level await, edit refusal, optional undefined data normalization, unsupported
host callbacks and bounded loop/job execution. Reactive tests cover batching,
memo equality and pull order, conditional dependencies, nested cleanup, list
identity/reordering, Show lifetime, mount ordering, ticker boundaries, optional
children props, offline holds/cancellation, and independent samples.

The export clock follows the existing capture engine: authored seconds are
quantized to output frames when its fresh world is mounted, including workareas,
natural source durations and motion windows. Frame strings such as `45f` retain
the language's fixed 30 fps unit before conversion. The encoder counts complete
frames from those integer endpoint indices; this avoids cancellation when a short
workarea is far from scene time zero. Audio sample counts derive from that frame
count at 48 kHz. The report distinguishes the authored
workarea duration from the video duration; audio covers the workarea at 48 kHz.
`playing()` is true in exports. The ticker's frame accessor uses the output FPS
in exports and 30 fps in the editor/capture, matching the current engine behavior.
Native vector/media/text export rasterizes each frame in Rust and streams it to one encoder.
Uncached video frames still start a separate FFmpeg decode process. The older filter planner remains for compatibility and rejects unported node kinds;
native GPU rendering and persistent decoders remain pending. Decoded audio is held in
job-owned temporary files, with a limit of 128 distinct source/rate variants.

The compositor chooses a backend from all materialized node types on the
first frame, including hidden nodes, and retains it and geometry history per scene.
Plain text and font/color ranges use the same native backend as vectors and media.
A native scene with incompatible nodes, properties or planning data stays stopped
until the project is reopened; cancellation during pixel rendering and text/font
preparation remains recoverable. Text shaping starts only after visibility and
culling have been resolved; text preparation failures do not commit geometry
history. Culling is inherited from the scene, as in the original: text outside
the canvas can still be measured while its scene is visible. Both backends
explicitly reject unsupported properties; the older
renderer is not a generic fallback for styles. Reactive addition/removal of supported
text stays on the native compositor.
Nested scenes render natively: their transformed rounded rectangle clips descendants,
intersects inherited masks, and restores the ancestor clip for following siblings.
Their children retain the containing root scene clock, including trims and playback rates.
Rounded shapes support negative width/height by reflecting the local path after
radius normalization, preserving the logical corner mapping. Pixel regressions
cover all sign combinations with unequal corner radii. Rounded corners use a
fine cubic approximation;
Supported non-rect nodes can also serve as masks: their transformed rectangular
box supplies the clip, including corner radii for media boxes. Authored text
boxes clip without painting glyphs. Masks retain their own timing and remain
excluded from fitted container bounds and the normal draw traversal.
Fixed-box keepAspectRatio now resolves authored width/height with the pinned
ratio or the default box ratio when only one dimension is authored. This resize
runs before motion; independent size tracks can then override the result.
Text keepAspectRatio also preserves the measured box ratio when a previously
automatic text receives one authored bound; two positive authored bounds pin
their ratio. Its resize precedes animated sizes, and removing the lock leaves
the other dimension unchanged on a subsequent single-axis edit.
browser Canvas antialiasing is not assumed byte-identical. Preview currently
shows the normalized scene, rather than a complete stage/camera editing view.
Output resizing keeps the original image encoder's uniform resolution even
when rounding canvas dimensions leaves a partial edge pixel or crops a fraction
of one. Repeated resizes derive their ratio from the original scene dimensions.
Color/corner keyframes preserve the original trait membership: a color track
does not paint an unfilled rectangle, and mixed-corner fallbacks resolve before
an animated uniform radius.

Rectangle masks use their full world transform and half-open time window.
Masks retain antialiased edge coverage. The oracle uses the original default
Chromium canvas configuration; forcing software/read-frequent rendering changes
clip coverage and is not the reference mode.
Their Hidden flag, fill, opacity, blend and render descendants do not affect the
clip; hidden masks skip animation sampling. Nested masks inside a mask do not
add another clip. Masks are excluded from group bounds and fitted durations.
The current reconciler creates masks at **100x100**, and the timing runtime
defaults them to **480 frames**. These actual source defaults differ from the
500x500/whole-parent-span claims in `reference/jsx/rect.md`.
Unsupported style children still fail validation, including beneath a mask.

The development oracle `tests/reference/generate-masks.cjs` executes the original
rect/mask adapter cases, cache refiling, timing, transform, visibility and
`renderNode` functions with small ECS fixtures and Chromium Canvas. Its shape
body stub paints intrinsic fills/solid strokes; clipping uses the original
renderer. Source hashes accompany the 56 cases.
Their matrices and timing are checked numerically. Pixel comparisons require
matching interiors/exteriors within two premultiplied levels; only samples at
an original one-pixel boundary may use that neighborhood's color/alpha range.
The rounded-edge coverage of Canvas and tiny-skia is not byte-identical.
Regenerate/check with
`node node_modules/electron/cli.js native/editor/tests/reference/generate-masks.cjs --check`.
The Rust tests consume the committed fixture without invoking Electron.

Paint/stroke blending and opacity are inherited per draw. A `sourceOver` or
unknown blend value removes the original trait and inherits the parent's mode.
Invalid alpha assignments retain the current Canvas value; nonpositive stroke
width/miter assignments retain the preceding line state. Text solid paint/stroke
color and opacity read authored traits, as in the original text renderer;
gradient stops/geometry and stroke width read animated values. This preserves
the existing difference between text styles and shape styles. A hidden range
does not suppress its intrinsic text styling, while a hidden paint is skipped.
Gradient stops remain included even when their own `hidden` prop is true.

Filters use the original `buildEffects` order: the holder's animated `blur`,
then visible effect children. A nonempty local list replaces the inherited
Canvas filter; an empty/no-op list retains it. Each fill, stroke or word is
filtered separately, including beneath groups. This follows `renderNode`, whose
behavior differs from the isolated-group wording in the older JSX reference.
Brightness, contrast and saturation retain the original 0..1 clamp. Unknown
effect types map to blur, and the original epsilon skips neutral effects.
The color matrices follow the [CSS filter definitions](https://www.w3.org/TR/filter-effects-1/#FilterProperty).

Shadows draw their colored source silhouette as well as the cast shadow, just
like the original `ctx.fill()`/`fillText()` call. Shape shadows read animated
properties; text shadows read authored color, opacity, blur and offsets.
Text ranges replace shadow lists independently of fills/strokes. All text
shadows precede all strokes and fills. The widest visible stroke supplies a
stroked word's shadow silhouette, while its foreground stroke retains the
parent's stroke color because `renderTokens` only changes `fillStyle` there.
Negative shadow blur and invalid alpha retain the preceding Canvas assignment.

Rasterization follows the [Canvas drawing model](https://html.spec.whatwg.org/multipage/canvas.html#drawing-model):
transform, global alpha, filters, cast shadow, then clip and blend each output.
CSS blur lengths are device pixels; shadow blur/offsets additionally follow
capture resolution, as the original renderer explicitly scales them. Filtered
media applies its fitted image before clipping to the holder, preserving pixels
outside the box that contribute to blur. Intermediate bounds include this
source margin even at canvas edges. Small/medium blurs use separable Gaussian
convolution; sigma above 32 uses the specification's three-box approximation.
This CPU implementation is not an interactive-performance or GPU parity claim.

tiny-skia 0.12 changes pure pattern translations to nearest-neighbor sampling
even when bilinear quality is requested. Fractional shadow offsets are therefore
resampled explicitly before integer compositing; keep that regression when
changing the raster backend.

`tests/reference/generate-effects.cjs` executes the original filter builders,
shape/shadow/stroke renderer, transforms and `renderNode` in default Chromium
Canvas. Its 107 cases include all eight filters, clamping/defaults, alpha,
inheritance, filter order, masks, off-canvas input, shadows and capture scale.
The 57856 samples require agreement within two premultiplied byte levels without
blur and four with blur. Four transformed shape cases permit only the original
one-pixel neighborhood range plus two levels for differing edge antialiasing.
`generate-text-shadows.cjs` executes the original text styling consumers with a
Canvas-state recorder; eight command traces check exact draw order, inherited
stroke color/width, ranges and authored-vs-animated values. Both oracles record
source hashes and accept `--check`; only the development generator uses Node or
Electron. Rust tests consume the committed JSON directly.

Native planning limits each scene to 100000 nodes/128 levels and each frame to
65536 draws, eight million styled path points and one million gradient stop
applications. A holder accepts 1024 paints/strokes/shadows/effects; a gradient accepts 4096 stops.
Stroke width is limited to 8192 and miter ratio to one million. Planning checks
cancellation before committing new geometry history. Large raster buffers use
fallible reservation: at most 256 MiB for pixels and 64 MiB per clip mask.
Clip paths use a shared index arena capped at 4096 entries. Rasterization reuses
two full masks for the current intersection and scratch, regardless of depth,
plus one temporary mask while drawing media (at most 192 MiB for these masks).
Consecutive styles reuse the intersection; descendants extend it, while sibling
stacks are rebuilt. A frame caps clip raster work at 2^31 pixel applications.
Pixel intersections and stack planning check cancellation. These bounds
exclude decoded textures, fonts and the rasterizer's additional working storage;
they do not guarantee recovery from every allocation inside dependencies.

Effects additionally permit at most 64 active filters per holder, sigma 256
device pixels, 8192 pixels of accumulated blur support and 2^31 filter pixel/tap
applications per frame. Shadow inputs permit blur 512 and offsets of one million
document pixels, with the sigma bound checked again after output scaling.
Each padded effect raster is at most 16384 pixels per edge and 64 MiB; foreground,
cast shadow and scratch together need at most 192 MiB beyond the existing frame,
clip, font and media storage. Allocation is fallible, processing checks
cancellation between rows/chunks, and resource excesses fail explicitly.

Text uses LTR Canvas paragraph context, as the current runtime does, and Unicode
bidi runs within it. The bundled Inter font is pinned with its OFL license and
hashes in `assets/fonts/`; system font availability and fallback ordering remain
platform dependent. Synthetic bold/italic, sequence-aware ZWJ/variation-selector
font selection, color/bitmap/SVG glyph painting, and full script/locale fallback
parity remain pending and unsupported representations fail explicitly. Requested
weights outside 1..1000 and malformed/excessive font requests also fail instead
of emulating Canvas's retained invalid-font assignment. Unhinted outline ink
bounds and antialiasing can differ from Canvas, even when advances and font box
metrics agree. An empty styled text uses finite zero geometry instead of the
original non-finite empty-line result. Solid and gradient paints and solid strokes
also apply to text ranges; shadow/effect and media/HTML/surface/shader paint
children remain pending.
Font files are copied into owned bytes, limited to 64 MiB per file; the cache
holds at most 128 faces/256 MiB of source bytes, plus parser data. Shaped words
use a 2048-entry/64 MiB cache and fallback results a 4096-entry/4 MiB text-key
cache; up to 32 metadata candidate lists cache family/style matching. A frame limits text to 4 MiB, 100000 pieces, two million outline points
and 65536 uncached font probes; a word limits characters to 100000 and probes
to 8192. Discovery scans common system/user font folders with cancellation checks, at most
65536 directory entries, 8192 files and 16384 faces; font collections cap at 256
faces. Canonical descendants must remain inside those roots; links escaping the
roots are skipped. Discovery reads at most 1 GiB, and each frame at most 256 MiB
of font source bytes (plus one EOF probe byte). Faces sharing a cached source
version share owned bytes, including collection faces; eviction cannot bypass
the frame I/O budget. Oversized/unreadable/malformed system files are skipped as unavailable.
Additional fontconfig/OS registration paths remain pending. Installed font versions
are recorded during discovery; an evicted face cannot reload changed bytes under
its old catalog identity. Cached bytes are immutable; reopen the renderer after
changing installed fonts. In-flight paths and parser working storage are
additional allocations beyond these cache limits.

Native source textures currently cap decoded pixels at 4096x4096 in total (16 Mi
pixels, with other aspect ratios allowed). Rust image decoder allocation is limited
to 128 MiB. The shared cache owns at most 128 MiB and 256 entries; in-flight frames
and codec working buffers are additional allocations. File reads/seeks and pixel
conversion check cancellation; codec CPU work between reads can delay cancellation.
Image requests never fall through to the video decoder: unsupported native formats,
including SVG, fail explicitly. Animated raster image assets use the first bitmap,
as the original image asset decoder does. Color profile/HDR conversion, exact browser
interpolation and arbitrary display rotations remain pending. Cache identity uses
canonical path, length and modification time, not content hashing. Container/VFR
timing needs broader parity coverage beyond the checked CFR/Matroska cases.
Probes carry that source version through frame planning. Changed sources require
resampling before decode, and a change during probing/decoding rejects the result.
Raw video stdout is bounded to the expected RGBA frame size (at most 64 MiB);
unexpected output size fails instead of admitting a different frame resolution.
FFprobe stdout is capped at 16 MiB. Oversized process output triggers kill/reap.

Native exports also cap absolute workarea endpoint indices at 2^40 frames, leaving
precision headroom for conversion between frame indices and seconds. This is
separate from the workarea duration limit and is checked before encoding starts.
The current renderer caps one job at a 24-hour workarea and applies a 24-hour
process timeout. Probe/capture processes have a 120-second timeout.
The evaluator limits modules (512), each source (16 MiB), loop iterations
(1,000,000), asynchronous jobs (10,000), call recursion (128), and output tree
size/depth. It is a local project runtime, not a security boundary for untrusted
code or a general browser implementation.
The VM and recursive host updates use [stacker](https://docs.rs/stacker/0.1.25/stacker/)
to grow the stack on the same thread when necessary, including the smaller
Windows main stack. A regression test mounts the full animated example on a
256 KiB thread stack and samples/disposes it there.

On Antonio's Windows machine, use the existing toolchain under
`D:/Tools/ad3-rust`, with `CARGO_HOME=D:/Tools/ad3-rust/cargo` and
`RUSTUP_HOME=D:/Tools/ad3-rust/rustup`. Set `TEMP`/`TMP` to `D:/tmp` for local test
fixtures. These are machine-local build settings, not required paths for users.
For continued development on this machine, set
`CARGO_TARGET_DIR=E:/AD3_BuildCache/ad3-editor-20260927` to use the prepared SSD
build cache. The source checkout stays in `D:/projects/ad3-editing`. The cache
was copied with preserved timestamps, its main binary/library hashes were
verified and Cargo successfully reused it. Use the compiler, linker and
dependency profiles shown above. With this setting, run the resulting binaries
from `CARGO_TARGET_DIR/debug`.

References for dependencies: [Oxc](https://docs.rs/oxc_parser/0.149.0/oxc_parser/),
[eframe](https://docs.rs/eframe/0.36.2/eframe/),
[tiny-skia](https://docs.rs/tiny-skia/0.12.0/tiny_skia/),
[Boa](https://boajs.dev/docs/intro),
[FFmpeg](https://ffmpeg.org/ffmpeg.html).
