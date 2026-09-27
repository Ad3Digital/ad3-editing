// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Boa/native JSX callbacks can use more than the Windows main thread's 1 MiB
//! stack. Grow on the same thread so Context/GC ownership never migrates.
pub(crate) fn runtime<T>(run: impl FnOnce() -> T) -> T {
    stacker::maybe_grow(2 * 1024 * 1024, 16 * 1024 * 1024, run)
}
pub(crate) fn recursive<T>(run: impl FnOnce() -> T) -> T {
    stacker::maybe_grow(512 * 1024, 8 * 1024 * 1024, run)
}
