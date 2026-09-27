// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
pub mod document;
pub mod evaluator;
pub mod media;
pub mod render;
pub mod source;
pub mod timing;
pub mod transport;

pub type Result<T> = std::result::Result<T, String>;
