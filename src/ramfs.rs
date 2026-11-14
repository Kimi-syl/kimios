//! A tiny read-only in-memory filesystem and the VFS interface.
//!
//! User binaries are embedded in the kernel image at build time by build.rs /
//! `include_bytes!`, which keeps Phase 10 dependency-free at runtime.

pub struct File {
    pub name: &'static str,
    pub data: &'static [u8],
}

#[allow(dead_code)] // The full VFS surface is defined ahead of the shell.
pub trait Vfs {
    fn open(&self, path: &str) -> Option<&'static File>;

    fn read(&self, file: &File, offset: usize, buffer: &mut [u8]) -> usize;

    fn write(&self, _file: &File, _offset: usize, _buffer: &[u8]) -> usize {
        0
    }

    fn close(&self, _file: &File) {}
}

static INIT_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/init.elf"));

static FILES: [File; 1] = [File {
    name: "/init",
    data: INIT_ELF,
}];

pub struct RamFs;

impl Vfs for RamFs {
    fn open(&self, path: &str) -> Option<&'static File> {
        FILES.iter().find(|file| file.name == path)
    }

    fn read(&self, file: &File, offset: usize, buffer: &mut [u8]) -> usize {
        if offset >= file.data.len() {
            return 0;
        }

        let count = core::cmp::min(buffer.len(), file.data.len() - offset);
        buffer[..count].copy_from_slice(&file.data[offset..offset + count]);
        count
    }
}

pub static RAMFS: RamFs = RamFs;

/// Convenience for diagnostics and the future shell.
#[allow(dead_code)]
pub fn list() -> impl Iterator<Item = &'static File> {
    FILES.iter()
}
