//! Bitmap-based physical frame allocator (PFA).
//!
//! RAM is described by the device tree; KimiOS hands the first usable region
//! to this allocator. Frames are 4 KiB and numbered relative to the region
//! base. One bit per frame: 1 = used, 0 = free.
//!
//! The bitmap is a static array, so it lives inside the kernel image and is
//! therefore already reserved. Everything below `__kernel_end` (boot area,
//! DTB, kernel .text/.rodata/.data/.bss, heap and boot stack) is reserved.

use core::ptr::addr_of;
use spin::Mutex;

use crate::dtb::MemoryRegion;

pub const PAGE_SIZE: usize = 4096;

/// Support up to 1 GiB of managed RAM: 262144 frames -> 32 KiB of bitmap.
const MAX_PAGES: usize = (1024 * 1024 * 1024) / PAGE_SIZE;
const BITMAP_WORDS: usize = MAX_PAGES / 64;

extern "C" {
    static __kernel_end: u8;
}

struct Pfa {
    base: u64,
    pages: usize,
    free: usize,
    bitmap: [u64; BITMAP_WORDS],
}

impl Pfa {
    const fn new() -> Self {
        Self {
            base: 0,
            pages: 0,
            free: 0,
            bitmap: [u64::MAX; BITMAP_WORDS],
        }
    }

    fn is_used(&self, index: usize) -> bool {
        self.bitmap[index / 64] & (1 << (index % 64)) != 0
    }

    fn set_used(&mut self, index: usize) {
        self.bitmap[index / 64] |= 1 << (index % 64);
    }

    fn set_free(&mut self, index: usize) {
        self.bitmap[index / 64] &= !(1 << (index % 64));
    }

    fn reserve(&mut self, start: u64, end: u64) {
        let region_start = self.base;
        let region_end = self.base + (self.pages as u64) * PAGE_SIZE as u64;

        let start = start.max(region_start);
        let end = end.min(region_end);
        if start >= end {
            return;
        }

        let first = ((start - region_start) / PAGE_SIZE as u64) as usize;
        let last = ((end - region_start + PAGE_SIZE as u64 - 1) / PAGE_SIZE as u64) as usize;

        for index in first..last.min(self.pages) {
            if !self.is_used(index) {
                self.set_used(index);
                self.free -= 1;
            }
        }
    }

    /// Mark all frames used, then free the usable region and re-reserve the
    /// kernel image and DTB.
    fn init(&mut self, memory: MemoryRegion, dtb: Option<(u64, u64)>) {
        let base = (memory.base + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
        let end = (memory.base + memory.size) & !(PAGE_SIZE as u64 - 1);

        self.base = base;
        self.pages = core::cmp::min(((end - base) / PAGE_SIZE as u64) as usize, MAX_PAGES);
        self.bitmap.fill(u64::MAX);
        self.free = 0;

        for index in 0..self.pages {
            self.set_free(index);
            self.free += 1;
        }

        // Reserve everything below the end of the kernel image: the boot
        // area, DTB (if any), kernel .text/.rodata/.data/.bss, heap, boot
        // stack and the PFA bitmap itself.
        let kernel_end = addr_of!(__kernel_end) as u64;
        self.reserve(self.base, kernel_end);

        if let Some((start, size)) = dtb {
            self.reserve(start, start + size);
        }
    }

    fn alloc(&mut self) -> Option<u64> {
        if self.free == 0 {
            return None;
        }

        for (word_index, word) in self.bitmap.iter_mut().enumerate() {
            if *word != u64::MAX {
                let bit = (!*word).trailing_zeros() as usize;
                let index = word_index * 64 + bit;
                if index >= self.pages {
                    break;
                }
                *word |= 1 << bit;
                self.free -= 1;
                return Some(self.base + (index * PAGE_SIZE) as u64);
            }
        }
        None
    }

    fn free_frame(&mut self, address: u64) -> bool {
        if address < self.base || (address & (PAGE_SIZE as u64 - 1)) != 0 {
            return false;
        }

        let index = ((address - self.base) / PAGE_SIZE as u64) as usize;
        if index >= self.pages || !self.is_used(index) {
            return false;
        }

        self.set_free(index);
        self.free += 1;
        true
    }
}

static PFA: Mutex<Pfa> = Mutex::new(Pfa::new());

pub fn init(memory: MemoryRegion, dtb: Option<(u64, u64)>) {
    PFA.lock().init(memory, dtb);
}

pub fn alloc_frame() -> Option<u64> {
    PFA.lock().alloc()
}

pub fn free_frame(address: u64) -> bool {
    PFA.lock().free_frame(address)
}

/// Returns `(free_frames, total_frames)`.
pub fn stats() -> (usize, usize) {
    let pfa = PFA.lock();
    (pfa.free, pfa.pages)
}
