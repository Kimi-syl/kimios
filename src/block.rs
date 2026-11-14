//! Block-device abstraction.
//!
//! Filesystems talk to this module, not to a transport. `virtio-blk` is the
//! first implementation; UFS/SD would register the same trait on hardware.

use spin::Mutex;

pub trait BlockDevice: Sync {
    fn block_size(&self) -> usize;
    fn block_count(&self) -> u64;
    fn read_block(&self, lba: u64, buffer: &mut [u8]) -> Result<(), &'static str>;
    fn write_block(&self, lba: u64, buffer: &[u8]) -> Result<(), &'static str>;
    fn flush(&self) -> Result<(), &'static str>;
}

static DEVICE: Mutex<Option<&'static dyn BlockDevice>> = Mutex::new(None);

pub fn register(device: &'static dyn BlockDevice) {
    *DEVICE.lock() = Some(device);
}

pub fn geometry() -> Option<(usize, u64)> {
    DEVICE
        .lock()
        .map(|device| (device.block_size(), device.block_count()))
}

pub fn read_block(lba: u64, buffer: &mut [u8]) -> Result<(), &'static str> {
    let device = DEVICE.lock().ok_or("no block device")?;
    device.read_block(lba, buffer)
}

pub fn write_block(lba: u64, buffer: &[u8]) -> Result<(), &'static str> {
    let device = DEVICE.lock().ok_or("no block device")?;
    device.write_block(lba, buffer)
}

pub fn flush() -> Result<(), &'static str> {
    let device = DEVICE.lock().ok_or("no block device")?;
    device.flush()
}
