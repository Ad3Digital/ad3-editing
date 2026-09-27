// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
fn main() {
    println!("cargo:rerun-if-env-changed=AD3_NATIVE_PDB");
    println!("cargo:rerun-if-changed=build.rs");
    // Opt-in only. Debug assertions/overflow checks stay enabled; no dependency
    // compilation flags change. Release packaging keeps its normal settings.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
        && std::env::var("PROFILE").as_deref() == Ok("debug")
        && std::env::var("AD3_NATIVE_PDB").as_deref() == Ok("0")
    {
        println!("cargo:rustc-link-arg=/DEBUG:NONE");
    }
}
