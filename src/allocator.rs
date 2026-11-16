//! A small first-fit heap over a static 4 MiB region.
//!
//! Blocks carry a 16-byte header (`size`, `next`) and the free list is kept
//! sorted by address so neighbours can merge on `dealloc`. This replaced the
//! original monotonic bump allocator, which leaked every ELF image copy on
//! `spawn` and eventually failed after a dozen process creations.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{addr_of_mut, NonNull};
use spin::Mutex;

/// Global allocator backed by the free-list heap below, so kernel code (and
/// dependencies such as smoltcp) can use `alloc` collections.
struct KernelAlloc;

unsafe impl GlobalAlloc for KernelAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match alloc(layout) {
            Some(pointer) => pointer.as_ptr(),
            None => core::ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
        if let Some(pointer) = NonNull::new(pointer) {
            dealloc(pointer);
        }
    }
}

#[global_allocator]
static GLOBAL: KernelAlloc = KernelAlloc;

const HEAP_SIZE: usize = 8 * 1024 * 1024; // 8 MiB
const HEADER: usize = 16;
const MIN_SPLIT: usize = HEADER + 16;

#[repr(align(16))]
#[allow(dead_code)] // The byte array is only accessed through its address.
struct Heap([u8; HEAP_SIZE]);

static mut HEAP: Heap = Heap([0; HEAP_SIZE]);

#[repr(C)]
struct Block {
    size: usize,
    next: *mut Block,
}

struct FreeList {
    head: *mut Block,
    used: usize,
}

// The free list is only touched with interrupts masked (boot and SVC paths).
unsafe impl Send for FreeList {}

static FREE: Mutex<FreeList> = Mutex::new(FreeList {
    head: core::ptr::null_mut(),
    used: 0,
});

fn heap_base() -> usize {
    addr_of_mut!(HEAP) as usize
}

fn align_up(value: usize, align: usize) -> Option<usize> {
    value.checked_add(align - 1).map(|sum| sum & !(align - 1))
}

/// Bytes currently reserved by callers (including block headers).
pub fn used() -> usize {
    FREE.lock().used
}

pub fn capacity() -> usize {
    HEAP_SIZE
}

/// Allocate `layout.size()` bytes with `layout.align()` alignment.
pub fn alloc(layout: Layout) -> Option<NonNull<u8>> {
    let size = layout.size().max(1);
    let align = layout.align().max(16);

    let mut list = FREE.lock();
    if list.head.is_null() {
        let base = heap_base();
        let block = base as *mut Block;
        unsafe {
            (*block).size = HEAP_SIZE;
            (*block).next = core::ptr::null_mut();
        }
        list.head = block;
    }

    let mut previous: *mut Block = core::ptr::null_mut();
    let mut current = list.head;
    while !current.is_null() {
        let start = current as usize;
        let payload = align_up(start + HEADER, align)?;
        let used = align_up(payload + size, 16)? - start;
        let total = unsafe { (*current).size };

        if used <= total {
            let next = unsafe { (*current).next };
            if previous.is_null() {
                list.head = next;
            } else {
                unsafe {
                    (*previous).next = next;
                }
            }

            let header = (payload - HEADER) as *mut Block;
            let block_size = start + used - header as usize;
            if total >= used + MIN_SPLIT {
                let rest = (start + used) as *mut Block;
                unsafe {
                    (*rest).size = total - used;
                    if previous.is_null() {
                        (*rest).next = list.head;
                        list.head = rest;
                    } else {
                        (*rest).next = (*previous).next;
                        (*previous).next = rest;
                    }
                }
            }
            unsafe {
                (*header).size = block_size;
                (*header).next = core::ptr::null_mut();
            }
            list.used += block_size;
            return NonNull::new(payload as *mut u8);
        }

        previous = current;
        current = unsafe { (*current).next };
    }
    None
}

/// Return a block previously handed out by `alloc`.
pub fn dealloc(pointer: NonNull<u8>) {
    let block = (pointer.as_ptr() as usize - HEADER) as *mut Block;
    let address = block as usize;

    let mut list = FREE.lock();
    let mut previous: *mut Block = core::ptr::null_mut();
    let mut current = list.head;
    while !current.is_null() && (current as usize) < address {
        previous = current;
        current = unsafe { (*current).next };
    }

    unsafe {
        (*block).next = current;
        if previous.is_null() {
            list.head = block;
        } else {
            (*previous).next = block;
        }
        list.used = list.used.saturating_sub((*block).size);

        // Merge with the following block.
        if !current.is_null() && address + (*block).size == current as usize {
            (*block).size += (*current).size;
            (*block).next = (*current).next;
        }
        // Merge with the preceding block.
        if !previous.is_null()
            && previous as usize + (*previous).size == address
        {
            (*previous).size += (*block).size;
            (*previous).next = (*block).next;
        }
    }
}

/// Allocate zeroed memory.
pub fn alloc_zeroed(layout: Layout) -> Option<NonNull<u8>> {
    let ptr = alloc(layout)?;
    unsafe {
        core::ptr::write_bytes(ptr.as_ptr(), 0, layout.size());
    }
    Some(ptr)
}