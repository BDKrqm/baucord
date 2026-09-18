fn main() {
    // macOS: FFmpeg's `__builtin_available(macOS N, *)` checks (in
    // hwcontext_videotoolbox.c, and anything else built against a
    // newer SDK than the deployment target) call
    // `___isPlatformVersionAtLeast`, which lives in clang's compiler-rt
    // — a library clang adds to a link on its own, but rustc links with
    // `-nodefaultlibs` and its own `compiler_builtins`, which lacks it.
    // With `MACOSX_DEPLOYMENT_TARGET` below the SDK the release link
    // failed on exactly that symbol; without the variable the checks
    // fold away and nothing notices. Ask the toolchain where the
    // library is and link it; a missing answer is a warning, and the
    // link then fails only when the symbol is actually needed.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        link_compiler_rt();
    }

    // FFmpeg static libs on Windows pull in DirectShow (dshow), Media
    // Foundation (mfenc), and COM symbols that aren't linked by default.
    //
    // Checked via CARGO_CFG_TARGET_OS (the *target*), not cfg! (which in
    // a build script evaluates for the *host*) — matters now that this
    // crate cross-compiles for mobile targets.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // vcpkg's static ffmpeg[x264] port leaves libx264 for the
        // consumer to link (ffmpeg-sys-next only names the av* libs).
        // It used to live in the desktop binary's build script, which
        // meant this crate's own examples and tests could not link on
        // Windows (LNK2001 x264_*). A dependency's link directive
        // reaches every binary that links it, so here is where it goes.
        println!("cargo:rustc-link-lib=static=libx264");
        for lib in &[
            "ole32",    // COM runtime (CoCreateInstance, etc.)
            "oleaut32", // OLE Automation (SysAllocString, etc.)
            "strmiids", // DirectShow interface IIDs (IID_IBaseFilter, CLSID_FilterGraph, etc.)
            "uuid",     // Standard COM IIDs (IID_IUnknown, etc.)
            "mfuuid",   // Media Foundation IIDs (IID_IMFTransform, IID_ICodecAPI, etc.)
            "mfplat",   // Media Foundation platform APIs
            "secur32",  // SSPI (pulled in by some network components)
            "bcrypt",   // Crypto primitives
            "user32",   // Window management (avdevice needs it)
        ] {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
}

/// `clang -print-file-name=libclang_rt.osx.a` names the availability
/// runtime for the clang in use (Apple's under Xcode, or a Homebrew
/// LLVM); its directory becomes a link search path.
fn link_compiler_rt() {
    let clang = std::env::var("CC").unwrap_or_else(|_| "clang".to_string());
    let out = std::process::Command::new(&clang)
        .arg("-print-file-name=libclang_rt.osx.a")
        .output();
    let Ok(out) = out else {
        println!("cargo:warning=could not run {clang} to locate libclang_rt.osx.a");
        return;
    };
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let path = std::path::Path::new(&path);
    match path.parent() {
        Some(dir) if path.is_file() => {
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-lib=static=clang_rt.osx");
        }
        _ => println!(
            "cargo:warning=libclang_rt.osx.a not found via {clang}; \
             availability checks in FFmpeg will fail to link if any are emitted"
        ),
    }
}
