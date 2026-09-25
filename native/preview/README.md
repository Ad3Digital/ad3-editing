# Native preview (Windows)

The desktop video preview uses a Rust sidecar and the bundled FFmpeg. The Electron/Solid interface, timeline model, audio scheduling and frame-exact export remain unchanged. This is a migration of preview decoding, not a rewrite of the entire editor.

## Build

Install Rust with the MSVC toolchain and Visual Studio C++ build tools, then run `npm run build:preview --workspace=@diffusionstudio/desktop`. The normal desktop build also runs this step on Windows. Cargo uses the committed lockfile; the executable is staged under `apps/desktop/dist` and included by the existing packager. Other platforms retain WebCodecs for now.

The installed worker uses `resources/hyperframes-engine/ffmpeg/ffmpeg.exe`, already staged with the app and its license notices. Development can set `AD3_FFMPEG_PATH`; `CARGO` can select a Cargo executable. `AD3_NATIVE_PREVIEW=0` before launching the app disables the native preview for troubleshooting.

## Contract

Renderer → restricted preload IPC → main process → Rust stdin. Rust returns a little-endian 4-byte JSON-header length, the JSON header, then `bytes` raw RGBA bytes. No ports, shell commands or remote media protocols are used.

Each active clip has a bounded request queue and persistent FFmpeg process. The client has at most one frame request in flight and coalesces cursor movement to its latest target. Old scrub responses are discarded. Native sessions close when a clip leaves the warmup range, the renderer navigates, or the app exits. Stdout is copied incrementally rather than repeatedly concatenating complete frames.

Preview is at most 960×540 and 30 source frames/second; the scene clock remains authoritative for speed and audio synchronization. Future clips warm up two wall-clock seconds ahead at the current shuttle speed. Paused seeks resolve the requested position to within one preview frame. Original media and export resolution are not modified.

## Validation

```
cargo test --manifest-path native/preview/Cargo.toml
node --test scripts/native-preview-client.test.mjs scripts/timeline-performance.test.mjs scripts/audio-retiming.test.mjs
node --test scripts/native-preview.test.mjs
```

The native integration test needs a release binary and FFmpeg. It creates a moving fixture by default; `AD3_PREVIEW_TEST_SOURCE` selects a real local video. It checks 2×/5× requests, backward jumps, invalid-input recovery, reopening a session, different image hashes, timestamps and clean worker exit.

Cold random seeks still depend on the source GOP and CPU decode speed. Reverse playback can require repeated decoder seeks. These limits are not solved simply by using Rust; future proxy/cache work can address them. The app retains its existing export decoder rather than routing exports through the lower-resolution preview.
