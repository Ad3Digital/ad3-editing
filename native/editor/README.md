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

Nine runnable projects are included: `examples/static` exercises source editing;
`examples/modules` exercises imported components/JSON; `examples/reactive` animates
rectangles and text through the native ticker, signals, `For` and `Show`;
`examples/inspect` exposes editable controls from an imported component module;
`examples/motion` combines slide/fade/text presets with an animated color track;
`examples/vectors` exercises native rounded shapes, transforms and group opacity;
`examples/media` combines native image/video composition with generated test assets;
`examples/text` exercises wrapped text, variable font weights, italic ranges and rotation;
`examples/paints` exercises animated gradients, glyph outlines and screen blending.
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
  byte spans, missing-ID stamping on edits, split/trim/move/remove/set, active
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
  batching, cleanup scopes, `createRoot`, `onMount`, `untrack`, `mergeProps`,
  `splitProps`, `children`, keyed `For`, positional `Index`, `Show`, and `useTicker`.
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
  rounded corners and scene clipping. Group bounds follow the original parent-first
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
(including stores, resources, contexts, `Switch`/`Match`) are still pending.
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

## Full conversion acceptance ledger

The source of requirements is `reference/jsx/`, `reference/`, the existing desktop
implementation and the runtime/encoder/assets/HyperFrames packages. Nothing in
this ledger is waived because a smaller example renders successfully.

| Area | Current implementation | Required before replacement |
| --- | --- | --- |
| Source/project model | Static TSX edits; native ES modules, reactive props, signals/effects, For/Index/Show and ticker; original file spans; live `@inspect` and imported initializer writeback | Remaining package/Solid/browser APIs, generation hooks, loop unrolling and general multi-file JSX edits; complete last-good-render behavior |
| Timeline | Source/timeline clocks, workareas, cuts/trims/moves, nested rates, transport | Ripple Q/E, multi-selection, clipboard/grouping, transitions, keyframe tracks, all timeline gestures/colors/toggles and established constraints |
| Graphics | Native transformed/rounded vectors, images, video, wrapped text/ranges, solid/gradient paint stacks, solid strokes and blend modes | GPU compositor, complete font fallback/scripts/emoji and synthetic styles, remaining transforms/constraints, masks, media/HTML/surface/shader paints, shadows, effects, remaining text styling, HTML, surfaces, WGSL and adjustment layers |
| Audio | Offline retiming and Rust stereo mixing with per-frame reactive/preset/keyframe gain | Real-time output and seek/rate re-anchoring, solo/bus hierarchy, sample-accurate gain automation, syncTo and precise behavior across all cuts/rates |
| Animation/captions | Native preset/keyframe sampling integrated with preview/export, including vector/media/text transforms | Blur/style rendering, sequence targeting, transition behavior, track editing UI, transcript parsing and every caption preset |
| Media/assets | Rust raster image codecs/EXIF, FFprobe/video RGBA, linked YAML sources and bounded shared decoded texture cache | Hash/manifest lifecycle, import/relink/move/folders, SVG/remaining formats, color profiles/HDR, proxies and persistent decoders/cache tiers, VFR/container timestamp parity, frame directories, remote resolution/modifiers, transcripts |
| CLI/IPC | Native explicit-project commands | Existing dapi flags/output/contracts, open/context IPC, logs, fonts, screenshots, filmstrips/contact sheets and all applicable local commands |
| HyperFrames | Existing separate package untouched | Rust save/template/preview/render/cancel engine, project isolation, immutable outputs and linked timeline updates |
| Desktop | Basic native editor | All inspector/canvas interactions, shortcuts, native pickers/menus, project dashboard/create/rename/duplicate, watcher/reload, session persistence, accessible UI QA |
| Export | H.264/AAC MP4, reactive frame sampling, progress/cancellation and fresh mounts; PNG capture | All current containers/codecs, alpha/sequences, complete animation/audio parity and error recovery, high resolution and broader fractional fps comparisons |
| Delivery | Local Cargo crate | Windows/macOS builds and packaging, bundled native tools/fonts/licenses, install/launch/upgrade checks and removal of Electron/Node/TypeScript application dependency |

Project format migration and authoring API compatibility must be addressed by
the native evaluator rather than silently flattening or rewriting dynamic source.
The existing JavaScript project language is user-authored content; implementing
its host/evaluator in Rust is compatible with a Rust application.

## Verification

```sh
cargo fmt --manifest-path native/editor/Cargo.toml -- --check
cargo test --locked --manifest-path native/editor/Cargo.toml
cargo test --locked --manifest-path native/editor/Cargo.toml --test integration rendering:: -- --ignored
cargo build --locked --manifest-path native/editor/Cargo.toml --features desktop
```

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
Nested scenes, masks, aspect constraints and negative-size rounded shapes fail
explicitly. Rounded corners use a fine cubic approximation;
browser Canvas antialiasing is not assumed byte-identical. Preview currently
shows the normalized scene, rather than a complete stage/camera editing view.
Output resizing keeps the original image encoder's uniform resolution even
when rounding canvas dimensions leaves a partial edge pixel or crops a fraction
of one. Repeated resizes derive their ratio from the original scene dimensions.
Color/corner keyframes preserve the original trait membership: a color track
does not paint an unfilled rectangle, and mixed-corner fallbacks resolve before
an animated uniform radius.

Paint/stroke blending and opacity are inherited per draw. A `sourceOver` or
unknown blend value removes the original trait and inherits the parent's mode.
Invalid alpha assignments retain the current Canvas value; nonpositive stroke
width/miter assignments retain the preceding line state. Text solid paint/stroke
color and opacity read authored traits, as in the original text renderer;
gradient stops/geometry and stroke width read animated values. This preserves
the existing difference between text styles and shape styles. A hidden range
does not suppress its intrinsic text styling, while a hidden paint is skipped.
Gradient stops remain included even when their own `hidden` prop is true.

Native planning limits each scene to 100000 nodes/128 levels and each frame to
65536 draws, eight million styled path points and one million gradient stop
applications. A holder accepts 1024 paints/strokes; a gradient accepts 4096 stops.
Stroke width is limited to 8192 and miter ratio to one million. Planning checks
cancellation before committing new geometry history. Large raster buffers use
fallible reservation: at most 256 MiB for pixels and 64 MiB per clip mask. Media
clipping combines one temporary mask with the scene mask in place. These bounds
exclude decoded textures, fonts and the rasterizer's additional working storage;
they do not guarantee recovery from every allocation inside dependencies.

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
