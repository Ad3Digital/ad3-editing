---
name: ad3-editing
description: Editar videos no AD3 Editing com a CLI dapi e composicoes JSX editaveis. Use para montar ou refinar takes, cortes por fala, audio, legendas e exportacao no editor, preservando ajustes humanos da timeline.
---

# AD3 Editing

Edit the same composition that a person can refine on the timeline. The desktop
application supplies `dapi`; the agent supplies editorial decisions and edits
the project's JSX. This skill does not install an LLM or add a panel to the app.

## Locate the application and project

Use `dapi --help` from the installed distribution. On Windows its launcher is
`resources/cli/bin/dapi.cmd`; on macOS it is
`AD3 Editing.app/Contents/Resources/cli/bin/dapi`. Resolve the installed path on
this machine instead of assuming a username, drive or version. In PowerShell,
invoke an absolute launcher path with `&` and quote paths containing spaces.

Run `dapi context` before modifying an open project. Confirm `projectDir`, then
read its current entry file (`package.json` main, or `index.tsx`). Canvas and
timeline edits write back to source: preserve current IDs, cuts, active scene,
assets and human adjustments. Re-read before saving if the person is editing
at the same time. Keep a recoverable source copy outside watched entry files.

To open or create the intended project:

```sh
dapi open "path/to/project"
dapi context
dapi media probe "path/to/take.mp4"
dapi media filmstrip "path/to/take.mp4"
dapi media waveform "path/to/take.mp4"
```

Opening a missing project creates it. Use a separate folder for experiments.
Keep original footage intact and analysis, transcripts and exports in `edit/`.
Use the [CLI reference](https://github.com/Ad3Digital/ad3-editing/blob/main/reference/README.md)
and each command's `--help` for options; use repository-local `reference/` when
available. Read only the relevant JSX reference before using unfamiliar props.

## Select and refine cuts

Inspect the footage and the user's brief before choosing pace, framing and
structure. Use existing approved editorial presets when the workspace provides
them. Resolve routine choices from the request; discuss only material creative
choices that remain unspecified. Existing approval remains valid.

For speech-led edits, use a local word-timed transcript if available. Associate
each transcript with its exact source; a generic transcript filename shared
across several takes is ambiguous. Existing `video-use` and `video-edit-local`
skills can supply transcript tooling, but are not installation dependencies.
If local transcription is unavailable, use supplied timestamps or inspect the
audio manually and state the timing limitation. Do not imply it is bundled.

- Pick complete thoughts and the strongest takes; retain emphasis, reactions
  and grammatical beginnings. Remove false starts only after listening.
- Treat silence as a candidate, not an automatic deletion. Keep enough air for
  the intended pace; check breaths and word attacks against the actual audio.
- ASR word boundaries are estimates. Never apply a fixed offset that could
  remove the beginning of a word without checking the source.
- For a substantial edit, record source, source in/out, timeline start, quote
  and reason for each kept range in `edit/edl.json` or a concise cut table.
  This is a planning artifact; `dapi` does not import that EDL automatically.
- Keep picture and dialogue aligned. Separate tracks only for a deliberate
  J/L cut or mix; do not accidentally play both embedded and detached audio.
- Inspect and listen around every changed cut. Add short gain fades if needed
  to remove clicks without clipping speech. Add B-roll only when it helps the
  requested edit.

## Author the editable timeline

The entry file default-exports a Solid component containing `<stage>` and
`<scene>`. Keep one intended scene `active`, with explicit width and height.
Use the scene's stable JSX `id` for `check` and `capture`; `context` reports the
project and playhead, not a scene-ID list.

`start` is a position on the parent timeline. `sourceIn` and `sourceOut` select
the part of the original file. At rate 1, duration is `sourceOut - sourceIn`;
at other rates divide by `playbackRate`. Place the next clip explicitly at the
previous clip's end, including inside a `<sequence>`.

Prefer either `sourceOut` or `end` for an out edge. If both are present, both
cap playback and the earlier one wins. After recutting, update the scene's
`workarea` to the intended export interval; it does not follow the cuts for you.

Example: play source seconds 2–5, then 8–10, as a continuous five-second edit:

```tsx
export default function Project() {
  return (
    <stage>
      <scene id="main" name="Main" width={1920} height={1080}
        active workarea={[0, 5]}>
        <video id="take-a" src="take.mp4" width={1920} height={1080}
          start={0} sourceIn={2} sourceOut={5} />
        <video id="take-b" src="take.mp4" width={1920} height={1080}
          start={3} sourceIn={8} sourceOut={10} />
      </scene>
    </stage>
  );
}
```

Read the [timing reference](https://github.com/Ad3Digital/ad3-editing/blob/main/reference/jsx/timing.md)
when nesting or retiming clips. Prefer numeric seconds: the editor's `"30f"`
time notation uses a 30-fps internal clock, independent of delivery frame rate.

## Audio, captions and overlays

`volume` is **decibels**, not linear gain: `0` preserves level, negative values
attenuate. Start a music bed quietly and listen against dialogue; a numeric
FFmpeg gain such as `0.16` must not be copied into this prop. Use `gain`
animations for audible fades; a visual `fade` does not fade the audio.

Use local transcripts with an explicit caption source, for example
`<captions src="edit/subtitles/final.srt" preset="whisper" />`. Omitting `src`
requests automatic transcription, which can require a cloud provider. Local
editing is the default; uploads and paid providers need applicable approval.

After cuts, either build a transcript in **final scene time**, or use separate
caption nodes with the same source trims and timeline placements as their
associated clips. Do not mix the two time bases. For a kept range beginning at
source `in` and timeline `start`, a retained word at source `t` moves to
`start + (t - in) / playbackRate`. Verify names and boundary words by listening.

Keep captions visible over overlays and inside the safe region for the chosen
format. Check the rendered frame rather than relying on styling props alone.
The installed HyperFrames panel can create, preview, render and insert linked
motion elements. Use it when the requested edit needs them; keep editable HTML
and existing asset links. No HyperFrames CLI command is assumed to exist in
`dapi`. For grading or compositing, consult the supported JSX effects/shaders
or prepare a separate local media derivative; do not invent Resolve APIs.

## Check and export

```sh
dapi check main
dapi capture main --time 0 2.9 3.1 4.9 --output "edit/verify"
dapi export --help
dapi export "edit/exports/final.mp4" --resolution 1080 --fps 30
```

These capture times match the five-second example; choose actual cut boundaries
for the current project. Capture positions start at the **workarea's first
frame**. Convert a scene time to capture time by subtracting workarea start.

`check` is structural: no reported gap does not prove the image or audio is
correct. Inspect captures and listen to the edited result, especially cut
boundaries, the first/last seconds, captions and music. Export renders the
**active** scene. `--resolution` means output **height**: use 1920 for a
1080×1920 portrait scene, and choose frame rate deliberately.

Verify the exported file's duration, dimensions, frame rate and expected audio
with `ffprobe` or `dapi media probe`, then inspect the actual exported frames.
Confirm the workarea excludes unintended black tails. A stale preview after a
compile failure is not proof that the new source rendered; inspect `dapi logs`
and resolve the error. Deliver the editable project path and export path with
remaining limitations. `dapi report` publishes a public issue and may attach
logs; it is not a local verification step.
