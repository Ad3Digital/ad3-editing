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

Five runnable projects are included: `examples/static` exercises source editing;
`examples/modules` exercises imported components/JSON; `examples/reactive` animates
rectangles and text through the native ticker, signals, `For` and `Show`;
`examples/inspect` exposes editable controls from an imported component module;
`examples/motion` combines slide/fade/text presets with an animated color track.
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
- `media.rs`: local probing, linked asset resolution, video frames and waveform.
- `render.rs`: local static scene composition, rectangles, basic unwrapped text,
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
  position/color/opacity/gain tracks. Transform/blur presets and style targets
  still require their pending graphics implementations; unsupported render
  properties fail explicitly. Group opacity inherits per draw, as in the original
  Canvas renderer, including retention of inherited alpha when a computed
  assignment is outside [0,1], following the
  [Canvas contract](https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-globalalpha).
  Width/height rendering still requires integers.
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
| Graphics | Static media, rects and basic unwrapped text | GPU compositor; every transform, masks, blend modes, gradients, paints, strokes, shadows, effects, wrapped/styled text, HTML, surfaces, WGSL and adjustment layers |
| Audio | Offline retiming and Rust stereo mixing with per-frame reactive/preset/keyframe gain | Real-time output and seek/rate re-anchoring, solo/bus hierarchy, sample-accurate gain automation, syncTo and precise behavior across all cuts/rates |
| Animation/captions | Native preset/keyframe sampling integrated with preview/export for supported render properties | Transform/blur/style rendering, sequence targeting, transition behavior, track editing UI, transcript parsing and every caption preset |
| Media/assets | Local FFprobe and linked YAML sources | Hash/manifest lifecycle, import/relink/move/folders, proxies and bounded caches, frame directories, remote resolution/modifiers, transcripts |
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
the language's fixed 30 fps unit before conversion. The encoder writes complete frames
(`floor(workarea duration * output FPS)`). The report distinguishes the authored
workarea duration from the video duration; audio covers the workarea at 48 kHz.
`playing()` is true in exports. The ticker's frame accessor uses the output FPS
in exports and 30 fps in the editor/capture, matching the current engine behavior.
Reactive export currently composes one CPU frame per FFmpeg invocation; native
GPU rendering and persistent decoders remain pending. Decoded audio is held in
job-owned temporary files, with a limit of 128 distinct source/rate variants.

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

References for dependencies: [Oxc](https://docs.rs/oxc_parser/0.149.0/oxc_parser/),
[eframe](https://docs.rs/eframe/0.36.2/eframe/),
[Boa](https://boajs.dev/docs/intro),
[FFmpeg](https://ffmpeg.org/ffmpeg.html).
