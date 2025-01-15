use crate::{
    app::{AppTx, Context, TaskContext},
    storage::write_message,
};
use bootloader_icd::scratch::BootMessage;
use bridge_icd::RebootToBootloader;
use cortex_m::peripheral::SCB;
use embassy_time::Timer;
use postcard_rpc::{header::VarHeader, server::Sender};

/// This is an example of a BLOCKING handler.
pub fn unique_id(context: &mut Context, _header: VarHeader, _arg: ()) -> u64 {
    context.unique_id
}


#[embassy_executor::task]
pub async fn reboot_bootloader(
    _c: TaskContext,
    header: VarHeader,
    _arg: (),
    sender: Sender<AppTx>,
) {
    let msg = BootMessage::StayInBootloader;
    write_message(&msg);
    let _ = sender.reply::<RebootToBootloader>(header.seq_no, &()).await;
    Timer::after_millis(100).await;
    SCB::sys_reset();
}
