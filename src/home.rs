//! Locates the directories the toolchain ships beside the compiler, `lib` for
//! the standard library and `runtime` for the C runtime sources. A source
//! checkout finds them at the crate root, an installed binary finds them in
//! the share directory beside itself, and DUSK_HOME overrides both.

use std::env;
use std::path::{Path, PathBuf};

/// The share directory name an installed toolchain uses, as in
/// /usr/share/dusk-lang beside /usr/bin/dusk.
const SHARE_DIR: &str = "dusk-lang";

/// Resolves a shipped asset directory by name, `lib` or `runtime`.
/// Resolution order: the DUSK_HOME environment variable, the share directory
/// two levels up from the running binary, then the crate root baked in at
/// compile time, which keeps `cargo test` and a source checkout working with
/// no environment at all. The first candidate that exists on disk wins, and
/// the baked path is returned even when absent so the caller's error names a
/// real location.
pub fn asset_dir(name: &str) -> PathBuf {
    if let Ok(home) = env::var("DUSK_HOME") {
        let d = PathBuf::from(home).join(name);
        if d.is_dir() {
            return d;
        }
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(prefix) = exe.parent().and_then(|bin| bin.parent()) {
            let d = prefix.join("share").join(SHARE_DIR).join(name);
            if d.is_dir() {
                return d;
            }
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_the_crate_root_in_a_checkout() {
        // The test binary sits in target/debug/deps, so the exe relative
        // share directory does not exist and the baked crate root wins.
        let lib = asset_dir("lib");
        assert!(lib.ends_with("lib"), "{lib:?}");
        assert!(lib.join("std").is_dir(), "the checkout stdlib should exist: {lib:?}");
    }

    #[test]
    fn dusk_home_wins_when_it_holds_the_directory() {
        // Uses the crate root itself as a stand in DUSK_HOME, since it
        // contains a real lib directory. Serialized by running in one test
        // process is not guaranteed, so restore the variable afterward.
        let root = env!("CARGO_MANIFEST_DIR");
        let prev = env::var_os("DUSK_HOME");
        env::set_var("DUSK_HOME", root);
        let lib = asset_dir("lib");
        match prev {
            Some(v) => env::set_var("DUSK_HOME", v),
            None => env::remove_var("DUSK_HOME"),
        }
        assert_eq!(lib, Path::new(root).join("lib"));
    }
}
