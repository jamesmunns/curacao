use embassy_time::{Instant, Timer};
use postcard_rpc::{header::VarHeader, server::Sender};
use template_icd::{
    LedState, SleepEndpoint, SleepMillis, SleptMillis
};

use crate::app::{AppTx, Context, TaskContext};

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

// pub async fn set_display(
//     context: &mut Context,
//     _header: VarHeader,
//     arg: DisplayCommand<'_>,
// ) -> DisplayResult {


//     let Some(dbuf) = context.dispbuf.get_mut((arg.offset as usize)..) else {
//         return DisplayResult::Err(DecodeError::Overflow)
//     };

//     match decode_to(arg.data, dbuf) {
//         Ok(()) => {}
//         Err(_) => return DisplayResult::Err(DecodeError::DecodeOrEof),
//     }

//     Ok(())
// }

// pub async fn draw_display(
//     context: &mut Context,
//     _header: VarHeader,
//     _arg: (),
// ) -> DrawResult {
//     if Instant::now() < context.next_draw {
//         return DrawResult::Err(TooSoon);
//     }

//     defmt::println!("Updating!");
//     let start = Instant::now();
//     context.next_draw = start + Duration::from_secs(50);
//     defmt::println!("{=u64}ms - Waking display...", start.elapsed().as_millis());
//     context
//         .display
//         .wake_up(&mut context.spi, &mut Delay)
//         .await
//         .unwrap();
//     defmt::println!("{=u64}ms - Drawring", start.elapsed().as_millis());
//     context
//         .display
//         .update_and_display_frame(&mut context.spi, context.dispbuf)
//         .await
//         .unwrap();
//     defmt::println!("{=u64}ms - Waiting for idle", start.elapsed().as_millis());
//     context
//         .display
//         .wait_until_idle(&mut context.spi)
//         .await
//         .unwrap();
//     defmt::println!(
//         "{=u64}ms - Putting back to sleep",
//         start.elapsed().as_millis()
//     );
//     context.display.sleep(&mut context.spi).await.unwrap();
//     defmt::println!("{=u64}ms - Done", start.elapsed().as_millis());

//     Ok(())
// }

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
