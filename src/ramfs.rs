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

static HELLO_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/hello.elf"));

static HELLO_PE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/hello.exe"));

static FSTEST_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fstest.elf"));

static SPIN_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spin.elf"));

static FBTEST_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fbtest.elf"));

static HEAPTEST_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/heaptest.elf"));

static SLEEPER_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sleeper.elf"));

static ALLOCTEST_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/alloctest.elf"));

static GFXTEST_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gfxtest.elf"));

static INPUTTEST_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/inputtest.elf"));

static GUI_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gui.elf"));

static FAULTTEST_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/faulttest.elf"));

static BADPTR_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/badptr.elf"));

static STRESS_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/stress.elf"));

static FILES: [File; 16] = [
    File {
        name: "/init",
        data: INIT_ELF,
    },
    File {
        name: "/hello",
        data: HELLO_ELF,
    },
    File {
        name: "/hello.exe",
        data: HELLO_PE,
    },
    File {
        name: "/fstest",
        data: FSTEST_ELF,
    },
    File {
        name: "/spin",
        data: SPIN_ELF,
    },
    File {
        name: "/fbtest",
        data: FBTEST_ELF,
    },
    File {
        name: "/heaptest",
        data: HEAPTEST_ELF,
    },
    File {
        name: "/sleeper",
        data: SLEEPER_ELF,
    },
    File {
        name: "/alloctest",
        data: ALLOCTEST_ELF,
    },
    File {
        name: "/gfxtest",
        data: GFXTEST_ELF,
    },
    File {
        name: "/inputtest",
        data: INPUTTEST_ELF,
    },
    File {
        name: "/gui",
        data: GUI_ELF,
    },
    File {
        name: "/faulttest",
        data: FAULTTEST_ELF,
    },
    File {
        name: "/badptr",
        data: BADPTR_ELF,
    },
    File {
        name: "/stress",
        data: STRESS_ELF,
    },
    File {
        name: "/motd",
        data: b"Welcome to KimiOS!\nThis file lives in RamFS.\n",
    },
];

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

pub fn index_of(path: &str) -> Option<usize> {
    FILES.iter().position(|file| file.name == path)
}

pub fn file_at(index: usize) -> Option<&'static File> {
    FILES.get(index)
}

pub fn files() -> impl Iterator<Item = &'static File> {
    FILES.iter()
}
