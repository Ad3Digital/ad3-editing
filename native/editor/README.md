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

Two small runnable projects are included: `examples/static` exercises source
editing; `examples/modules` exercises components, imported JSON and generated
rectangles. Pass the full path to either folder to the native executable.

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
- `evaluator.rs`: Rust-hosted ECMAScript modules using Boa and Oxc. Non-reactive
  function components, expressions, arrays, fragments, relative TS/JS imports,
  JSON modules, reexports, top-level await and installed ESM packages with exact
  `exports` entries can materialize a composition. No package is installed by the
  application and no Node/Electron process participates.
- `timing.rs`: 30 fps authoring time, source trims, both out-edge caps, natural
  source durations, nested clocks, container spans/clipping, workareas and dB buses.
- `transport.rs`: seeks, J/K/L shuttle tiers, looping and decoder generations.
- `media.rs`: local probing, linked asset resolution, video frames and waveform.
- `render.rs`: local static scene composition, rectangles, basic unwrapped text,
  video/image fitting, stacking, opacity, trimmed/rate-adjusted audio, mixing,
  workarea MP4 export and PNG/RGBA capture.
- `main.rs`: JSON CLI for inspection, context, structural checks, source edits,
  media inspection, capture and export; no running Electron application required.
- `desktop.rs`: native egui canvas preview, timeline, inspector, source editing,
  cancellable export and export process cleanup when the window closes.

The static reader preserves source spans for editing. When a project needs
module execution, the native evaluator opens it for inspection/rendering; source
edits are refused until multi-file source mapping and instance unrolling exist.
Solid host modules, drawing callbacks, CommonJS, package export wildcards/arrays and
browser remapping tables remain unsupported. Functions in composition properties,
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
| Source/project model | Static TSX span edits; native non-reactive ES module evaluation and components | Complete package resolution, Solid signals/control flow, hooks/tickers, `@inspect`, loop unrolling and multi-file edits; last-good-render behavior |
| Timeline | Source/timeline clocks, workareas, cuts/trims/moves, nested rates, transport | Ripple Q/E, multi-selection, clipboard/grouping, transitions, keyframe tracks, all timeline gestures/colors/toggles and established constraints |
| Graphics | Static media, rects and basic unwrapped text | GPU compositor; every transform, masks, blend modes, gradients, paints, strokes, shadows, effects, wrapped/styled text, HTML, surfaces, WGSL and adjustment layers |
| Audio | Offline retiming, gain and mixing | Real-time output and seek/rate re-anchoring, solo/bus hierarchy, gain/keyframes, syncTo and precise behavior across all cuts/rates |
| Animation/captions | Authored data retained | Easing, preset animations, keyframes, transitions, transcript parsing and every caption preset |
| Media/assets | Local FFprobe and linked YAML sources | Hash/manifest lifecycle, import/relink/move/folders, proxies and bounded caches, frame directories, remote resolution/modifiers, transcripts |
| CLI/IPC | Native explicit-project commands | Existing dapi flags/output/contracts, open/context IPC, logs, fonts, screenshots, filmstrips/contact sheets and all applicable local commands |
| HyperFrames | Existing separate package untouched | Rust save/template/preview/render/cancel engine, project isolation, immutable outputs and linked timeline updates |
| Desktop | Basic native editor | All inspector/canvas interactions, shortcuts, native pickers/menus, project dashboard/create/rename/duplicate, watcher/reload, session persistence, accessible UI QA |
| Export | Opaque H.264/AAC MP4, PNG capture and cancellation | All current containers/codecs, alpha/sequences, frame-exact animation/audio, progress and complete error recovery, high resolution and fractional fps comparisons |
| Delivery | Local Cargo crate | Windows/macOS builds and packaging, bundled native tools/fonts/licenses, install/launch/upgrade checks and removal of Electron/Node/TypeScript application dependency |

Project format migration and authoring API compatibility must be addressed by
the native evaluator rather than silently flattening or rewriting dynamic source.
The existing JavaScript project language is user-authored content; implementing
its host/evaluator in Rust is compatible with a Rust application.

## Verification

```sh
cargo fmt --manifest-path native/editor/Cargo.toml -- --check
cargo test --locked --manifest-path native/editor/Cargo.toml
cargo test --locked --manifest-path native/editor/Cargo.toml --test rendering -- --ignored
cargo build --locked --manifest-path native/editor/Cargo.toml --features desktop
```

The explicit FFmpeg integration test creates its own video/audio fixture. It
checks workarea duration, dimensions/fps, source motion, the overlaid color,
audible dB gain, capture, cancellation/reaping, temporary-file cleanup, missing
streams and refusal to overwrite. Source tests cover UTF-8 and
comments, split/trim at altered rate, inherited timing/gain, unsupported dynamic
modules, JavaScript text coercion, atomic save, validated undo/redo and external
edit conflicts. A debug framebuffer smoke test also opened the native editor and
captured the real egui window. These are gates for
the implemented subset; they do **not** prove complete editor parity.

Module evaluation tests cover typed function components, map/conditionals,
fragments, JSON, installed ESM packages, exact export conditions, reexports,
top-level await, edit refusal, optional undefined data normalization, unsupported
host callbacks and bounded loop/job
execution. These do not verify reactive lifecycle parity.

The current renderer caps one job at a 24-hour workarea and applies a 24-hour
process timeout. Probe/capture processes have a 120-second timeout.
The evaluator limits modules (512), each source (16 MiB), loop iterations
(1,000,000), asynchronous jobs (10,000), call recursion (128), and output tree
size/depth. It is a local project runtime, not a security boundary for untrusted
code or a general browser implementation.

On Antonio's Windows machine, use the existing toolchain under
`D:/Tools/ad3-rust`, with `CARGO_HOME=D:/Tools/ad3-rust/cargo` and
`RUSTUP_HOME=D:/Tools/ad3-rust/rustup`. Set `TEMP`/`TMP` to `D:/tmp` for local test
fixtures. These are machine-local build settings, not required paths for users.

References for dependencies: [Oxc](https://docs.rs/oxc_parser/0.149.0/oxc_parser/),
[eframe](https://docs.rs/eframe/0.36.2/eframe/),
[Boa](https://boajs.dev/docs/intro),
[FFmpeg](https://ffmpeg.org/ffmpeg.html).
