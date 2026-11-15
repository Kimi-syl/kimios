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
