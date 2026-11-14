//! Builds the EL0 workspace (`kimi-init`, `kimi-hello`, `kimi-fstest`,
//! `kimi-spin`, `kimi-fbtest`), copies
//! the ELFs into OUT_DIR, and generates a tiny AArch64 PE image for the WinAPI translation
//! test. The kernel embeds these files into RamFS with `include_bytes!`.
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
    println!("cargo:rerun-if-changed=user/libkimi");
    println!("cargo:rerun-if-changed=user/kimi-init");
    println!("cargo:rerun-if-changed=user/kimi-hello");
    println!("cargo:rerun-if-changed=user/kimi-fstest");
    println!("cargo:rerun-if-changed=user/kimi-spin");
    println!("cargo:rerun-if-changed=user/kimi-fbtest");
    println!("cargo:rerun-if-changed=user/kimi-heaptest");
    println!("cargo:rerun-if-changed=user/kimi-sleeper");

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
        .arg("--workspace")
        .status()
        .expect("failed to spawn cargo for the user program");

    assert!(status.success(), "user program build failed");

    for (binary, embedded) in [
        ("kimi-init", "init.elf"),
        ("kimi-hello", "hello.elf"),
        ("kimi-fstest", "fstest.elf"),
        ("kimi-spin", "spin.elf"),
        ("kimi-fbtest", "fbtest.elf"),
        ("kimi-heaptest", "heaptest.elf"),
        ("kimi-sleeper", "sleeper.elf"),
    ] {
        let built = target_dir
            .join("aarch64-unknown-none/release")
            .join(binary);
        let destination = out_dir.join(embedded);
        fs::copy(&built, &destination)
            .unwrap_or_else(|_| panic!("failed to copy {} into OUT_DIR", binary));
    }

    fs::write(out_dir.join("hello.exe"), build_pe())
        .expect("failed to write generated PE image");
}

const PE_IMAGE_BASE: u64 = 0x6200_0000;

fn put_u16(buffer: &mut [u8], offset: usize, value: u16) {
    buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(buffer: &mut [u8], offset: usize, value: u64) {
    buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn encode_movz(register: u32, immediate: u16) -> u32 {
    0xd280_0000 | ((immediate as u32) << 5) | register
}

fn encode_movk(register: u32, immediate: u16) -> u32 {
    0xf280_0000 | (1 << 21) | ((immediate as u32) << 5) | register
}

fn encode_movn(register: u32, immediate: u16) -> u32 {
    0x9280_0000 | ((immediate as u32) << 5) | register
}

fn encode_mov(register: u32, source: u32) -> u32 {
    0xaa00_03e0 | (source << 16) | register
}

fn encode_ldr(register: u32, base: u32) -> u32 {
    0xf940_0000 | (base << 5) | register
}

fn encode_blr(register: u32) -> u32 {
    0xd63f_0000 | (register << 5)
}

fn push_load_address(code: &mut Vec<u32>, register: u32, address: u64) {
    code.push(encode_movz(register, address as u16));
    if (address >> 16) != 0 {
        code.push(encode_movk(register, (address >> 16) as u16));
    }
    if (address >> 32) != 0 {
        code.push(encode_movk(register, (address >> 32) as u16));
    }
    if (address >> 48) != 0 {
        code.push(encode_movk(register, (address >> 48) as u16));
    }
}

fn build_pe() -> Vec<u8> {
    const MESSAGE1: &[u8] = b"Hello from an ARM64 PE on KimiOS!\n";
    const MESSAGE2: &[u8] = b"PE called WriteConsoleA and Sleep(50).\n";
    const MESSAGE3: &[u8] = b"PE is still alive after sleeping.\n";
    const SECTION_RVA: u32 = 0x1000;
    const SECTION_RAW: usize = 0x200;
    const MESSAGE1_OFF: usize = 0x200;
    const MESSAGE2_OFF: usize = 0x230;
    const MESSAGE3_OFF: usize = 0x260;
    const DLL_OFF: usize = 0x290;
    const HINT_OFF: usize = 0x2a0;
    const ILT_OFF: usize = 0x330;
    const IAT_OFF: usize = 0x380;
    const IMPORT_OFF: usize = 0x3d0;
    const SECTION_SIZE: usize = 0x400;

    let mut content = vec![0u8; SECTION_SIZE];

    // Code: GetStdHandle -> WriteFile -> Sleep -> WriteConsoleA ->
    // GetTickCount64 -> Sleep -> WriteConsoleA -> GetTickCount -> ExitProcess.
    let mut code: Vec<u32> = Vec::new();
    let iat = PE_IMAGE_BASE + SECTION_RVA as u64 + IAT_OFF as u64;
    let message1 = PE_IMAGE_BASE + SECTION_RVA as u64 + MESSAGE1_OFF as u64;
    let message2 = PE_IMAGE_BASE + SECTION_RVA as u64 + MESSAGE2_OFF as u64;
    let message3 = PE_IMAGE_BASE + SECTION_RVA as u64 + MESSAGE3_OFF as u64;

    // x19 = GetStdHandle(STD_OUTPUT_HANDLE = -11)
    push_load_address(&mut code, 16, iat);
    code.push(encode_ldr(16, 16));
    code.push(encode_movn(0, 10));
    code.push(encode_blr(16));
    code.push(encode_mov(19, 0));

    // WriteFile(handle, message1, len, NULL, NULL)
    push_load_address(&mut code, 16, iat + 8);
    code.push(encode_ldr(16, 16));
    code.push(encode_mov(0, 19));
    push_load_address(&mut code, 1, message1);
    code.push(encode_movz(2, MESSAGE1.len() as u16));
    code.push(encode_movz(3, 0));
    code.push(encode_movz(4, 0));
    code.push(encode_blr(16));

    // Sleep(50)
    push_load_address(&mut code, 16, iat + 40);
    code.push(encode_ldr(16, 16));
    code.push(encode_movz(0, 50));
    code.push(encode_blr(16));

    // WriteConsoleA(handle, message2, len, NULL, NULL)
    push_load_address(&mut code, 16, iat + 64);
    code.push(encode_ldr(16, 16));
    code.push(encode_mov(0, 19));
    push_load_address(&mut code, 1, message2);
    code.push(encode_movz(2, MESSAGE2.len() as u16));
    code.push(encode_movz(3, 0));
    code.push(encode_movz(4, 0));
    code.push(encode_blr(16));

    // GetTickCount64()
    push_load_address(&mut code, 16, iat + 48);
    code.push(encode_ldr(16, 16));
    code.push(encode_blr(16));

    // Sleep(100)
    push_load_address(&mut code, 16, iat + 40);
    code.push(encode_ldr(16, 16));
    code.push(encode_movz(0, 100));
    code.push(encode_blr(16));

    // WriteConsoleA(handle, message3, len, NULL, NULL)
    push_load_address(&mut code, 16, iat + 64);
    code.push(encode_ldr(16, 16));
    code.push(encode_mov(0, 19));
    push_load_address(&mut code, 1, message3);
    code.push(encode_movz(2, MESSAGE3.len() as u16));
    code.push(encode_movz(3, 0));
    code.push(encode_movz(4, 0));
    code.push(encode_blr(16));

    // GetTickCount()
    push_load_address(&mut code, 16, iat + 56);
    code.push(encode_ldr(16, 16));
    code.push(encode_blr(16));

    // ExitProcess(0)
    push_load_address(&mut code, 16, iat + 16);
    code.push(encode_ldr(16, 16));
    code.push(encode_movz(0, 0));
    code.push(encode_blr(16));
    code.push(0xd420_0000); // brk #0

    for (index, word) in code.iter().enumerate() {
        let offset = index * 4;
        content[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
    }

    content[MESSAGE1_OFF..MESSAGE1_OFF + MESSAGE1.len()].copy_from_slice(MESSAGE1);
    content[MESSAGE2_OFF..MESSAGE2_OFF + MESSAGE2.len()].copy_from_slice(MESSAGE2);
    content[MESSAGE3_OFF..MESSAGE3_OFF + MESSAGE3.len()].copy_from_slice(MESSAGE3);
    content[DLL_OFF..DLL_OFF + 13].copy_from_slice(b"kernel32.dll\0");

    let names = [
        "GetStdHandle",
        "WriteFile",
        "ExitProcess",
        "GetLastError",
        "SetLastError",
        "Sleep",
        "GetTickCount64",
        "GetTickCount",
        "WriteConsoleA",
    ];
    for (index, name) in names.iter().enumerate() {
        let offset = HINT_OFF + index * 16;
        content[offset + 2..offset + 2 + name.len()].copy_from_slice(name.as_bytes());
    }

    for index in 0..names.len() {
        let rva = (SECTION_RVA + HINT_OFF as u32 + index as u32 * 16) as u64;
        content[ILT_OFF + index * 8..ILT_OFF + index * 8 + 8]
            .copy_from_slice(&rva.to_le_bytes());
        content[IAT_OFF + index * 8..IAT_OFF + index * 8 + 8]
            .copy_from_slice(&rva.to_le_bytes());
    }

    put_u32(&mut content, IMPORT_OFF, SECTION_RVA + ILT_OFF as u32);
    put_u32(
        &mut content,
        IMPORT_OFF + 12,
        SECTION_RVA + DLL_OFF as u32,
    );
    put_u32(
        &mut content,
        IMPORT_OFF + 16,
        SECTION_RVA + IAT_OFF as u32,
    );

    let mut image = vec![0u8; SECTION_RAW + SECTION_SIZE];
    image[0..2].copy_from_slice(b"MZ");
    put_u32(&mut image, 0x3c, 0x40);
    image[0x40..0x44].copy_from_slice(b"PE\0\0");

    let coff = 0x44;
    put_u16(&mut image, coff, 0xaa64);
    put_u16(&mut image, coff + 2, 1);
    put_u16(&mut image, coff + 16, 0xf0);
    put_u16(&mut image, coff + 18, 0x22);

    let optional = coff + 20;
    put_u16(&mut image, optional, 0x20b);
    put_u32(&mut image, optional + 4, SECTION_SIZE as u32);
    put_u32(&mut image, optional + 16, SECTION_RVA);
    put_u32(&mut image, optional + 20, SECTION_RVA);
    put_u64(&mut image, optional + 24, PE_IMAGE_BASE);
    put_u32(&mut image, optional + 32, 0x1000);
    put_u32(&mut image, optional + 36, 0x200);
    put_u16(&mut image, optional + 40, 6);
    put_u16(&mut image, optional + 48, 6);
    put_u32(&mut image, optional + 56, 0x2000);
    put_u32(&mut image, optional + 60, 0x200);
    put_u16(&mut image, optional + 68, 3);
    put_u64(&mut image, optional + 72, 0x10_0000);
    put_u64(&mut image, optional + 80, 0x1000);
    put_u64(&mut image, optional + 88, 0x10_0000);
    put_u64(&mut image, optional + 96, 0x1000);
    put_u32(&mut image, optional + 108, 16);
    put_u32(
        &mut image,
        optional + 112 + 8,
        SECTION_RVA + IMPORT_OFF as u32,
    );
    put_u32(&mut image, optional + 112 + 12, 40);

    let section = optional + 0xf0;
    image[section..section + 8].copy_from_slice(b".text\0\0\0");
    put_u32(&mut image, section + 8, SECTION_SIZE as u32);
    put_u32(&mut image, section + 12, SECTION_RVA);
    put_u32(&mut image, section + 16, SECTION_SIZE as u32);
    put_u32(&mut image, section + 20, SECTION_RAW as u32);
    put_u32(&mut image, section + 36, 0xe000_0020);

    image[SECTION_RAW..SECTION_RAW + SECTION_SIZE].copy_from_slice(&content);
    image
}
