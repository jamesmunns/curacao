use bootloader_icd::scratch::BootMessage;
use bridge_icd::{
    write_topic2, B2NTopic, Bridge2Node, LedState, ProxyError, ProxyMessage, ProxyResult, RebootToBootloader, SleepEndpoint, SleepMillis, SleptMillis
};
use cortex_m::peripheral::SCB;
use embassy_time::{Instant, Timer};
use esb::EsbHeader;
use postcard_rpc::{
    header::{VarHeader, VarSeq},
    server::Sender,
};

use crate::{app::{AppTx, Context, TaskContext}, storage::write_message};

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

pub async fn proxy_handler(
    context: &mut Context,
    _header: VarHeader,
    arg: ProxyMessage<'_>,
) -> ProxyResult {
    let pipe = {
        let guard = context.table.lock().await;
        guard.pipe_for_serial(&arg.serial)
    };
    let Some(pipe) = pipe else {
        return Err(ProxyError::UnknownDevice);
    };

    let chunks = (arg.msg.len() + 127) / 128;
    for (i, ch) in arg.msg.chunks(128).enumerate() {
        let Ok(header) = EsbHeader::new(252, 0, pipe, false) else {
            defmt::error!("Bad header?");
            return Err(ProxyError::UnknownDevice);
        };

        let mut guard = context.esb_sender.sender.lock().await;
        let Ok(mut wgr) = guard.wait_grant_packet(header).await else {
            panic!();
        };

        let res = write_topic2::<B2NTopic>(
            &Bridge2Node::Proxy {
                part: i as u8,
                ttl_parts: chunks as u8,
            },
            VarSeq::Seq2(0),
            &mut wgr,
        )
        .unwrap();
        let (_used, remain) = wgr.split_at_mut(res);
        remain[..ch.len()].copy_from_slice(ch);
        let ttl = res + ch.len();
        wgr.commit(ttl);
        defmt::info!("Proxied to pipe {=u8}", pipe);
    }

    Ok(())
}

/// This is a SPAWN handler
///
/// The pool size of three means we can have up to three of these requests "in flight"
/// at the same time. We will return an error if a fourth is requested at the same time
#[embassy_executor::task(pool_size = 3)]
pub async fn sleep_handler(
    _context: TaskContext,
    header: VarHeader,
    arg: SleepMillis,
    sender: Sender<AppTx>,
) {
    // We can send string logs, using the sender
    let _ = sender.log_str("Starting sleep...").await;
    let start = Instant::now();
    Timer::after_millis(arg.millis.into()).await;
    let _ = sender.log_str("Finished sleep").await;
    // Async handlers have to manually reply, as embassy doesn't support returning by value
    let _ = sender
        .reply::<SleepEndpoint>(
            header.seq_no,
            &SleptMillis {
                millis: start.elapsed().as_millis() as u16,
            },
        )
        .await;
}

#[embassy_executor::task]
pub async fn reboot_bootloader(_c: TaskContext, header: VarHeader, _arg: (), sender: Sender<AppTx>) {
    let msg = BootMessage::StayInBootloader;
    write_message(&msg);
    let _ = sender.reply::<RebootToBootloader>(header.seq_no, &()).await;
    Timer::after_millis(100).await;
    SCB::sys_reset();
}
