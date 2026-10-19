//! Embeds the Windows application manifest into makit.exe (#285).
//!
//! Without it Windows binds the exe to comctl32 v5.82, which has no `TaskDialogIndirect`
//! (GPUI's confirm dialog calls it), so the exe dies at load time with "entry point not found".
//! The manifest also opts in to PerMonitorV2 DPI awareness, which GPUI assumes.
//!
//! This has to live in the final binary's crate: `embed-resource` links via `rustc-link-arg-bins`,
//! which only reaches the bins of the package whose build script emitted it, so gpui's own
//! `windows-manifest` feature cannot do it for us.

fn main() {
    println!("cargo:rerun-if-changed=resources/windows/makit.manifest.xml");
    println!("cargo:rerun-if-changed=resources/windows/makit.rc");
    #[cfg(windows)]
    embed_resource::compile("resources/windows/makit.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
