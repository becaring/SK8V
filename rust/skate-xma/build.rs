// Link the VC runtime statically into the skate-xma binary so it needs no
// redistributable DLL (the UCRT is part of Windows 10+). Same approach as
// the static_vcruntime crate; applies to this package's binaries only.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    let features = std::env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
    if target.contains("windows-msvc") && !features.split(',').any(|f| f == "crt-static") {
        for lib in ["libvcruntimed.lib", "vcruntime.lib", "vcruntimed.lib", "libcmtd.lib", "msvcrt.lib", "msvcrtd.lib", "libucrt.lib", "libucrtd.lib"] {
            println!("cargo:rustc-link-arg-bins=/NODEFAULTLIB:{lib}");
        }
        for lib in ["libcmt.lib", "libvcruntime.lib", "ucrt.lib"] {
            println!("cargo:rustc-link-arg-bins=/DEFAULTLIB:{lib}");
        }
    }
}
