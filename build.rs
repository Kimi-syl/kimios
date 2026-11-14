//! Builds the EL0 `kimi-init` program and copies its ELF into OUT_DIR so the
//! kernel can embed it in RamFS with `include_bytes!`.
//!
//! The nested cargo invocation uses a separate target directory to avoid
//! fighting the outer build over the target lock.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let user_dir = manifest_dir.join("user");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=user/Cargo.toml");
    println!("cargo:rerun-if-changed=user/linker.ld");
    println!("cargo:rerun-if-changed=user/src/main.rs");

    let target_dir = manifest_dir.join("target/user-build");
    let linker = user_dir.join("linker.ld");
    let rustflags = format!(
        "-C linker=rust-lld -C linker-flavor=ld.lld -C link-arg=-T{} -C link-arg=-zmax-page-size=4096 -C relocation-model=static",
        linker.display()
    );

    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(cargo)
        .current_dir(&user_dir)
        .env("RUSTFLAGS", rustflags)
        .arg("build")
        .arg("--release")
        .arg("--target")
        .arg("aarch64-unknown-none")
        .arg("--target-dir")
        .arg(&target_dir)
        .status()
        .expect("failed to spawn cargo for the user program");

    assert!(status.success(), "user program build failed");

    let built = target_dir.join("aarch64-unknown-none/release/kimi-init");
    let destination = out_dir.join("init.elf");
    fs::copy(&built, &destination).expect("failed to copy user ELF into OUT_DIR");
}
