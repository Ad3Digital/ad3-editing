// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
mod atomic_file;
mod audio;
pub mod audio_output;
pub mod audio_session;
pub mod audio_stream;
pub mod audio_sync;
pub mod color;
pub mod compositor;
pub mod contact_sheet;
mod control;
pub mod document;
pub mod easing;
mod effects;
mod encoder;
pub mod evaluator;
pub mod filmstrip;
mod font_catalog;
pub mod font_cli;
pub mod fonts;
pub mod frame_triage;
pub mod geometry;
pub mod grab;
pub mod inspect;
pub mod jsx;
mod materialize;
pub mod media;
pub mod motion;
mod paints;
pub mod playback_audio;
#[cfg(windows)]
pub mod playback_session;
pub mod project;
pub mod project_config;
pub mod props;
pub mod reactive;
pub mod render;
pub mod source;
mod source_inspect;
mod stack;
mod text;
pub mod text_layout;
pub mod text_motion;
pub mod texture;
pub mod timing;
pub mod transport;
pub mod video_clock;
pub mod video_decode;
pub mod video_session;
pub mod waveform;
#[cfg(windows)]
pub mod windows_audio;

pub type Result<T> = std::result::Result<T, String>;
