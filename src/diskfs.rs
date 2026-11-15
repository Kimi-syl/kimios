//! KimiFS: a tiny persistent, contiguous-file filesystem.
//!
//! Layout (512-byte sectors):
//! - LBA 0: superblock (magic, version, geometry, next free block).
//! - LBA 1..17: 64 fixed 128-byte inodes.
//! - LBA 17..: file data, each file allocated as a contiguous block run.
//!
//! There are no directories yet. Files are created/truncated by name and kept
//! as one contiguous run. A rebuilt-in-RAM block bitmap makes allocation
//! first-fit, so `unlink` and truncation return their runs for reuse instead
//! of leaking them. Every namespace update is written through to the block
//! device and flushed, which is enough to survive a reboot.

use spin::Mutex;

use crate::block;

const MAGIC: &[u8; 8] = b"KIMIFS1\0";
const VERSION: u32 = 1;
const SECTOR_SIZE: usize = 512;
const INODE_COUNT: usize = 64;
const INODE_SIZE: usize = 128;
const INODE_LBA: u64 = 1;
const INODE_BLOCKS: u64 = (INODE_COUNT * INODE_SIZE / SECTOR_SIZE) as u64;
const DATA_LBA: u64 = INODE_LBA + INODE_BLOCKS;
const MAX_NAME: usize = 31;

/// Bitmap capacity: 134_217_728 blocks (64 GiB). Larger disks are not mounted.
const MAX_BLOCKS: u64 = 134_217_728;
const BITMAP_WORDS: usize = (MAX_BLOCKS as usize) / 64;

const INODE_USED: usize = 32;
const INODE_KIND: usize = 33;
const INODE_SIZE_OFF: usize = 36;
const INODE_FIRST: usize = 44;
const INODE_BLOCKS_OFF: usize = 52;
const INODE_PARENT: usize = 60;

const KIND_FILE: u8 = 1;
const KIND_DIR: u8 = 2;
/// Parent value used by entries in the filesystem root. Inode 0 is reserved.
const ROOT: u64 = 0;
const MAX_COMPONENTS: usize = 8;

struct Fs {
    mounted: bool,
    total_blocks: u64,
    /// High-water mark kept in the superblock for information/back-compat.
    next_free: u64,
    files: u64,
    free: u64,
    used: [u64; BITMAP_WORDS],
}

static FS: Mutex<Fs> = Mutex::new(Fs {
    mounted: false,
    total_blocks: 0,
    next_free: DATA_LBA,
    files: 0,
    free: 0,
    used: [0; BITMAP_WORDS],
});

impl Fs {
    fn is_used(&self, block: u64) -> bool {
        let word = (block / 64) as usize;
        let bit = 1u64 << (block % 64);
        word < self.used.len() && self.used[word] & bit != 0
    }

    /// Set/clear a bit without touching the free count (used while rebuilding).
    fn raw_set(&mut self, block: u64, used: bool) {
        if block >= self.total_blocks {
            return;
        }
        let word = (block / 64) as usize;
        let bit = 1u64 << (block % 64);
        if used {
            self.used[word] |= bit;
        } else {
            self.used[word] &= !bit;
        }
    }

    fn set_used(&mut self, block: u64, used: bool) {
        if block < DATA_LBA || block >= self.total_blocks {
            return;
        }
        let was = self.is_used(block);
        if was == used {
            return;
        }
        self.raw_set(block, used);
        if used {
            self.free -= 1;
        } else {
            self.free += 1;
        }
    }

    fn mark_range(&mut self, first: u64, blocks: u64, used: bool) {
        for index in 0..blocks {
            self.set_used(first + index, used);
        }
    }

    /// Rebuild the allocation bitmap by scanning the inode table.
    fn rebuild(&mut self) -> Result<(), &'static str> {
        self.used = [0; BITMAP_WORDS];
        for block in 0..DATA_LBA {
            self.raw_set(block, true);
        }

        let mut inode = [0u8; INODE_SIZE];
        let mut data_used = 0u64;
        for index in 0..INODE_COUNT {
            read_inode(index, &mut inode)?;
            if inode[INODE_USED] == 0 {
                continue;
            }
            let first = read_u64(&inode, INODE_FIRST);
            let blocks = read_u64(&inode, INODE_BLOCKS_OFF);
            for block in first..first + blocks {
                if block >= DATA_LBA && block < self.total_blocks && !self.is_used(block) {
                    self.raw_set(block, true);
                    data_used += 1;
                }
            }
        }

        self.free = self.total_blocks - DATA_LBA - data_used;
        Ok(())
    }

    /// First-fit allocation of `needed` consecutive free blocks.
    fn alloc_run(&mut self, needed: u64) -> Option<u64> {
        if needed == 0 {
            return None;
        }

        let mut run_start = 0u64;
        let mut run = 0u64;
        let mut block = DATA_LBA;
        while block < self.total_blocks {
            if self.is_used(block) {
                run = 0;
            } else {
                if run == 0 {
                    run_start = block;
                }
                run += 1;
                if run == needed {
                    for index in run_start..run_start + needed {
                        self.set_used(index, true);
                    }
                    if run_start + needed > self.next_free {
                        self.next_free = run_start + needed;
                    }
                    return Some(run_start);
                }
            }
            block += 1;
        }
        None
    }
}

fn write_superblock(fs: &Fs) -> Result<(), &'static str> {
    block::write_block(0, &build_superblock(fs.total_blocks, fs.next_free))
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn build_superblock(total: u64, next_free: u64) -> [u8; SECTOR_SIZE] {
    let mut sector = [0u8; SECTOR_SIZE];
    sector[..8].copy_from_slice(MAGIC);
    write_u32(&mut sector, 8, VERSION);
    write_u32(&mut sector, 12, SECTOR_SIZE as u32);
    write_u64(&mut sector, 16, total);
    write_u32(&mut sector, 24, INODE_COUNT as u32);
    write_u64(&mut sector, 32, INODE_LBA);
    write_u64(&mut sector, 40, DATA_LBA);
    write_u64(&mut sector, 48, next_free);
    sector
}

fn format(total: u64) -> Result<(), &'static str> {
    if total <= DATA_LBA + 8 {
        return Err("disk too small");
    }

    let zero = [0u8; SECTOR_SIZE];
    for lba in INODE_LBA..DATA_LBA {
        block::write_block(lba, &zero)?;
    }
    block::write_block(0, &build_superblock(total, DATA_LBA))?;
    block::flush()?;

    let mut fs = FS.lock();
    fs.mounted = true;
    fs.total_blocks = total;
    fs.next_free = DATA_LBA;
    fs.files = 0;
    fs.free = total - DATA_LBA;
    fs.used = [0; BITMAP_WORDS];
    for block in 0..DATA_LBA {
        fs.raw_set(block, true);
    }
    Ok(())
}

/// Mount an existing filesystem, or format a blank/foreign disk.
pub fn mount() -> bool {
    let Some((block_size, total)) = block::geometry() else {
        return false;
    };
    if block_size != SECTOR_SIZE {
        crate::println!("[disk] unsupported block size {}", block_size);
        return false;
    }

    let mut sector = [0u8; SECTOR_SIZE];
    let valid = block::read_block(0, &mut sector).is_ok()
        && &sector[..8] == MAGIC
        && read_u32(&sector, 8) == VERSION
        && read_u32(&sector, 12) == SECTOR_SIZE as u32
        && read_u64(&sector, 16) == total
        && read_u64(&sector, 40) == DATA_LBA;

    if !valid {
        crate::println!("[disk] no KimiFS found; formatting {} sectors", total);
        if let Err(error) = format(total) {
            crate::println!("[disk] format failed: {}", error);
            return false;
        }
        return true;
    }

    if let Err(error) = migrate_reserved_inode() {
        crate::println!("[disk] inode migration failed: {}", error);
        return false;
    }

    if total > MAX_BLOCKS {
        crate::println!(
            "[disk] KimiFS supports at most {} sectors; {} not mounted",
            MAX_BLOCKS,
            total
        );
        return false;
    }

    let next_free = read_u64(&sector, 48).max(DATA_LBA);
    let mut files = 0u64;
    if let Ok(count) = count_files() {
        files = count;
    }

    let mut fs = FS.lock();
    fs.mounted = true;
    fs.total_blocks = total;
    fs.next_free = next_free;
    fs.files = files;
    if let Err(error) = fs.rebuild() {
        crate::println!("[disk] bitmap rebuild failed: {}", error);
        fs.mounted = false;
        return false;
    }
    crate::println!(
        "[disk] KimiFS mounted: {} sectors, {} file(s), {} free block(s)",
        total,
        files,
        fs.free
    );
    true
}

fn count_files() -> Result<u64, &'static str> {
    let mut count = 0;
    let mut inode = [0u8; INODE_SIZE];
    for index in 0..INODE_COUNT {
        read_inode(index, &mut inode)?;
        if inode[INODE_USED] != 0 {
            count += 1;
        }
    }
    Ok(count)
}

/// Inode 0 is reserved (specifically not a directory) so that parent 0 can
/// mean "root". Older disks may store a file there; move it once at mount.
fn migrate_reserved_inode() -> Result<(), &'static str> {
    let mut first = [0u8; INODE_SIZE];
    read_inode(0, &mut first)?;
    if first[INODE_USED] == 0 {
        return Ok(());
    }

    let mut inode = [0u8; INODE_SIZE];
    for index in 1..INODE_COUNT {
        read_inode(index, &mut inode)?;
        if inode[INODE_USED] == 0 {
            write_inode(index, &first)?;
            let zero = [0u8; INODE_SIZE];
            write_inode(0, &zero)?;
            block::flush()?;
            crate::println!("[disk] moved legacy inode 0 to {}", index);
            return Ok(());
        }
    }
    Err("no free inode for migration")
}

fn inode_lba(index: usize) -> (u64, usize) {
    let byte = index * INODE_SIZE;
    (INODE_LBA + (byte / SECTOR_SIZE) as u64, byte % SECTOR_SIZE)
}

fn read_inode(index: usize, inode: &mut [u8; INODE_SIZE]) -> Result<(), &'static str> {
    if index >= INODE_COUNT {
        return Err("inode out of range");
    }
    let (lba, offset) = inode_lba(index);
    let mut sector = [0u8; SECTOR_SIZE];
    block::read_block(lba, &mut sector)?;
    inode.copy_from_slice(&sector[offset..offset + INODE_SIZE]);
    Ok(())
}

fn write_inode(index: usize, inode: &[u8; INODE_SIZE]) -> Result<(), &'static str> {
    if index >= INODE_COUNT {
        return Err("inode out of range");
    }
    let (lba, offset) = inode_lba(index);
    let mut sector = [0u8; SECTOR_SIZE];
    block::read_block(lba, &mut sector)?;
    sector[offset..offset + INODE_SIZE].copy_from_slice(inode);
    block::write_block(lba, &sector)
}

fn inode_name(inode: &[u8; INODE_SIZE]) -> &str {
    let end = inode[..32].iter().position(|&byte| byte == 0).unwrap_or(32);
    core::str::from_utf8(&inode[..end]).unwrap_or("")
}

fn name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name.bytes().all(|byte| byte.is_ascii_graphic() && byte != b'/')
}

fn split_path(path: &str) -> Option<([&str; MAX_COMPONENTS], usize)> {
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains("//")
    {
        return None;
    }
    let mut parts = [""; MAX_COMPONENTS];
    let mut count = 0;
    for part in path.split('/') {
        if count == MAX_COMPONENTS || !name_is_valid(part) {
            return None;
        }
        parts[count] = part;
        count += 1;
    }
    Some((parts, count))
}

/// Find `name` in directory `parent` (0 = root). Caller holds the FS lock.
fn find_child(parent: u64, name: &str) -> Option<usize> {
    let mut inode = [0u8; INODE_SIZE];
    for index in 1..INODE_COUNT {
        if read_inode(index, &mut inode).is_err() {
            return None;
        }
        if inode[INODE_USED] != 0
            && read_u64(&inode, INODE_PARENT) == parent
            && inode_name(&inode) == name
        {
            return Some(index);
        }
    }
    None
}

/// Resolve the first `count` components as directories, returning the final
/// directory inode (0 = root). Caller holds the FS lock.
fn resolve_dir(parts: &[&str], count: usize) -> Option<u64> {
    let mut parent = ROOT;
    for part in parts.iter().take(count) {
        let index = find_child(parent, part)?;
        let mut inode = [0u8; INODE_SIZE];
        read_inode(index, &mut inode).ok()?;
        if inode[INODE_KIND] != KIND_DIR {
            return None;
        }
        parent = index as u64;
    }
    Some(parent)
}

/// Split a path into its parent directory and final component.
fn resolve<'a>(path: &'a str) -> Option<(u64, &'a str)> {
    let (parts, count) = split_path(path)?;
    let parent = resolve_dir(&parts, count - 1)?;
    Some((parent, parts[count - 1]))
}

fn first_free_inode() -> Option<usize> {
    let mut inode = [0u8; INODE_SIZE];
    for index in 1..INODE_COUNT {
        read_inode(index, &mut inode).ok()?;
        if inode[INODE_USED] == 0 {
            return Some(index);
        }
    }
    None
}

/// Find a file or directory by disk-relative path.
pub fn lookup(path: &str) -> Option<usize> {
    let fs = FS.lock();
    if !fs.mounted {
        return None;
    }
    let (parent, name) = resolve(path)?;
    find_child(parent, name)
}

/// Create or truncate a file; returns its inode index.
pub fn create(path: &str) -> Option<usize> {
    let (parts, count) = split_path(path)?;
    let mut fs = FS.lock();
    if !fs.mounted {
        return None;
    }
    let parent = resolve_dir(&parts, count - 1)?;
    let name = parts[count - 1];

    if let Some(index) = find_child(parent, name) {
        let mut inode = [0u8; INODE_SIZE];
        read_inode(index, &mut inode).ok()?;
        if inode[INODE_KIND] == KIND_DIR {
            return None;
        }
        let first = read_u64(&inode, INODE_FIRST);
        let blocks = read_u64(&inode, INODE_BLOCKS_OFF);
        if blocks > 0 {
            fs.mark_range(first, blocks, false);
        }
        write_u64(&mut inode, INODE_SIZE_OFF, 0);
        write_u64(&mut inode, INODE_FIRST, 0);
        write_u64(&mut inode, INODE_BLOCKS_OFF, 0);
        let result = write_inode(index, &inode);
        let _ = write_superblock(&fs);
        let _ = block::flush();
        return result.is_ok().then_some(index);
    }

    let index = first_free_inode()?;
    let mut fresh = [0u8; INODE_SIZE];
    fresh[..name.len()].copy_from_slice(name.as_bytes());
    fresh[INODE_USED] = 1;
    fresh[INODE_KIND] = KIND_FILE;
    write_u64(&mut fresh, INODE_PARENT, parent);
    write_inode(index, &fresh).ok()?;
    fs.files += 1;
    let _ = block::flush();
    Some(index)
}

/// Create a directory; fails if the name already exists.
pub fn mkdir(path: &str) -> bool {
    let Some((parts, count)) = split_path(path) else {
        return false;
    };
    let mut fs = FS.lock();
    if !fs.mounted {
        return false;
    }
    let Some(parent) = resolve_dir(&parts, count - 1) else {
        return false;
    };
    let name = parts[count - 1];
    if find_child(parent, name).is_some() {
        return false;
    }
    let Some(index) = first_free_inode() else {
        return false;
    };
    let mut fresh = [0u8; INODE_SIZE];
    fresh[..name.len()].copy_from_slice(name.as_bytes());
    fresh[INODE_USED] = 1;
    fresh[INODE_KIND] = KIND_DIR;
    write_u64(&mut fresh, INODE_PARENT, parent);
    if write_inode(index, &fresh).is_err() {
        return false;
    }
    fs.files += 1;
    let _ = block::flush();
    true
}

/// Read up to `buffer.len()` bytes at `offset`.
pub fn read(inode_index: usize, offset: u64, buffer: &mut [u8]) -> usize {
    let fs = FS.lock();
    if !fs.mounted {
        return 0;
    }
    let mut inode = [0u8; INODE_SIZE];
    if read_inode(inode_index, &mut inode).is_err() || inode[INODE_USED] == 0 {
        return 0;
    }

    let size = read_u64(&inode, INODE_SIZE_OFF);
    if offset >= size || buffer.is_empty() {
        return 0;
    }
    let first = read_u64(&inode, INODE_FIRST);
    let count = core::cmp::min(buffer.len() as u64, size - offset) as usize;

    let mut done = 0;
    let mut sector = [0u8; SECTOR_SIZE];
    while done < count {
        let position = offset + done as u64;
        let lba = first + position / SECTOR_SIZE as u64;
        let within = (position % SECTOR_SIZE as u64) as usize;
        if block::read_block(lba, &mut sector).is_err() {
            break;
        }
        let chunk = core::cmp::min(SECTOR_SIZE - within, count - done);
        buffer[done..done + chunk].copy_from_slice(&sector[within..within + chunk]);
        done += chunk;
    }
    done
}

fn zero_range(first: u64, start: u64, end: u64) {
    if start >= end {
        return;
    }
    let mut sector = [0u8; SECTOR_SIZE];
    let mut position = start;
    while position < end {
        let lba = first + position / SECTOR_SIZE as u64;
        let within = (position % SECTOR_SIZE as u64) as usize;
        if block::read_block(lba, &mut sector).is_err() {
            return;
        }
        let chunk = core::cmp::min(SECTOR_SIZE - within, (end - position) as usize);
        sector[within..within + chunk].fill(0);
        if block::write_block(lba, &sector).is_err() {
            return;
        }
        position += chunk as u64;
    }
}

/// Write `data` at `offset`, growing the contiguous allocation if needed.
pub fn write(inode_index: usize, offset: u64, data: &[u8]) -> Result<usize, &'static str> {
    let mut fs = FS.lock();
    if !fs.mounted {
        return Err("filesystem not mounted");
    }

    let mut inode = [0u8; INODE_SIZE];
    read_inode(inode_index, &mut inode)?;
    if inode[INODE_USED] == 0 {
        return Err("inode not in use");
    }

    let end = offset.checked_add(data.len() as u64).ok_or("file too large")?;
    let needed = (end + SECTOR_SIZE as u64 - 1) / SECTOR_SIZE as u64;
    if needed == 0 {
        return Ok(0);
    }

    let old_size = read_u64(&inode, INODE_SIZE_OFF);
    let old_blocks = read_u64(&inode, INODE_BLOCKS_OFF);
    let old_first = read_u64(&inode, INODE_FIRST);

    let mut first = old_first;
    if needed > old_blocks {
        let Some(start) = fs.alloc_run(needed) else {
            return Err("disk full");
        };

        for block_index in 0..needed {
            let zero = [0u8; SECTOR_SIZE];
            block::write_block(start + block_index, &zero)?;
        }
        let copy_length = core::cmp::min(old_size, needed * SECTOR_SIZE as u64);
        let mut sector = [0u8; SECTOR_SIZE];
        let mut copied = 0u64;
        while copied < copy_length {
            let source = old_first + copied / SECTOR_SIZE as u64;
            let target = start + copied / SECTOR_SIZE as u64;
            let chunk = core::cmp::min(
                SECTOR_SIZE as u64 - copied % SECTOR_SIZE as u64,
                copy_length - copied,
            ) as usize;
            block::read_block(source, &mut sector)?;
            block::write_block(target, &sector[..])?;
            copied += chunk as u64;
        }

        if old_blocks > 0 {
            fs.mark_range(old_first, old_blocks, false);
        }
        first = start;
        write_u64(&mut inode, INODE_FIRST, first);
        write_u64(&mut inode, INODE_BLOCKS_OFF, needed);
        write_superblock(&fs)?;
    }

    if offset > old_size {
        zero_range(first, old_size, offset);
    }

    let mut done = 0usize;
    let mut sector = [0u8; SECTOR_SIZE];
    while done < data.len() {
        let position = offset + done as u64;
        let lba = first + position / SECTOR_SIZE as u64;
        let within = (position % SECTOR_SIZE as u64) as usize;
        let chunk = core::cmp::min(SECTOR_SIZE - within, data.len() - done);
        if within == 0 && chunk == SECTOR_SIZE {
            block::write_block(lba, &data[done..done + chunk])?;
        } else {
            block::read_block(lba, &mut sector)?;
            sector[within..within + chunk].copy_from_slice(&data[done..done + chunk]);
            block::write_block(lba, &sector)?;
        }
        done += chunk;
    }

    let size = core::cmp::max(old_size, end);
    write_u64(&mut inode, INODE_SIZE_OFF, size);
    write_inode(inode_index, &inode)?;
    block::flush()?;
    Ok(data.len())
}

/// Remove a file, returning its block run to the bitmap.
pub fn unlink(path: &str) -> bool {
    let Some((parent, name)) = resolve(path) else {
        return false;
    };
    let mut fs = FS.lock();
    if !fs.mounted {
        return false;
    }
    let Some(index) = find_child(parent, name) else {
        return false;
    };
    let mut inode = [0u8; INODE_SIZE];
    if read_inode(index, &mut inode).is_err() {
        return false;
    }
    if inode[INODE_KIND] == KIND_DIR {
        return false;
    }

    let first = read_u64(&inode, INODE_FIRST);
    let blocks = read_u64(&inode, INODE_BLOCKS_OFF);
    if blocks > 0 {
        fs.mark_range(first, blocks, false);
    }

    let zero = [0u8; INODE_SIZE];
    if write_inode(index, &zero).is_err() {
        return false;
    }
    fs.files = fs.files.saturating_sub(1);
    let _ = write_superblock(&fs);
    let _ = block::flush();
    true
}

/// Remove an empty directory.
pub fn rmdir(path: &str) -> bool {
    let Some((parent, name)) = resolve(path) else {
        return false;
    };
    let mut fs = FS.lock();
    if !fs.mounted {
        return false;
    }
    let Some(index) = find_child(parent, name) else {
        return false;
    };
    let mut inode = [0u8; INODE_SIZE];
    if read_inode(index, &mut inode).is_err() || inode[INODE_KIND] != KIND_DIR {
        return false;
    }

    let mut child = [0u8; INODE_SIZE];
    for other in 1..INODE_COUNT {
        if read_inode(other, &mut child).is_err() {
            return false;
        }
        if child[INODE_USED] != 0 && read_u64(&child, INODE_PARENT) == index as u64 {
            return false;
        }
    }

    let zero = [0u8; INODE_SIZE];
    if write_inode(index, &zero).is_err() {
        return false;
    }
    fs.files = fs.files.saturating_sub(1);
    let _ = block::flush();
    true
}

/// Entry kind, size and block count for a path.
pub fn stat(path: &str) -> Option<(u64, u64, u64)> {
    let fs = FS.lock();
    if !fs.mounted {
        return None;
    }
    let (parent, name) = resolve(path)?;
    let index = find_child(parent, name)?;
    let mut inode = [0u8; INODE_SIZE];
    read_inode(index, &mut inode).ok()?;
    Some((
        inode[INODE_KIND] as u64,
        read_u64(&inode, INODE_SIZE_OFF),
        read_u64(&inode, INODE_BLOCKS_OFF),
    ))
}

/// Rename or move a file or directory.
pub fn rename(old: &str, new: &str) -> bool {
    if old == new {
        return false;
    }
    let Some((old_parent, old_name)) = resolve(old) else {
        return false;
    };
    let Some((new_parent, new_name)) = resolve(new) else {
        return false;
    };

    let fs = FS.lock();
    if !fs.mounted {
        return false;
    }
    let Some(index) = find_child(old_parent, old_name) else {
        return false;
    };
    if find_child(new_parent, new_name).is_some() {
        return false;
    }
    // Moving a directory into itself would detach a subtree.
    let mut inode = [0u8; INODE_SIZE];
    if read_inode(index, &mut inode).is_err() {
        return false;
    }
    if inode[INODE_KIND] == KIND_DIR {
        let mut cursor = new_parent;
        while cursor != ROOT {
            if cursor == index as u64 {
                return false;
            }
            let Some(entry) = find_inode_by_index(cursor as usize) else {
                return false;
            };
            cursor = entry;
        }
    }

    for byte in &mut inode[..32] {
        *byte = 0;
    }
    inode[..new_name.len()].copy_from_slice(new_name.as_bytes());
    write_u64(&mut inode, INODE_PARENT, new_parent);
    if write_inode(index, &inode).is_err() {
        return false;
    }
    let _ = block::flush();
    true
}

fn find_inode_by_index(index: usize) -> Option<u64> {
    let mut inode = [0u8; INODE_SIZE];
    read_inode(index, &mut inode).ok()?;
    if inode[INODE_USED] == 0 {
        return None;
    }
    Some(read_u64(&inode, INODE_PARENT))
}

/// Visit the direct children of a directory (`path == ""` means the root).
pub fn list_dir(path: &str, mut visit: impl FnMut(&str, bool)) {
    let fs = FS.lock();
    if !fs.mounted {
        return;
    }
    let parent = if path.is_empty() {
        ROOT
    } else {
        let Some((parts, count)) = split_path(path) else {
            return;
        };
        if count == 0 {
            ROOT
        } else {
            let Some(dir) = resolve_dir(&parts, count) else {
                return;
            };
            dir
        }
    };

    let mut inode = [0u8; INODE_SIZE];
    for index in 1..INODE_COUNT {
        if read_inode(index, &mut inode).is_err() {
            return;
        }
        if inode[INODE_USED] != 0 && read_u64(&inode, INODE_PARENT) == parent {
            visit(inode_name(&inode), inode[INODE_KIND] == KIND_DIR);
        }
    }
}

/// Free data blocks (excluding the superblock/inode area).
pub fn free_blocks() -> Option<u64> {
    let fs = FS.lock();
    if fs.mounted {
        Some(fs.free)
    } else {
        None
    }
}

/// Returns `(total_sectors, file_count)` when mounted.
pub fn info() -> Option<(u64, u64)> {
    let fs = FS.lock();
    if fs.mounted {
        Some((fs.total_blocks, fs.files))
    } else {
        None
    }
}
