use core::{slice, sync::atomic::{compiler_fence, Ordering}};

use bootloader_icd::scratch::{BootMessage, BOOT_KEY};
use grounded::uninit::GroundedArrayCell;
use postcard_rpc::Key;

pub const MEM_SCRATCH_SIZE: usize = 1024;
pub const TTL_FLASH: usize = 1024 * 1024;
pub const BOOT_FLASH_SIZE: usize = 64 * 1024;
pub const APP_FLASH_SIZE: usize = TTL_FLASH - BOOT_FLASH_SIZE;

#[no_mangle]
#[used]
#[link_section = ".scratch.MEM_SCRATCH"]
pub static MEM_SCRATCH: GroundedArrayCell<u8, MEM_SCRATCH_SIZE> = GroundedArrayCell::uninit();

pub fn read_message(buf: &mut [u8; MEM_SCRATCH_SIZE]) -> Option<BootMessage<'_>> {
    let (ptr, len) = MEM_SCRATCH.get_ptr_len();
    unsafe {
        compiler_fence(Ordering::SeqCst);
        let sli = slice::from_raw_parts(ptr, len);
        buf.copy_from_slice(sli);
    }
    let (key, remain) = postcard::take_from_bytes::<Key>(buf).ok()?;
    if key != BOOT_KEY {
        return None;
    }
    postcard::from_bytes::<BootMessage>(remain).ok()
}

pub fn clear_message() {
    let (ptr, len) = MEM_SCRATCH.get_ptr_len();
    unsafe {
        ptr.write_bytes(0x00, len);
    }
    compiler_fence(Ordering::SeqCst);
}

pub fn write_message(msg: &BootMessage<'_>) -> bool {
    // Start by clearing message
    clear_message();

    // Then get a mut slice
    let (ptr, len) = MEM_SCRATCH.get_ptr_len();
    let sli = unsafe {
        slice::from_raw_parts_mut(ptr, len)
    };

    // Split off the key, but DON'T write it yet
    let (keyb, datab) = sli.split_at_mut(8);

    // Write message
    if postcard::to_slice(msg, datab).is_err() {
        clear_message();
        return false;
    }
    // Write key as sentinel
    if postcard::to_slice(&BOOT_KEY, keyb).is_err() {
        clear_message();
        false
    } else {
        true
    }
}
