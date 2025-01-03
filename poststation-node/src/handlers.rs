use crate::{app::{AppTx, Context, TaskContext}, impls::EsbTx, smartled::smartled, storage::write_message};
use bootloader_icd::scratch::BootMessage;
use bridge_icd::RebootToBootloader;
use cortex_m::peripheral::SCB;
use embassy_time::Timer;
use node_icd::{Dummy, InvalidIndex, LedState, SetRGBCommand, SetRGBResult, RGB8};
use postcard_rpc::{header::VarHeader, server::Sender};

/// This is an example of a BLOCKING handler.
pub fn unique_id(context: &mut Context, _header: VarHeader, _arg: ()) -> u64 {
    context.unique_id
}

/// Also a BLOCKING handler
pub fn set_led(context: &mut Context, _header: VarHeader, arg: LedState) {
    match arg {
        LedState::Off => context.led.set_high(),
        LedState::On => context.led.set_low(),
    }
}

pub fn get_led(context: &mut Context, _header: VarHeader, _arg: ()) -> LedState {
    match context.led.is_set_high() {
        true => LedState::Off,
        false => LedState::On,
    }
}

pub async fn set_one_rgb(context: &mut Context, _header: VarHeader, arg: SetRGBCommand) -> SetRGBResult {
    let pos = arg.pos as usize;
    if pos > context.rgb_buf.len() {
        return Err(InvalidIndex);
    }
    context.rgb_buf[pos] = arg.color;
    let Context { smartled: pwm, rgb_buf, data_buf, .. } = context;
    smartled(pwm, rgb_buf, data_buf).await;
    Ok(())
}

pub async fn set_all_rgb(context: &mut Context, _header: VarHeader, arg: RGB8) {
    context.rgb_buf.iter_mut().for_each(|c| *c = arg);
    let Context { smartled: pwm, rgb_buf, data_buf, .. } = context;
    smartled(pwm, rgb_buf, data_buf).await;
}

pub fn handle_dummy(
    _context: &mut Context,
    _header: VarHeader,
    arg: Dummy,
    _sender: &Sender<EsbTx>,
) {
    // defmt::info!("Handled dummy via postcard-rpc dispatch {:?}", arg.data)
}

// pub async fn proxy_handler(context: &mut Context, _header: VarHeader, arg: ProxyMessage<'_>) -> ProxyResult {
//     let pipe = {
//         let guard = context.table.lock().await;
//         guard.pipe_for_serial(&arg.serial)
//     };
//     let Some(pipe) = pipe else {
//         return Err(ProxyError::UnknownDevice);
//     };

//     let Ok(header) = EsbHeader::new(252, 0, pipe, false) else {
//         defmt::error!("Bad header?");
//         return Err(ProxyError::UnknownDevice);
//     };

//     let mut guard = context.esb_sender.sender.lock().await;
//     let Ok(mut wgr) = guard.wait_grant_packet(header).await else {
//         panic!();
//     };

//     let res = write_topic2::<B2NTopic>(&Bridge2Node::Proxy, VarSeq::Seq2(0), &mut wgr).unwrap();
//     let (_used, remain) = wgr.split_at_mut(res);
//     remain[..arg.msg.len()].copy_from_slice(arg.msg);
//     let ttl = res + arg.msg.len();
//     wgr.commit(ttl);
//     defmt::info!("Proxied to pipe {=u8}", pipe);

//     Ok(())
// }

// /// This is a SPAWN handler
// ///
// /// The pool size of three means we can have up to three of these requests "in flight"
// /// at the same time. We will return an error if a fourth is requested at the same time
// #[embassy_executor::task(pool_size = 3)]
// pub async fn sleep_handler(_context: TaskContext, header: VarHeader, arg: SleepMillis, sender: Sender<AppTx>) {
//     // We can send string logs, using the sender
//     let _ = sender.log_str("Starting sleep...").await;
//     let start = Instant::now();
//     Timer::after_millis(arg.millis.into()).await;
//     let _ = sender.log_str("Finished sleep").await;
//     // Async handlers have to manually reply, as embassy doesn't support returning by value
//     let _ = sender.reply::<SleepEndpoint>(header.seq_no, &SleptMillis { millis: start.elapsed().as_millis() as u16 }).await;
// }

#[embassy_executor::task]
pub async fn reboot_bootloader(_c: TaskContext, header: VarHeader, _arg: (), sender: Sender<AppTx>) {
    let msg = BootMessage::StayInBootloader;
    write_message(&msg);
    let _ = sender.reply::<RebootToBootloader>(header.seq_no, &()).await;
    Timer::after_millis(100).await;
    SCB::sys_reset();
}
