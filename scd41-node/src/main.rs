#![no_std]
#![no_main]

pub mod app;
pub mod handlers;
pub mod impls;
pub mod storage;
pub mod smartled;

use core::{
    fmt::Write,
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use app::AppTx;
use bootloader_icd::scratch::BootMessage;
use bridge_icd::{
    extract_topic2, postcard_rpc::header::VarSeq, write_topic2, B2NTopic, Bridge2Node, N2BTopic,
    Node2Bridge,
};
use cortex_m::peripheral::NVIC;
use embassy_executor::Spawner;
use embassy_nrf::{
    bind_interrupts, config::{Config, HfclkSource}, interrupt, pac::{Interrupt, FICR}, peripherals::{self, PWM0, TWISPI0}, pwm::{self, Prescaler, SequenceConfig, SequenceLoad, SequencePwm, SingleSequenceMode, SingleSequencer}, twim::{self, Twim}
};
use embassy_nrf::{
    gpio::{Level, Output, OutputDrive},
    pac::RADIO,
};
use embassy_time::{Delay, Duration, Instant, Ticker, Timer, WithTimeout};
use esb::{
    bbq2::queue::BBQueue,
    irq::StatePTX,
    peripherals::{PtrTimer, Timer0},
    Addresses, ConfigBuilder, Error, EsbApp, EsbBuffer, EsbHeader, EsbIrq, IrqTimer,
};
use impls::{EsbRx, EsbTx};
use libscd::asynchronous::scd4x::Scd41;
use mutex::{raw_impls::cs::CriticalSectionRawMutex, BlockingMutex};
use postcard_rpc::server::{Dispatch, Sender, Server};
use scd41_node_icd::{Scd41Reading, ScdReadingTopic};
use smart_leds::{colors, gamma};
use smartled::{BUF_CT, LED_CT, RGB8};
use static_cell::{ConstStaticCell, StaticCell};
use storage::write_message;

const MAX_PAYLOAD_SIZE: u8 = 64;

type IrqStorage = BlockingMutex<CriticalSectionRawMutex, EsbIrq<1024, 1024, Timer0, StatePTX>>;
static ESB_IRQ: StaticCell<IrqStorage> = StaticCell::new();
static IRQ_PTR: AtomicPtr<IrqStorage> = AtomicPtr::new(null_mut());

type TimerStorage = BlockingMutex<CriticalSectionRawMutex, IrqTimer<Timer0>>;
static TIM_IRQ: StaticCell<TimerStorage> = StaticCell::new();
static TIM_PTR: AtomicPtr<TimerStorage> = AtomicPtr::new(null_mut());

bind_interrupts!(pub struct Irqs {
    TWISPI0 => twim::InterruptHandler<peripherals::TWISPI0>;
});

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mut c = Config::default();
    c.hfclk_source = HfclkSource::ExternalXtal;
    let p = embassy_nrf::init(c);
    let mut cp = cortex_m::Peripherals::take().unwrap();
    let led = Output::new(p.P0_13, Level::Low, OutputDrive::Standard);

    static BUFFER: EsbBuffer<1024, 1024> = EsbBuffer {
        app_to_radio_buf: BBQueue::new(),
        radio_to_app_buf: BBQueue::new(),
        timer_flag: AtomicBool::new(false),
    };
    let addresses = Addresses::default();

    // a 256 byte packet at 2mbps is 1ms-ish.
    //
    // Let's wait 1.5x this time for an ack, and 10x this for
    // a retry, because if there was a collision, we want to give enough
    // time for someone else to succeed. We should probably randomize this
    // either here or in the lib to avoid multiple devices fighting.

    let config = ConfigBuilder::default()
        .tx_power(embassy_nrf::radio::TxPower::POS8_DBM)
        .maximum_transmit_attempts(32)
        .retransmit_delay(10_000)
        .wait_for_ack_timeout(1500)
        .max_payload_size(252)
        .check()
        .unwrap();
    let (mut esb_app, esb_irq, esb_timer) = BUFFER
        .try_split(unsafe { Timer0::take() }, RADIO, addresses, config)
        .unwrap();

    let esb_irq = esb_irq.into_ptx();
    {
        let irq_ref = ESB_IRQ.init(BlockingMutex::new(esb_irq));
        IRQ_PTR.store(irq_ref, Ordering::Release);
    }
    {
        let tim_ref = TIM_IRQ.init(BlockingMutex::new(esb_timer));
        TIM_PTR.store(tim_ref, Ordering::Release);
    }
    unsafe {
        cp.NVIC.set_priority(Interrupt::TIMER0, 0);
        cp.NVIC.set_priority(Interrupt::RADIO, 0);
        NVIC::unmask(Interrupt::TIMER0);
        NVIC::unmask(Interrupt::RADIO);
    }
    let serial = get_unique_id();
    // defmt::info!("Getting addr pipe");
    let pipe = get_pipe(&mut esb_app, serial).await;
    // defmt::info!("Got pipe addr {=u8}", pipe);

    let (tx, rx) = esb_app.split();
    let esb_tx = EsbTx::new(tx, serial, pipe);
    let esb_rx = EsbRx::new(rx, serial, pipe);

    spawner.must_spawn(keepalive(esb_tx.clone()));

    // ///////////
    // ESB/RPC INIT
    // ///////////

    // P0.02: Dial or button?
    // P0.28: Dial or button?
    // P0.29: Dial or button?
    // P0.05: Dial or button?

    // P0.04: Green LED
    // P0.03: Red LED

    let pbufs = app::PBUFS.take();
    let context = app::Context {
        unique_id: get_unique_id(),
        led,
    };
    let dispatcher = app::MyApp::new(context, spawner.into());
    let vkk = dispatcher.min_key_len();
    let mut server: app::AppServer =
        Server::new(esb_tx, esb_rx, pbufs.rx_buf.as_mut_slice(), dispatcher, vkk);
    // let prpc_sender = server.sender();

    let config = twim::Config::default();
    let twi = Twim::new(p.TWISPI0, Irqs, p.P1_14, p.P1_15, config);
    let scd = Scd41::new(twi, embassy_time::Delay);

    let mut config = pwm::Config::default();
    config.sequence_load = SequenceLoad::Common;
    config.prescaler = Prescaler::Div1;
    config.max_duty = 20; // 1.25us (1s / 16Mhz * 20)
    let pwm = SequencePwm::new_1ch(p.PWM0, p.P1_13, config).unwrap();

    // spawner.must_spawn(rgb(pwm));
    spawner.must_spawn(sensor_task(scd, server.sender(), pwm));

    // Begin running!
    loop {
        // If the host disconnects, we'll return an error here.
        // If this happens, just wait until the host reconnects
        let _ = server.run().await;
        // defmt::info!("I/O error");
        Timer::after_millis(100).await;
    }

    // loop {
    //     match esb_rx.receive(&mut buf).await {
    //         Ok(b) => {
    //             defmt::info!("Got proxy packet");
    //             let res = VarHeader::take_from_slice(b).unwrap();
    //             assert_eq!(res.0.key, VarKey::Key8(DummyTopic::TOPIC_KEY));
    //             let res = postcard::from_bytes::<Dummy>(res.1).unwrap();
    //             defmt::info!("Decoded packet successfully: {:?}", res.data);
    //         }
    //         Err(_e) => {
    //             defmt::info!("RX error");
    //         }
    //     }
    // }
}

// #[embassy_executor::task]
// pub async fn rgb(
//     mut pwm: SequencePwm<'static, PWM0>,
// ) {
//     const T1H: u16 = 0x8000 | 13; // Duty = 13/20 ticks (0.8us/1.25us) for a 1
//     const T0H: u16 = 0x8000 | 7; // Duty 7/20 ticks (0.4us/1.25us) for a 0
//     const RES: u16 = 0x8000;

//     // Declare the bits of 24 bits in a buffer we'll be
//     // mutating later.
//     let mut seq_words = [
//         T0H, T0H, T0H, T0H, T0H, T0H, T0H, T0H, // G
//         T0H, T0H, T0H, T0H, T0H, T0H, T0H, T0H, // R
//         T1H, T1H, T1H, T1H, T1H, T1H, T1H, T1H, // B
//         RES,
//     ];
//     let mut seq_config = SequenceConfig::default();
//     seq_config.end_delay = 799; // 50us (20 ticks * 40) - 1 tick because we've already got one RES;

//     let mut color_bit = 16;
//     let mut bit_value = T0H;

//     loop {
//         let sequences = SingleSequencer::new(&mut pwm, &seq_words, seq_config.clone());
//         sequences.start(SingleSequenceMode::Times(1)).unwrap();

//         Timer::after_millis(50).await;

//         if bit_value == T0H {
//             if color_bit == 20 {
//                 bit_value = T1H;
//             } else {
//                 color_bit += 1;
//             }
//         } else {
//             if color_bit == 16 {
//                 bit_value = T0H;
//             } else {
//                 color_bit -= 1;
//             }
//         }

//         drop(sequences);

//         seq_words[color_bit] = bit_value;
//     }
// }

#[embassy_executor::task]
pub async fn sensor_task(
    mut scd: Scd41<Twim<'static, TWISPI0>, Delay>,
    sender: Sender<AppTx>,
    mut pwm: SequencePwm<'static, PWM0>,
) {
    static RGB_BUF: ConstStaticCell<[RGB8; LED_CT]> =
        ConstStaticCell::new([const { RGB8 { r: 0, g: 0, b: 0 } }; LED_CT]);
    static DATA_BUF: ConstStaticCell<[u16; BUF_CT]> = ConstStaticCell::new([0u16; BUF_CT]);
    let rgb_buf = RGB_BUF.take();
    let data_buf = DATA_BUF.take();


    // When re-programming, the controller will be restarted,
    // but not the sensor. We try to stop it in order to
    // prevent the rest of the commands failing.
    _ = scd.stop_periodic_measurement().await;

    if let Err(_e) = scd.start_periodic_measurement().await {
        // defmt::panic!("Failed to start periodic measurement: {:?}", e);
    }

    // let mut color = 0u8;
    let mut ctr = 0u16;
    let mut iir = 0.0f32;

    // tick every 100ms, 5000ms typical update
    let ratio = 100.0f32 / 5000.0f32;
    let mut ticker = Ticker::every(Duration::from_millis(100));
    let mut last = 0.0f32;
    loop {
        ticker.next().await;
        ctr = ctr.wrapping_add(1);
        if scd.data_ready().await.unwrap() {
            let m = scd.read_measurement().await.unwrap();
            let reading = Scd41Reading {
                temp_c: m.temperature,
                humi_pct: m.humidity,
                co2_ppm: m.co2,
            };
            last = m.co2 as f32;

            let _ = sender
                .publish::<ScdReadingTopic>(VarSeq::Seq2(ctr), &reading)
                .await;
        }


        fn lerp(min: u16, max: u16, act: u16, minc: smart_leds::RGB8, maxc: smart_leds::RGB8) -> smart_leds::RGB8 {
            let act = act.clamp(min, max);
            let range = (max - min) as f32;
            let offset = (act - min) as f32;
            let pct = offset / range;

            let shift = |l: u8, r: u8| {
                let a = (r as f32) * pct;
                let b = (l as f32) * (1.0 - pct);
                (a + b) as u8
            };

            smart_leds::RGB8 {
                r: shift(minc.r, maxc.r),
                g: shift(minc.g, maxc.g),
                b: shift(minc.b, maxc.b),
            }
        }

        // Update iir filter
        iir = (iir * (1.0 - ratio)) + (last * ratio);
        let iir_now = iir as u16;

        let color = match iir_now {
            0..400 => lerp(0, 400, iir_now, colors::BLACK, colors::PURPLE),
            400..600 => lerp(400, 600, iir_now, colors::PURPLE, colors::BLUE),
            600..800 => lerp(600, 800, iir_now, colors::BLUE, colors::GREEN),
            800..1000 => lerp(800, 1000, iir_now, colors::GREEN, colors::YELLOW),
            1000..1200 => lerp(1000, 1200, iir_now, colors::YELLOW, colors::ORANGE),
            1200..1400 => lerp(1200, 1400, iir_now, colors::ORANGE, colors::RED),
            1400.. => lerp(1400, 2000, iir_now, colors::RED, colors::WHITE),
        };
        let color = gamma([color].into_iter()).next().unwrap();
        rgb_buf[0] = RGB8 { r: color.r, g: color.g, b: color.b };
        smartled::smartled(&mut pwm, rgb_buf, data_buf).await;
    }
}

#[embassy_executor::task]
async fn keepalive(esb_tx: EsbTx) {
    // TODO: some kind of jitter?
    let mut ticker = Ticker::every(Duration::from_millis(100));
    let mut last_ka = Instant::now();
    loop {
        ticker.next().await;
        if last_ka.elapsed() >= Duration::from_secs(3) {
            esb_tx.send_keepalive().await;
            last_ka = Instant::now();
        } else {
            esb_tx.send_nop().await;
        }
    }
}

async fn get_pipe(esb_app: &mut EsbApp<1024, 1024>, unique_id: u64) -> u8 {
    let mut ctr = 0u16;
    let mut pids = (0..4).cycle();
    loop {
        let pid = pids.next().unwrap();
        ctr = ctr.wrapping_add(1);

        let esb_header = EsbHeader::build()
            .max_payload(MAX_PAYLOAD_SIZE)
            .pid(pid)
            .pipe(0)
            .no_ack(false)
            .check()
            .unwrap();

        // defmt::info!("Sending Init, tx: {=u32}, err: {=u32}", ct_tx, ct_err);

        let mut packet = esb_app.grant_packet(esb_header).unwrap();
        let msg = Node2Bridge::Initialize {
            serial: unique_id.to_le_bytes(),
        };
        let used = write_topic2::<N2BTopic>(&msg, VarSeq::Seq2(ctr), &mut packet).unwrap();
        packet.commit(used);
        esb_app.start_tx();

        // Did we receive any packet ?
        let fut = esb_app.wait_read_packet();
        let tofut = fut.with_timeout(Duration::from_secs(1));
        if let Ok(response) = tofut.await {
            if let Some(extract) = extract_topic2::<B2NTopic>(&response) {
                match extract.msg {
                    Bridge2Node::InitializeAck { serial, use_pipe } => {
                        if serial == unique_id.to_le_bytes() {
                            response.release();
                            return use_pipe;
                        }
                    }
                    Bridge2Node::Keepalive { .. } => {}
                    Bridge2Node::Proxy { .. } => {}
                    Bridge2Node::Reset => {}
                }
            }
            Timer::after_millis(250).await;
            response.release();
        }
    }
}

#[interrupt]
fn RADIO() {
    let ptr = IRQ_PTR.load(Ordering::Relaxed);
    let r = unsafe { ptr.as_ref() }.unwrap();
    let s = r.with_lock(|state| match state.radio_interrupt() {
        Ok(s) => Some(s),
        Err(Error::MaximumAttempts) => None,
        Err(_e) => panic!(),
    });
    if let Some(_s) = s {
        // match s {
        //     StatePTX::IdleTx => defmt::info!("IdleTx"),
        //     StatePTX::TransmitterTx => defmt::info!("TransmitterTx"),
        //     StatePTX::TransmitterTxNoAck => defmt::info!("TransmitterTxNoAck"),
        //     StatePTX::TransmitterWaitAck => defmt::info!("TransmitterWaitAck"),
        //     StatePTX::TransmitterWaitRetransmit => defmt::info!("TransmitterWaitRetransmit"),
        // }
    } else {
        // defmt::info!("MAX ATTEMPTS");
    }
}

#[interrupt]
fn TIMER0() {
    let ptr = TIM_PTR.load(Ordering::Relaxed);
    let r = unsafe { ptr.as_ref() }.unwrap();
    r.with_lock(|state| {
        state.timer_interrupt();
    });
}

fn get_unique_id() -> u64 {
    let lower = FICR.deviceid(0).read() as u64;
    let upper = FICR.deviceid(1).read() as u64;
    (upper << 32) | lower
}

use core::panic::PanicInfo;
#[panic_handler]
fn panic_handler(info: &PanicInfo<'_>) -> ! {
    let mut buf = [0u8; 512];
    critical_section::with(|_cs| {
        let mut writer = SliWrite {
            remain: &mut buf,
            written: 0,
            overflow: false,
        };
        writeln!(&mut writer, "{info}").ok();
        let len = writer.written;
        write_message(&BootMessage::BootPanicked {
            uptime: Instant::now().as_ticks(),
            reason: &buf[..len],
        });
        cortex_m::peripheral::SCB::sys_reset();
    });
    // Unreachable
    unreachable!()
}

struct SliWrite<'a> {
    remain: &'a mut [u8],
    written: usize,
    overflow: bool,
}

/// Internal Write implementation to output the formatted panic string into RAM
impl Write for SliWrite<'_> {
    fn write_str(&mut self, s: &str) -> Result<(), core::fmt::Error> {
        if !self.remain.is_empty() {
            // Get the data about the string that is being written now
            let data = s.as_bytes();

            // Take what we can from the input
            let len = data.len().min(self.remain.len());
            self.remain[..len].copy_from_slice(&data[..len]);

            // shrink the buffer to the remaining free space
            let window = core::mem::take(&mut self.remain);
            let (_now, later) = window.split_at_mut(len);
            self.remain = later;

            // Update tracking data
            self.overflow |= len < data.len();
            self.written += len;
        }

        Ok(())
    }
}
