#![no_std]
#![no_main]

pub mod app;
pub mod handlers;
pub mod impls;
pub mod storage;

use core::{
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use bridge_icd::{
    extract_topic2, postcard_rpc::header::VarSeq, write_topic2, B2NTopic, Bridge2Node, N2BTopic,
    Node2Bridge,
};
use cortex_m::peripheral::NVIC;
use embassy_executor::Spawner;
use embassy_nrf::{
    config::{Config, HfclkSource},
    interrupt,
    pac::{Interrupt, FICR},
};
use embassy_nrf::{
    gpio::{Level, Output, OutputDrive},
    pac::RADIO,
};
use embassy_time::{Duration, Instant, Ticker, Timer, WithTimeout};
use esb::{
    bbq2::queue::BBQueue,
    irq::StatePTX,
    peripherals::{PtrTimer, Timer0},
    Addresses, ConfigBuilder, Error, EsbApp, EsbBuffer, EsbHeader, EsbIrq, IrqTimer,
};
use impls::{EsbRx, EsbTx};
use mutex::{raw_impls::cs::CriticalSectionRawMutex, BlockingMutex};
use postcard_rpc::server::{Dispatch, Server};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

const MAX_PAYLOAD_SIZE: u8 = 64;

type IrqStorage = BlockingMutex<CriticalSectionRawMutex, EsbIrq<1024, 1024, Timer0, StatePTX>>;
static ESB_IRQ: StaticCell<IrqStorage> = StaticCell::new();
static IRQ_PTR: AtomicPtr<IrqStorage> = AtomicPtr::new(null_mut());

type TimerStorage = BlockingMutex<CriticalSectionRawMutex, IrqTimer<Timer0>>;
static TIM_IRQ: StaticCell<TimerStorage> = StaticCell::new();
static TIM_PTR: AtomicPtr<TimerStorage> = AtomicPtr::new(null_mut());

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
    defmt::info!("Getting addr pipe");
    let pipe = get_pipe(&mut esb_app, serial).await;
    defmt::info!("Got pipe addr {=u8}", pipe);

    let (tx, rx) = esb_app.split();
    let esb_tx = EsbTx::new(tx, serial, pipe);
    let esb_rx = EsbRx::new(rx, serial, pipe);

    spawner.must_spawn(keepalive(esb_tx.clone()));

    // ///////////
    // ESB/RPC INIT
    // ///////////
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

    // Begin running!
    loop {
        // If the host disconnects, we'll return an error here.
        // If this happens, just wait until the host reconnects
        let _ = server.run().await;
        defmt::info!("I/O error");
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
    let mut ct_tx: u32 = 0;
    let mut ct_err: u32 = 0;
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

        defmt::info!("Sending Init, tx: {=u32}, err: {=u32}", ct_tx, ct_err);

        ct_tx += 1;
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
        } else if ct_tx != 0 {
            ct_err += 1;
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
        defmt::info!("MAX ATTEMPTS");
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
