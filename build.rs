//! Build script: tell Cargo to rebuild when embedded templates change.
//!
//! `src/cli/run/template.rs` uses `include_dir!("$CARGO_MANIFEST_DIR/templates-data")`
//! to embed the official template directory tree at compile time. Without this
//! script, Cargo only re-evaluates the macro when a Rust source file changes,
//! so adding or editing files under `templates-data/` could leave the embedded
//! data stale until something in `src/` is touched. We emit a `rerun-if-changed`
//! line for the directory itself so any addition / removal / file edit under
//! `templates-data/` triggers a rebuild.

fn main() {
    println!("cargo:rerun-if-changed=templates-data");
}
