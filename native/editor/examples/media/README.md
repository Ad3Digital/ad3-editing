# Native media composition

Open this folder in the native desktop editor or capture/export it with `dapi-rust`.
The included assets are generated test patterns, with no external footage.
`generate-assets.py` documents their creation (Python standard library + FFmpeg);
it refuses to overwrite existing files.

The scene mixes a trimmed video at 1.25x playback with translucent PNG images,
rounded clips, contain/fill/cover fitting, group transforms and animated geometry.
Rust performs composition. Rust codecs decode the PNG; FFmpeg decodes the video
and encodes the final MP4.
