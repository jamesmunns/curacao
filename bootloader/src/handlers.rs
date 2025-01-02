use core::{slice, sync::atomic::{compiler_fence, Ordering}};

use embassy_time::{Instant, Timer};
use postcard_rpc::{header::VarHeader, server::Sender};
use bootloader_icd::{AppPartitionInfo, DataChunk, FlashReadCommand, ReadError, ReadResult};

use crate::{app::{AppTx, Context, TaskContext}, storage::APP_FLASH};

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
    }
}


pub fn read_flash(context: &mut Context, _header: VarHeader, arg: FlashReadCommand) -> ReadResult<'_> {
    let (ptr, flen) = APP_FLASH.get_ptr_len();
    let FlashReadCommand { start, len } = arg;
    let ptr_usize = ptr as usize;
    let rstart = start as usize;
    let rlen = len as usize;

    let oor = ReadError::OutOfRange {
        req_start: arg.start,
        req_end: arg.start.saturating_add(arg.len),
        mem_start: ptr_usize as u32,
        mem_end: (ptr_usize as u32).saturating_add(flen as u32),
    };

    // Before flash start?
    if rstart < ptr_usize {
        return Err(oor)
    }

    // Valid end?
    if let Some(rend) = rstart.checked_add(rlen) {
        // After flash end?
        if rend > (ptr_usize + flen) {
            return Err(oor);
        }
    } else {
        return Err(oor);
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
