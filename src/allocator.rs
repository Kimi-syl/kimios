//! A minimal bump allocator.
//!
//! Phase 4 keeps allocation intentionally simple: a single 1 MiB static heap
//! and a monotonically increasing offset. There is no `free` yet; later
//! phases can replace this with `linked_list_allocator` and a real heap region
//! discovered from the device tree.

use core::alloc::Layout;
use core::ptr::{addr_of_mut, NonNull};
use core::sync::atomic::{AtomicUsize, Ordering};

const HEAP_SIZE: usize = 1024 * 1024; // 1 MiB

#[repr(align(16))]
#[allow(dead_code)] // The byte array is only accessed through its address.
struct Heap([u8; HEAP_SIZE]);

static mut HEAP: Heap = Heap([0; HEAP_SIZE]);
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Bytes currently handed out (offset from the start of the heap).
pub fn used() -> usize {
    NEXT.load(Ordering::Acquire)
}

pub fn capacity() -> usize {
    HEAP_SIZE
}

/// Allocate `layout.size()` bytes with `layout.align()` alignment.
///
/// Returns `None` when the bump pointer would leave the static heap.
pub fn alloc(layout: Layout) -> Option<NonNull<u8>> {
    let size = layout.size();
    let align = layout.align();

    if size == 0 {
        return NonNull::new(align as *mut u8);
    }

    let base = addr_of_mut!(HEAP) as usize;

    let mut current = NEXT.load(Ordering::Acquire);
    loop {
        let start = (base + current + align - 1) & !(align - 1);
        let end = start.checked_add(size)? - base;

        if end > HEAP_SIZE {
            return None;
        }

        match NEXT.compare_exchange_weak(
            current,
            end,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return NonNull::new(start as *mut u8),
            Err(observed) => current = observed,
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
