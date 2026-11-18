//! ELF64 loader for user programs.
//!
//! Each `PT_LOAD` segment is loaded into freshly allocated 4 KiB physical
//! frames. Frames are zeroed first (covering .bss), the file-backed part is
//! copied in, and every page is mapped into the user address space with
//! EL0 permissions. Segments in `user/linker.ld` are page-aligned so pages are
//! never shared between segments.

use xmas_elf::program::Type;
use xmas_elf::ElfFile;

use crate::{allocator, mmu, println};

const PAGE_SIZE: u64 = 4096;

/// User stack lives in its own 2 MiB block above the image.
const USER_STACK_VA: u64 = 0x6100_0000;
const USER_STACK_PAGES: u64 = 16;

pub struct LoadedImage {
    pub entry: u64,
    pub stack_top: u64,
}

pub fn load(
    image: &[u8],
    space: &mut mmu::AddressSpace,
) -> Result<LoadedImage, &'static str> {
    // xmas-elf/zero requires the ELF header to be naturally aligned, but
    // include_bytes! only guarantees byte alignment. Parse from an aligned
    // copy instead; the copy is returned to the kernel heap afterwards.
    let layout = core::alloc::Layout::from_size_align(image.len(), 16)
        .map_err(|_| "invalid ELF size")?;
    let aligned = allocator::alloc(layout).ok_or("out of memory for ELF buffer")?;
    let buffer = unsafe {
        core::ptr::copy_nonoverlapping(image.as_ptr(), aligned.as_ptr(), image.len());
        core::slice::from_raw_parts(aligned.as_ptr(), image.len())
    };

    let result = load_aligned(buffer, space);
    allocator::dealloc(aligned);
    result
}

fn load_aligned(
    image: &[u8],
    space: &mut mmu::AddressSpace,
) -> Result<LoadedImage, &'static str> {
    let elf = ElfFile::new(image).map_err(|_| "not a valid ELF file")?;
    let entry = elf.header.pt2.entry_point();

    for program in elf.program_iter() {
        if program.get_type() != Ok(Type::Load) {
            continue;
        }

        let file_offset = program.offset() as usize;
        let virtual_address = program.virtual_addr();
        let file_size = program.file_size() as usize;
        let memory_size = program.mem_size() as usize;
        let executable = program.flags().is_execute();

        if memory_size == 0 {
            continue;
        }

        let file_end = file_offset
            .checked_add(file_size)
            .ok_or("segment file range overflow")?;
        if file_end > image.len() {
            return Err("segment extends past end of file");
        }
        if virtual_address == 0 {
            return Err("segment mapped at virtual address 0");
        }

        println!(
            "  PT_LOAD va={:#010x} filesz={} memsz={} flags={}",
            virtual_address,
            file_size,
            memory_size,
            program.flags()
        );

        let first_page = virtual_address & !(PAGE_SIZE - 1);
        let last_page = (virtual_address + memory_size as u64 + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        let mut page = first_page;
        while page < last_page {
            let frame = space.alloc_frame().ok_or("out of physical memory")?;

            // SAFETY: identity-mapped RAM, one page, reserved by the PFA.
            unsafe {
                core::ptr::write_bytes(frame as *mut u8, 0, PAGE_SIZE as usize);
            }

            let copy_start = page.max(virtual_address);
            let copy_end = (page + PAGE_SIZE).min(virtual_address + file_size as u64);
            if copy_start < copy_end {
                let source_offset = file_offset + (copy_start - virtual_address) as usize;
                let destination_offset = (copy_start - page) as usize;
                let count = (copy_end - copy_start) as usize;

                // SAFETY: bounds were checked above; destination is a fresh frame.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        image.as_ptr().add(source_offset),
                        (frame as *mut u8).add(destination_offset),
                        count,
                    );
                }
            }

            space.map_page(page, frame, executable);
            page += PAGE_SIZE;
        }
    }

    for index in 0..USER_STACK_PAGES {
        let stack_frame = space
            .alloc_frame()
            .ok_or("out of physical memory for stack")?;
        unsafe {
            core::ptr::write_bytes(stack_frame as *mut u8, 0, PAGE_SIZE as usize);
        }
        space.map_page(USER_STACK_VA + index * PAGE_SIZE, stack_frame, false);
    }

    Ok(LoadedImage {
        entry,
        stack_top: USER_STACK_VA + USER_STACK_PAGES * PAGE_SIZE,
    })
}

// ---------------------------------------------------------------------------
// Linux ABI: load a standard statically-linked AArch64 Linux ELF (musl/glibc)
// at its own link address (typically 0x40_0000) and build a Linux initial
// stack (argc/argv/envp/auxv). See src/linux.rs for syscall translation.
// ---------------------------------------------------------------------------

/// Linux user stack (high RAM, away from the low-linked image and heap).
const LINUX_STACK_VA: u64 = 0x7000_0000;
const LINUX_STACK_PAGES: u64 = 32;

pub struct LinuxImage {
    pub entry: u64,
    pub sp: u64,
    pub brk: u64,
}

/// Detect a standard static Linux ELF: ET_EXEC (non-PIE) whose lowest PT_LOAD
/// is linked below the KimiOS user base (0x6000_0000). KimiOS images link at
/// 0x6000_0000; musl/glibc static binaries link at 0x40_0000.
pub fn is_linux(image: &[u8]) -> bool {
    if image.len() < 64 || &image[..4] != b"\x7fELF" || image[4] != 2 {
        return false; // ELF64 only
    }
    let e_type = u16::from_le_bytes([image[16], image[17]]);
    if e_type == 3 {
        return true; // ET_DYN: PIE / dynamic Linux ELF (musl interpreter)
    }
    if e_type != 2 {
        return false; // ET_EXEC only besides PIE
    }
    let e_phoff = u64::from_le_bytes(image[32..40].try_into().unwrap());
    let e_phentsize = u16::from_le_bytes([image[54], image[55]]) as u64;
    let e_phnum = u16::from_le_bytes([image[56], image[57]]) as u64;
    let mut min_vaddr = u64::MAX;
    for i in 0..e_phnum {
        let off = (e_phoff + i * e_phentsize) as usize;
        if off + 56 > image.len() {
            break;
        }
        let p_type = u32::from_le_bytes(image[off..off + 4].try_into().unwrap());
        if p_type != 1 {
            continue; // PT_LOAD
        }
        let p_vaddr = u64::from_le_bytes(image[off + 16..off + 24].try_into().unwrap());
        min_vaddr = min_vaddr.min(p_vaddr);
    }
    min_vaddr < 0x6000_0000
}

pub fn load_linux(
    image: &[u8],
    space: &mut mmu::AddressSpace,
    argv: &[&str],
) -> Result<LinuxImage, &'static str> {
    let layout = core::alloc::Layout::from_size_align(image.len(), 16)
        .map_err(|_| "invalid ELF size")?;
    let aligned = allocator::alloc(layout).ok_or("out of memory for ELF buffer")?;
    let buffer = unsafe {
        core::ptr::copy_nonoverlapping(image.as_ptr(), aligned.as_ptr(), image.len());
        core::slice::from_raw_parts(aligned.as_ptr(), image.len())
    };
    let result = load_linux_aligned(buffer, space, argv);
    allocator::dealloc(aligned);
    result
}

fn load_linux_aligned(
    image: &[u8],
    space: &mut mmu::AddressSpace,
    argv: &[&str],
) -> Result<LinuxImage, &'static str> {
    let elf = ElfFile::new(image).map_err(|_| "not a valid ELF file")?;
    if u16::from_le_bytes([image[16], image[17]]) == 3 {
        return load_linux_dynamic(image, space, argv);
    }
    let entry = elf.header.pt2.entry_point();
    let phoff = elf.header.pt2.ph_offset();
    let phentsize = elf.header.pt2.ph_entry_size() as u64;
    let phnum = elf.header.pt2.ph_count() as u64;

    let mut data_end = 0u64;
    let mut phdr_va = 0u64;
    let mut phdr_found = false;

    for program in elf.program_iter() {
        if program.get_type() != Ok(Type::Load) {
            continue;
        }
        let file_offset = program.offset();
        let virtual_address = program.virtual_addr();
        let file_size = program.file_size() as usize;
        let memory_size = program.mem_size() as usize;
        let executable = program.flags().is_execute();
        if memory_size == 0 {
            continue;
        }
        let file_end = file_offset
            .checked_add(file_size as u64)
            .ok_or("segment file range overflow")?;
        if file_end > image.len() as u64 {
            return Err("segment extends past end of file");
        }
        if virtual_address == 0 {
            return Err("PIE/zero-linked Linux ELF not supported");
        }

        // The program headers live inside the first PT_LOAD (file offset 0).
        if !phdr_found && file_offset <= phoff && phoff < file_offset + file_size as u64 {
            phdr_va = virtual_address + (phoff - file_offset);
            phdr_found = true;
        }
        data_end = data_end.max(virtual_address + memory_size as u64);

        let first_page = virtual_address & !(PAGE_SIZE - 1);
        let last_page = (virtual_address + memory_size as u64 + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let mut page = first_page;
        while page < last_page {
            let frame = space.alloc_frame().ok_or("out of physical memory")?;
            unsafe {
                core::ptr::write_bytes(frame as *mut u8, 0, PAGE_SIZE as usize);
            }
            let copy_start = page.max(virtual_address);
            let copy_end = (page + PAGE_SIZE).min(virtual_address + file_size as u64);
            if copy_start < copy_end {
                let source_offset = (file_offset + (copy_start - virtual_address)) as usize;
                let destination_offset = (copy_start - page) as usize;
                let count = (copy_end - copy_start) as usize;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        image.as_ptr().add(source_offset),
                        (frame as *mut u8).add(destination_offset),
                        count,
                    );
                }
            }
            space.map_page(page, frame, executable);
            page += PAGE_SIZE;
        }
    }

    for index in 0..LINUX_STACK_PAGES {
        let frame = space
            .alloc_frame()
            .ok_or("out of physical memory for stack")?;
        unsafe {
            core::ptr::write_bytes(frame as *mut u8, 0, PAGE_SIZE as usize);
        }
        space.map_page(LINUX_STACK_VA + index * PAGE_SIZE, frame, false);
    }

    let stack_top = LINUX_STACK_VA + LINUX_STACK_PAGES * PAGE_SIZE;
    let sp = build_linux_stack(space, stack_top, phdr_va, phentsize, phnum, entry, 0, argv);
    let brk = (data_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    println!(
        "  [linux] entry={:#x} sp={:#x} brk={:#x} phdr={:#x}",
        entry, sp, brk, phdr_va
    );
    Ok(LinuxImage { entry, sp, brk })
}

/// Build argc/argv/envp/auxv + strings at the top of the stack and return the
/// initial SP (points at argc, 16-byte aligned). Layout (low -> high):
/// argc, argv[0..argc-1], NULL, envp NULL, auxv pairs..., AT_NULL, strings.
fn build_linux_stack(
    space: &mut mmu::AddressSpace,
    stack_top: u64,
    phdr_va: u64,
    phent: u64,
    phnum: u64,
    entry: u64,
    at_base: u64,
    argv: &[&str],
) -> u64 {
    let mut buf = [0u8; 4096];
    let mut cur = buf.len();
    let argc = argv.len().clamp(1, 16);

    // 16 random bytes (AT_RANDOM) at the very top.
    cur -= 16;
    let random_off = cur;
    for i in 0..16 {
        buf[cur + i] = (i as u8).wrapping_mul(7).wrapping_add(13);
    }
    let plat = b"aarch64\0";
    cur -= plat.len();
    buf[cur..cur + plat.len()].copy_from_slice(plat);

    // argv strings (NUL-terminated), recording each offset.
    let mut str_off = [0usize; 16];
    for i in 0..argc {
        let s = argv[i].as_bytes();
        if cur < s.len() + 1 {
            break;
        }
        cur -= s.len() + 1;
        str_off[i] = cur;
        buf[cur..cur + s.len()].copy_from_slice(s);
        buf[cur + s.len()] = 0;
    }
    // Align the structured region so argc lands 16-byte aligned for any argc:
    // its size is 288 + (argc+1)*8, which is 0 mod 16 for odd argc, 8 for even.
    cur &= !15;
    if argc % 2 == 0 {
        cur -= 8;
    }

    let base = stack_top - buf.len() as u64;
    let random_va = base + random_off as u64;
    let argv0_va = base + str_off[0] as u64;

    // auxv (AT_* pairs), terminated by AT_NULL.
    let auxv: [(u64, u64); 17] = [
        (3, phdr_va),    // AT_PHDR
        (4, phent),      // AT_PHENT
        (5, phnum),      // AT_PHNUM
        (6, PAGE_SIZE),  // AT_PAGESZ
        (7, at_base),    // AT_BASE (interpreter base; 0 for static)
        (8, 0),          // AT_FLAGS
        (9, entry),      // AT_ENTRY
        (11, 0),         // AT_UID
        (12, 0),         // AT_EUID
        (13, 0),         // AT_GID
        (14, 0),         // AT_EGID
        (16, 3),         // AT_HWCAP (FP | ASIMD)
        (17, 100),       // AT_CLKTCK
        (23, 0),         // AT_SECURE
        (25, random_va), // AT_RANDOM
        (31, argv0_va),  // AT_EXECFN
        (0, 0),          // AT_NULL
    ];
    cur -= auxv.len() * 16;
    let auxv_off = cur;
    for (i, (t, v)) in auxv.iter().enumerate() {
        let o = auxv_off + i * 16;
        buf[o..o + 8].copy_from_slice(&t.to_le_bytes());
        buf[o + 8..o + 16].copy_from_slice(&v.to_le_bytes());
    }

    // envp: empty environment (single NULL terminator).
    cur -= 8;
    // argv: [argv0 .. argv(argc-1), NULL] (buf is zeroed, so NULL is implicit).
    cur -= (argc + 1) * 8;
    let argv_off = cur;
    for i in 0..argc {
        let va = base + str_off[i] as u64;
        buf[argv_off + i * 8..argv_off + i * 8 + 8].copy_from_slice(&va.to_le_bytes());
    }
    // argc.
    cur -= 8;
    let argc_off = cur;
    buf[cur..cur + 8].copy_from_slice(&(argc as u64).to_le_bytes());

    let sp = base + argc_off as u64;
    space.write_bytes(sp, &buf[cur..]);
    sp
}

// ---------------------------------------------------------------------------
// Dynamic (PIE + musl interpreter) Linux loader.
//
// The kernel maps the executable and its PT_INTERP (musl ldso / libc.so) at
// fixed bases, builds an auxv with AT_PHDR/AT_BASE/AT_ENTRY, and jumps to the
// interpreter entry. musl's ldso then self-relocates, loads DT_NEEDED libraries
// and relocates the executable using syscalls (mmap/mprotect/openat/…).
// ---------------------------------------------------------------------------

const LINUX_EXEC_BASE: u64 = 0x0040_0000;
const LINUX_INTERP_BASE: u64 = 0x6000_0000;

/// Load PT_LOAD segments of `elf` at `base`, returning
/// (entry_rel, phdr_va_abs, data_end_abs, phent, phnum).
fn load_segments(
    elf: &ElfFile,
    image: &[u8],
    space: &mut mmu::AddressSpace,
    base: u64,
) -> Result<(u64, u64, u64, u64, u64), &'static str> {
    let entry = elf.header.pt2.entry_point();
    let phoff = elf.header.pt2.ph_offset();
    let phent = elf.header.pt2.ph_entry_size() as u64;
    let phnum = elf.header.pt2.ph_count() as u64;
    let mut data_end = 0u64;
    let mut phdr_va = 0u64;
    let mut found = false;
    for program in elf.program_iter() {
        if program.get_type() != Ok(Type::Load) {
            continue;
        }
        let off = program.offset();
        let va = program.virtual_addr();
        let fsz = program.file_size() as usize;
        let msz = program.mem_size() as usize;
        let exe = program.flags().is_execute();
        if msz == 0 {
            continue;
        }
        if off + fsz as u64 > image.len() as u64 {
            return Err("segment extends past end of file");
        }
        let hva = base + va;
        if !found && off <= phoff && phoff < off + fsz as u64 {
            phdr_va = hva + (phoff - off);
            found = true;
        }
        data_end = data_end.max(hva + msz as u64);
        let first = hva & !(PAGE_SIZE - 1);
        let last = (hva + msz as u64 + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let mut page = first;
        while page < last {
            let frame = space.alloc_frame().ok_or("out of physical memory")?;
            unsafe {
                core::ptr::write_bytes(frame as *mut u8, 0, PAGE_SIZE as usize);
            }
            let cs = page.max(hva);
            let ce = (page + PAGE_SIZE).min(hva + fsz as u64);
            if cs < ce {
                let so = (off + (cs - hva)) as usize;
                let doff = (cs - page) as usize;
                let cnt = (ce - cs) as usize;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        image.as_ptr().add(so),
                        (frame as *mut u8).add(doff),
                        cnt,
                    );
                }
            }
            space.map_page(page, frame, exe);
            page += PAGE_SIZE;
        }
    }
    Ok((entry, phdr_va, data_end, phent, phnum))
}

/// Read a Linux absolute path (e.g. "/lib/ld-musl-aarch64.so.1") from RamFS or
/// the Alpine sysroot at /disk/alpine/root. Returns a static slice (RamFS data
/// is static; sysroot files are read into a leaked kernel-heap buffer).
fn read_linux_file(path: &str) -> Result<&'static [u8], &'static str> {
    if let Some(f) = crate::ramfs::index_of(path).and_then(crate::ramfs::file_at) {
        return Ok(f.data);
    }
    // sysroot fallback: alpine/root + path
    let mut sb = [0u8; 160];
    let prefix = b"alpine/root";
    sb[..prefix.len()].copy_from_slice(prefix);
    let mut n = prefix.len();
    let pb = path.as_bytes();
    let take = pb.len().min(sb.len() - n - 1);
    sb[n..n + take].copy_from_slice(&pb[..take]);
    n += take;
    let sp = core::str::from_utf8(&sb[..n]).map_err(|_| "bad sysroot path")?;
    let inode = crate::diskfs::lookup(sp).ok_or("interpreter not found")?;
    let (_, size, _) = crate::diskfs::stat(sp).ok_or("interp stat failed")?;
    let size = size as usize;
    let layout = core::alloc::Layout::from_size_align(size.max(1), 16).map_err(|_| "bad size")?;
    let buf = crate::allocator::alloc(layout).ok_or("oom for interpreter")?;
    let slice = unsafe { core::slice::from_raw_parts_mut(buf.as_ptr(), size) };
    let got = crate::diskfs::read(inode, 0, slice);
    if got < size {
        return Err("short interpreter read");
    }
    Ok(unsafe { core::slice::from_raw_parts(buf.as_ptr(), size) })
}

fn load_linux_dynamic(
    image: &[u8],
    space: &mut mmu::AddressSpace,
    argv: &[&str],
) -> Result<LinuxImage, &'static str> {
    let elf = ElfFile::new(image).map_err(|_| "not a valid ELF file")?;
    let (entry_rel, phdr_va, data_end, phent, phnum) =
        load_segments(&elf, image, space, LINUX_EXEC_BASE)?;

    // PT_INTERP -> interpreter path
    let mut interp_path = "";
    for program in elf.program_iter() {
        if program.get_type() == Ok(Type::Interp) {
            let off = program.offset() as usize;
            let fsz = program.file_size() as usize;
            if off + fsz > image.len() {
                return Err("bad interp segment");
            }
            let end = image[off..off + fsz].iter().position(|&b| b == 0).unwrap_or(fsz);
            interp_path = core::str::from_utf8(&image[off..off + end]).map_err(|_| "bad interp path")?;
            break;
        }
    }
    if interp_path.is_empty() {
        return Err("PIE without interpreter (static-PIE unsupported)");
    }
    let interp_raw = read_linux_file(interp_path)?;
    // xmas-elf/zero requires natural alignment; RamFS data is byte-aligned.
    let ilayout = core::alloc::Layout::from_size_align(interp_raw.len(), 16)
        .map_err(|_| "bad interpreter size")?;
    let iali = crate::allocator::alloc(ilayout).ok_or("oom for interpreter buffer")?;
    let interp_image = unsafe {
        core::ptr::copy_nonoverlapping(interp_raw.as_ptr(), iali.as_ptr(), interp_raw.len());
        core::slice::from_raw_parts(iali.as_ptr(), interp_raw.len())
    };
    let interp_elf = ElfFile::new(interp_image).map_err(|_| "bad interpreter ELF")?;
    let (ientry_rel, _, _, _, _) = load_segments(&interp_elf, interp_image, space, LINUX_INTERP_BASE)?;
    unsafe { crate::allocator::dealloc(iali); }

    for index in 0..LINUX_STACK_PAGES {
        let frame = space.alloc_frame().ok_or("out of memory for stack")?;
        unsafe {
            core::ptr::write_bytes(frame as *mut u8, 0, PAGE_SIZE as usize);
        }
        space.map_page(LINUX_STACK_VA + index * PAGE_SIZE, frame, false);
    }
    let stack_top = LINUX_STACK_VA + LINUX_STACK_PAGES * PAGE_SIZE;
    let exec_entry = LINUX_EXEC_BASE + entry_rel;
    let sp = build_linux_stack(space, stack_top, phdr_va, phent, phnum, exec_entry, LINUX_INTERP_BASE, argv);
    let brk = (data_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    println!(
        "  [linux-dyn] interp={} exec_base={:#x} interp_base={:#x} entry={:#x} sp={:#x}",
        interp_path, LINUX_EXEC_BASE, LINUX_INTERP_BASE, LINUX_INTERP_BASE + ientry_rel, sp
    );
    Ok(LinuxImage { entry: LINUX_INTERP_BASE + ientry_rel, sp, brk })
}
