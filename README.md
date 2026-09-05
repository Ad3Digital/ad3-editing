# AD3 Editing

<p align="center"><img src="apps/desktop/assets/icon.png" alt="AD3 Editing" width="128" /></p>

**Agentic video editing for AI agents and human editors.**  
Editor de vídeo agêntico com IA e edição humana.

AI agents work through a structured CLI and composition code. People work on the
canvas and timeline. Both edit the same project, with changes written back to its
SolidJS/TypeScript source.

[Download Windows](https://github.com/Ad3Digital/ad3-editing/releases/latest) ·
[CLI reference](reference/README.md) ·
[Composition reference](reference/jsx/README.md) ·
[Issues](https://github.com/Ad3Digital/ad3-editing/issues)

## AI and humans, in the same editing workflow

- An agent can inspect footage, open projects, edit composition files, capture
  preview frames, check scenes, and export through `dapi`.
- A human can review the result, cut and trim clips, adjust the canvas, and finish
  the edit in the desktop interface.
- Canvas and timeline changes are written back into the composition source.
  Editing the source recompiles the project and updates the preview.
- Projects remain ordinary local folders, suitable for version control and use
  with any coding agent that can access the filesystem and terminal.

The editor is **model-agnostic**. This release does not bundle an LLM or a chat
service: bring your own coding agent and its credentials. The Windows workflow
is local-only; upstream cloud generation, accounts, billing, and credits are not
part of this release.

## Install on Windows

Current release: **0.202.0-windows.2**, Windows x64.

1. Open [Releases](https://github.com/Ad3Digital/ad3-editing/releases/latest).
2. Download `AD3-Editing-x64-Setup.exe`, or extract the portable ZIP.
3. Launch **AD3 Editing**.

The installer is currently **unsigned**. Windows may display a SmartScreen warning.
Only install binaries from the repository's releases or build from source.

For compatibility with existing local installations, the internal executable is
still `Diffusion Studio.exe`. The Squirrel installation directory remains
`%LOCALAPPDATA%\DiffusionStudio`, and the profile remains
`%APPDATA%\Diffusion Studio`. Existing projects do not need a format migration.
The application does not download upstream application updates.

Other operating systems are not validated or distributed by this Windows release.

## Editing controls

| Input | Action |
| --- | --- |
| Wheel over timeline | Zoom around the cursor |
| Ctrl + wheel | Pan horizontally without changing the zoom or playhead |
| Shift + wheel | Scroll track rows vertically |
| Horizontal trackpad gesture | Pan timeline horizontally |
| J | Reverse shuttle: 2× → 3× → 4× → 5× |
| L | Forward shuttle: 2× → 3× → 4× → 5× |
| K during shuttle | Resume forward playback at 1× |
| K at normal speed | Toggle play/pause |
| Space | Play/pause; hold over the canvas for the hand tool |
| W | Cut at the playhead |
| Q / E | Ripple trim to the previous / next cut |

Changing shuttle direction starts again at 2×. Holding a key does not increase
shuttle speed, and typing inside inputs does not trigger editing shortcuts.
Forward shuttle audio follows playback speed; reverse playback is silent.
Shuttle speed never retimes authored clips or changes export timing.

## Working with an AI agent

The desktop distribution includes `resources/cli/bin/dapi.cmd`. Invoke it directly
or add that directory to your terminal's PATH. Run `dapi --help` for the full CLI.

```sh
dapi open path/to/project
dapi context
dapi media probe path/to/clip.mp4
dapi media filmstrip path/to/clip.mp4
```

Use the returned scene IDs with `dapi capture` and `dapi check`. `dapi export`
renders a project to a local video file; see its `--help` for output options.
The [CLI reference](reference/README.md) documents arguments and structured output.
Some optional media commands require additional tools or providers; consult their
individual references rather than assuming a bundled cloud service.

A useful division of work is: let the agent assemble and inspect a cut, review it
in the editor, then have the agent check and export the revised composition.

`dapi report` files a **public** issue in this repository and can attach application
logs. Review the diagnostics first, or use `--logs 0` to omit log attachments.
It requires an installed and authenticated GitHub CLI.

## Preview and playback improvements

- Coalesced seek requests instead of an accumulated promise chain.
- Codec output drained for short clips, final frames, and keyframe scrubs.
- Idle decoders reactivated at the requested frame.
- Bounded, indexed preview caches and direct drawing into preview tiles.
- Cache resolution adapted to frame rate within a fixed pixel budget.
- Audio re-anchored after seeks or rate changes, with obsolete work cancelled.

High-resolution, long-GOP footage can show fewer intermediate preview frames
during fast reverse shuttle. Export decoding remains full-resolution.

## Build from source

Use **Node.js 24**, npm, Git, and Windows x64 to build the published Windows package.

```sh
git clone https://github.com/Ad3Digital/ad3-editing.git
cd ad3-editing
npm ci
```

Copy `apps/web/.env.example` to `apps/web/.env` for a new checkout. The template
contains public client configuration; do not commit private credentials or replace
an existing local configuration unnecessarily.

```sh
npm run check
npm run make
```

Outputs:

- Installer: `apps/desktop/out/make/squirrel.windows/x64/AD3-Editing-x64-Setup.exe`
- Portable ZIP: `apps/desktop/out/make/zip/win32/x64/`

For development, run `npm run dev`. `npm run check` checks all workspaces and the
examples. `npm run lint` runs the repository's linters.

## Repository layout

| Path | Purpose |
| --- | --- |
| `apps/web` | SolidJS editor interface |
| `apps/desktop` | Electron application and Windows packaging |
| `apps/cli` | Agent-facing `dapi` CLI |
| `packages/runtime` | Scene state, actions, playback, media decoding, and capture |
| `packages/reconciler` | Composition-to-runtime reconciliation |
| `packages/jsx` | Composition authoring API |
| `packages/assets` | Asset library, manifests, probing, and resolution |
| `packages/encoder` | Video, audio, and image export |
| `packages/koota-solid` | Solid bindings for the entity runtime |
| `reference` | CLI and composition documentation |
| `examples` | Example compositions |

Internal `@diffusionstudio/*` package names are preserved for composition and API
compatibility. They do not change the public product name or repository owner.

## Origin and license

AD3 Editing is independently maintained by **AD3 Digital** and is based on
[Diffusion Studio Editor](https://github.com/diffusionstudio/editor), including its
SolidJS composition format, runtime, and CLI. This repository begins a new public
AD3 history; that does not replace the authorship or licensing of inherited code.

Source code is available under the **[Mozilla Public License 2.0](LICENSE)**.
Commercial use, modification, and redistribution are permitted. Distributed
modifications to MPL-covered files must remain available under the MPL, and the
applicable notices must be retained. See the license for the complete terms.

Original Diffusion Studio brand assets are not covered by MPL-2.0.
Copyright (c) Diffusion Studio Inc. All rights reserved.
The original AD3 Editing icon and marks created for this fork are supplied under
MPL-2.0. Third-party components and assets retain their respective licenses.
