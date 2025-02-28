use core::{slice, sync::atomic::{compiler_fence, Ordering}};

use cortex_m::{interrupt::disable, peripheral::SCB};
use embassy_nrf::{nvmc::Nvmc, pac::POWER};
use embassy_time::Timer;
use embedded_storage::nor_flash::NorFlash;
use postcard_rpc::{header::VarHeader, server::Sender};
use bootloader_icd::{scratch::BootMessage, AppPartitionInfo, BootloadEndpoint, DataChunk, EraseError, EraseResult, FailedSanityCheck, FlashEraseCommand, FlashReadCommand, FlashWriteCommand, ReadError, ReadResult, WriteError, WriteResult};

use crate::{app::{AppTx, Context, TaskContext}, storage::{app_sanity_check, write_message, APP_FLASH}};

const CHUNK_LIMIT: usize = 512;

/// This is an example of a BLOCKING handler.
pub fn unique_id(context: &mut Context, _header: VarHeader, _arg: ()) -> u64 {
    context.unique_id
}

pub fn get_info(context: &mut Context, _header: VarHeader, _arg: ()) -> AppPartitionInfo {
    let (ptr, flen) = APP_FLASH.get_ptr_len();
    AppPartitionInfo {
        start: ptr as usize as u32,
        len: flen as u32,
        transfer_chunk: CHUNK_LIMIT.min(context.buf.len()) as u32,
        write_sz: Nvmc::WRITE_SIZE as u32,
        erase_sz: Nvmc::ERASE_SIZE as u32,
        align: 4,
    }
}

fn is_inbounds(addr: u32, len: u32) -> bool {
    let rstart = addr as usize;
    let rlen = len as usize;
    let (ptr, flen) = APP_FLASH.get_ptr_len();
    let ptr_usize = ptr as usize;
    if rstart < ptr_usize {
        return false;
    }

    if let Some(rend) = rstart.checked_add(rlen) {
        // After flash end?
        rend <= (ptr_usize + flen)
    } else {
        false
    }
}

fn frange() -> (u32, u32) {
    let (ptr, flen) = APP_FLASH.get_ptr_len();
    let ptr_usize = ptr as usize;
    (ptr_usize as u32, (ptr_usize as u32).saturating_add(flen as u32))
}

pub fn read_flash(context: &mut Context, _header: VarHeader, arg: FlashReadCommand) -> ReadResult<'_> {
    let (ptr, _flen) = APP_FLASH.get_ptr_len();
    let FlashReadCommand { start, len } = arg;
    let ptr_usize = ptr as usize;
    let rstart = start as usize;
    let rlen = len as usize;

    if !is_inbounds(start, len) {
        let (mem_start, mem_end) = frange();
        return Err(ReadError::OutOfRange {
            req_start: arg.start,
            req_end: arg.start.saturating_add(arg.len),
            mem_start,
            mem_end,
        });
    }

    // TODO: not sure what our largest packet size is, for now limit well under
    // 1K total
    let limit = CHUNK_LIMIT.min(context.buf.len());

    // Len larger than our limit?
    if rlen > limit {
        return Err(ReadError::TooLarge { req_len: arg.len, max_len: limit as u32 });
    }

    let bout = &mut context.buf[..rlen];
    let offset = rstart - ptr_usize;
    compiler_fence(Ordering::SeqCst);
    // We checked all the ranges and stuff above
    unsafe {
        let sli = slice::from_raw_parts(ptr.add(offset), rlen);
        bout.copy_from_slice(sli);
    }

    Ok(DataChunk { data: bout })
}

pub async fn erase_flash(context: &mut Context, _header: VarHeader, arg: FlashEraseCommand) -> EraseResult {
    let FlashEraseCommand { start, len, force } = arg;

    if !is_inbounds(start, len) {
        return Err(EraseError::OutOfRange);
    }

    let erase_size = Nvmc::ERASE_SIZE as u32;
    if start % erase_size != 0 {
        return Err(EraseError::StartNotAligned);
    }
    if len % erase_size != 0 {
        return Err(EraseError::LenNotAligned);
    }
    let mut addr = start as usize;
    let end = addr + (len as usize);

    let (ptr, _flen) = APP_FLASH.get_ptr_len();
    let ptr_usize = ptr as usize;

    while addr != end {
        let do_erase = force || !unsafe {
            let offset = addr - ptr_usize;
            compiler_fence(Ordering::SeqCst);
            let sli = slice::from_raw_parts(ptr.add(offset), erase_size as usize);
            sli.iter().all(|b| *b == 0xFF)
        };
        if do_erase {
            if let Err(_e) = context.nvmc.erase(addr as u32, addr as u32 + erase_size) {
                return Err(EraseError::HardwareError);
            }
            // give the hardware a little time to catch up in case we just stalled out
            Timer::after_millis(5).await;
        } else {
            Timer::after_millis(1).await;
        }

        addr += Nvmc::ERASE_SIZE;
    }
    Ok(())
}

#[embassy_executor::task]
pub async fn go_boot(_c: TaskContext, header: VarHeader, _arg: (), sender: Sender<AppTx>) {
    let msg = BootMessage::JustBoot;
    if !app_sanity_check() || !write_message(&msg) {
        let _ = sender.reply::<BootloadEndpoint>(header.seq_no, &Err(FailedSanityCheck)).await;
    } else {
        let _ = sender.reply::<BootloadEndpoint>(header.seq_no, &Ok(())).await;
        // Give some time for the message to be sent before rebooting
        Timer::after_millis(50).await;
        disable();
        SCB::sys_reset();
    }
}

pub fn reboot_reason(context: &mut Context, _header: VarHeader, _arg: ()) -> u32 {
    POWER.resetreas().read().0
}

pub fn get_boot_message(context: &mut Context, _header: VarHeader, _arg: ()) -> Option<BootMessage<'_>> {
    context.boot_message.clone()
}

pub fn write_flash(context: &mut Context, _header: VarHeader, arg: FlashWriteCommand<'_>) -> WriteResult {
    let FlashWriteCommand { start, data, force } = arg;
    let len = data.len() as u32;

    if !is_inbounds(start, len) {
        return Err(WriteError::OutOfRange);
    }
    let write_size = Nvmc::WRITE_SIZE as u32;
    if start % write_size != 0 {
        return Err(WriteError::StartNotAligned);
    }
    if len % write_size != 0 {
        return Err(WriteError::LenNotAligned);
    }

    if !force {
        let (ptr, _flen) = APP_FLASH.get_ptr_len();
        let ptr_usize = ptr as usize;
        let empty = unsafe {
            let offset = (start as usize) - ptr_usize;
            compiler_fence(Ordering::SeqCst);
            let sli = slice::from_raw_parts(ptr.add(offset), len as usize);
            sli.iter().all(|b| *b == 0xFF)
        };
        if !empty {
            return Err(WriteError::NeedsErase);
        }
    }

    match context.nvmc.write(start, data) {
        Ok(()) => Ok(()),
        Err(_) => Err(WriteError::HardwareError),
    }
}
