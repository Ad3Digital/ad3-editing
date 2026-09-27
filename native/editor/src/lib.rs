// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
mod atomic_file;
mod audio;
mod control;
pub mod document;
mod encoder;
pub mod evaluator;
pub mod inspect;
pub mod jsx;
mod materialize;
pub mod media;
pub mod project;
pub mod props;
pub mod reactive;
pub mod render;
pub mod source;
mod source_inspect;
mod stack;
pub mod timing;
pub mod transport;

pub type Result<T> = std::result::Result<T, String>;
