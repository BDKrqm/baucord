// rustc-shim: RUSTC_WRAPPER that delegates to sccache, EXCEPT when cargo
// falls back to an @argfile because the rustc command line exceeded the
// Windows 32,767-char CreateProcess limit (e.g. ffmpeg-sys-next emits ~680
// --cfg flags -> ~40K chars).
//
// Why: sccache 0.17.0 can *parse* @argfiles (PR #2782), but it still
// re-spawns rustc three times (--emit dep-info, --print file-names, and the
// compile itself) with all arguments flattened back onto the command line
// (see get_source_files_and_env_deps / get_compiler_outputs /
// generate_compile_commands in src/compiler/rust.rs). On Windows that dies
// with "The filename or extension is too long. (os error 206)".
//
// This shim routes exactly those invocations straight to rustc, which
// supports @argfile natively. Everything else goes through sccache as usual,
// so only the over-long crates (here: ffmpeg-sys-next) skip the cache.
//
// Build: rustc -O rustc-shim.rs -o rustc-shim.exe
// Use:   RUSTC_WRAPPER=<path>\rustc-shim.exe  (sccache must be on PATH or
//        SCCACHE_PATH must point to it — mozilla-actions/sccache-action
//        sets SCCACHE_PATH for you.)

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::process::{exit, Command};

fn run(mut cmd: Command) -> ! {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("rustc-shim: failed to spawn {:?}: {e}", cmd.get_program()));
    exit(status.code().unwrap_or(1));
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();

    // Normal case: `shim <rustc> <args...>` — delegate to sccache unchanged.
    if !args.iter().any(|a| a.starts_with('@')) {
        let sccache = env::var("SCCACHE_PATH").unwrap_or_else(|_| "sccache".into());
        let mut cmd = Command::new(sccache);
        cmd.args(&args);
        run(cmd);
    }

    eprintln!("rustc-shim: @argfile detected, bypassing sccache for this crate");

    // Defensive: `shim <rustc> ... @file ...` — rustc expands @file itself.
    if !args[0].starts_with('@') {
        let mut cmd = Command::new(&args[0]);
        cmd.args(&args[1..]);
        run(cmd);
    }

    // Cargo's actual shape (crates/cargo-util/src/process_builder.rs,
    // build_command_with_argfile): the shim receives a single `@file` arg and
    // the file's FIRST line is the rustc path, followed by one argument per
    // line. Rewrite the file without line 1 and hand it to rustc.
    let path = &args[0][1..];
    let content =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("rustc-shim: cannot read {path}: {e}"));
    let mut lines = content.lines();
    let rustc = lines
        .next()
        .expect("rustc-shim: argfile is empty")
        .trim_end_matches('\r')
        .to_owned();

    let new_file = env::temp_dir().join(format!("rustc-shim-{}.args", std::process::id()));
    {
        let mut f = fs::File::create(&new_file)
            .unwrap_or_else(|e| panic!("rustc-shim: cannot create {}: {e}", new_file.display()));
        for l in lines {
            writeln!(f, "{l}").expect("rustc-shim: write failed");
        }
    }

    let mut at = OsString::from("@");
    at.push(&new_file);
    let status = Command::new(&rustc)
        .arg(at)
        .status()
        .unwrap_or_else(|e| panic!("rustc-shim: failed to spawn {rustc}: {e}"));
    let _ = fs::remove_file(&new_file);
    exit(status.code().unwrap_or(1));
}
