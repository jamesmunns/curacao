use grounded::uninit::GroundedArrayCell;

const TTL_FLASH: usize = 1024 * 1024;
const BOOT_FLASH_SIZE: usize = 128 * 1024;
const APP_FLASH_SIZE: usize = TTL_FLASH - BOOT_FLASH_SIZE;

#[no_mangle]
#[used]
#[link_section = ".app.APP_FLASH"]
pub static APP_FLASH: GroundedArrayCell<u8, APP_FLASH_SIZE> = GroundedArrayCell::uninit();
