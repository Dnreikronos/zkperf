use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-env-changed=RUSTC");
    let rustc = env::var_os("RUSTC").and_then(|compiler| {
        let output = Command::new(compiler).arg("--version").output().ok()?;
        output.status.success().then_some(output.stdout)
    });
    if let Some(version) = rustc.and_then(|bytes| String::from_utf8(bytes).ok()) {
        println!("cargo:rustc-env=ZKPERF_BUILD_RUSTC={}", version.trim());
    }
    for name in ["TARGET", "PROFILE", "OPT_LEVEL", "DEBUG"] {
        if let Ok(value) = env::var(name) {
            println!("cargo:rustc-env=ZKPERF_BUILD_{name}={value}");
        }
    }
}
