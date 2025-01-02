#![no_std]
#![no_main]

use app::AppTx;
use bridge::{Bridge, SMutex, FRAG_BUFS};
use core::{
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};
use cortex_m::peripheral::NVIC;
use cortex_m_rt::interrupt;
use defmt::info;
use embassy_executor::Spawner;
use embassy_nrf::{
    bind_interrupts,
    config::{Config as NrfConfig, HfclkSource, LfclkSource},
    gpio::{Level, Output, OutputDrive},
    interrupt::{self, Priority},
    pac::{Interrupt, FICR},
    peripherals::USBD,
    usb::{self, vbus_detect::HardwareVbusDetect},
};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Ticker, Timer};
use embassy_usb::{Config, UsbDevice};
use esb::{
    app::{EsbAppReceiver, EsbAppSender},
    bbq2::queue::BBQueue,
    irq::StatePRX,
    peripherals::{PtrTimer, Timer0},
    Addresses, ConfigBuilder, Error, EsbBuffer, EsbIrq, IrqTimer,
};
use mutex::{raw_impls::cs::CriticalSectionRawMutex, BlockingMutex};
use postcard_rpc::{
    sender_fmt,
    server::{Dispatch, Sender, Server},
};
use static_cell::{ConstStaticCell, StaticCell};
use table::Table;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(pub struct Irqs {
    USBD => usb::InterruptHandler<USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
});

const MAX_PAYLOAD_SIZE: u8 = 252;

type IrqStorage = BlockingMutex<CriticalSectionRawMutex, EsbIrq<1024, 1024, Timer0, StatePRX>>;
static ESB_IRQ: StaticCell<IrqStorage> = StaticCell::new();
static IRQ_PTR: AtomicPtr<IrqStorage> = AtomicPtr::new(null_mut());

type TimerStorage = BlockingMutex<CriticalSectionRawMutex, IrqTimer<Timer0>>;
static TIM_IRQ: StaticCell<TimerStorage> = StaticCell::new();
static TIM_PTR: AtomicPtr<TimerStorage> = AtomicPtr::new(null_mut());

pub mod app;
pub mod bridge;
pub mod handlers;
pub mod table;
pub mod storage;

fn usb_config(serial: &'static str) -> Config<'static> {
    let mut config = Config::new(0x16c0, 0x27DD);
    config.manufacturer = Some("OneVariable");
    config.product = Some("poststation-nrf");
    config.serial_number = Some(serial);

    // Required for windows compatibility.
    // https://developer.nordicsemi.com/nRF_Connect_SDK/doc/1.9.1/kconfig/CONFIG_CDC_ACM_IAD.html#help
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;
    config.composite_with_iads = true;

    config
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // SYSTEM INIT
    info!("Start");
    let mut config = NrfConfig::default();
    config.hfclk_source = HfclkSource::ExternalXtal;
    config.time_interrupt_priority = Priority::P1;
    config.gpiote_interrupt_priority = Priority::P1;
    config.lfclk_source = LfclkSource::ExternalXtal;
    let p = embassy_nrf::init(config);

    // Obtain the device ID
    let unique_id = get_unique_id();

    static SERIAL_STRING: StaticCell<[u8; 16]> = StaticCell::new();
    let mut ser_buf = [b' '; 16];
    // This is a simple number-to-hex formatting
    unique_id
        .to_be_bytes()
        .iter()
        .zip(ser_buf.chunks_exact_mut(2))
        .for_each(|(b, chs)| {
            let mut b = *b;
            for c in chs {
                *c = match b >> 4 {
                    v @ 0..10 => b'0' + v,
                    v @ 10..16 => b'A' + (v - 10),
                    _ => b'X',
                };
                b <<= 4;
            }
        });
    let ser_buf = SERIAL_STRING.init(ser_buf);
    let ser_buf = core::str::from_utf8(ser_buf.as_slice()).unwrap();

    // ///////////
    // ESB INIT
    // ///////////
    let mut cp = cortex_m::Peripherals::take().unwrap();

    static BUFFER: EsbBuffer<1024, 1024> = EsbBuffer {
        app_to_radio_buf: BBQueue::new(),
        radio_to_app_buf: BBQueue::new(),
        timer_flag: AtomicBool::new(false),
    };
    let addresses = Addresses::default();
    let config = ConfigBuilder::default()
        .maximum_transmit_attempts(1)
        .max_payload_size(MAX_PAYLOAD_SIZE)
        .check()
        .unwrap();
    let (esb_app, esb_irq, esb_timer) = BUFFER
        .try_split(
            unsafe { Timer0::take() },
            embassy_nrf::pac::RADIO,
            addresses,
            config,
        )
        .unwrap();

    let mut esb_irq = esb_irq.into_prx();
    esb_irq.start_receiving().unwrap();
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
        cp.NVIC.set_priority(Interrupt::USBD, 1);
        NVIC::unmask(Interrupt::TIMER0);
        NVIC::unmask(Interrupt::RADIO);
    }

    static ESB_SENDER: StaticCell<Mutex<ThreadModeRawMutex, EsbAppSender<1024>>> =
        StaticCell::new();
    static TABLE: ConstStaticCell<Mutex<ThreadModeRawMutex, Table>> =
        ConstStaticCell::new(Mutex::new(Table::new()));
    let (tx, rx) = esb_app.split();
    let esb_sender = bridge::Sender {
        sender: ESB_SENDER.init(Mutex::new(tx)),
    };
    let table = TABLE.take();

    // ///////////
    // USB/RPC INIT
    // ///////////
    let driver = usb::Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
    let pbufs = app::PBUFS.take();
    let config = usb_config(ser_buf);
    let led = Output::new(p.P0_13, Level::Low, OutputDrive::Standard);

    let context = app::Context {
        unique_id,
        led,
        esb_sender: esb_sender.clone(),
        table,
    };

    let (device, tx_impl, rx_impl) =
        app::STORAGE.init_poststation(driver, config, pbufs.tx_buf.as_mut_slice());
    let dispatcher = app::MyApp::new(context, spawner.into());
    let vkk = dispatcher.min_key_len();
    let mut server: app::AppServer = Server::new(
        tx_impl,
        rx_impl,
        pbufs.rx_buf.as_mut_slice(),
        dispatcher,
        vkk,
    );
    let sender = server.sender();

    // We need to spawn the USB task so that USB messages are handled by
    // embassy-usb
    spawner.must_spawn(radio_prx(sender.clone(), esb_sender, table, rx));
    spawner.must_spawn(usb_task(device));
    spawner.must_spawn(logging_task(sender));

    // Begin running!
    loop {
        // If the host disconnects, we'll return an error here.
        // If this happens, just wait until the host reconnects
        let _ = server.run().await;
        defmt::info!("I/O error");
        Timer::after_millis(100).await;
    }
}

/// This handles the low level USB management
#[embassy_executor::task]
pub async fn usb_task(mut usb: UsbDevice<'static, app::AppDriver>) {
    usb.run().await;
}

/// This task is a "sign of life" logger
#[embassy_executor::task]
pub async fn logging_task(sender: Sender<AppTx>) {
    let mut ticker = Ticker::every(Duration::from_secs(3));
    let start = Instant::now();
    loop {
        ticker.next().await;
        let _ = sender_fmt!(sender, "Uptime: {:?}", start.elapsed()).await;
    }
}

fn get_unique_id() -> u64 {
    let lower = FICR.deviceid(0).read() as u64;
    let upper = FICR.deviceid(1).read() as u64;
    (upper << 32) | lower
}

#[embassy_executor::task]
async fn radio_prx(
    sender: Sender<AppTx>,
    esb_sender: bridge::Sender<1024>,
    table: SMutex<Table>,
    recv: EsbAppReceiver<1024>,
) {
    let mut bridge: Bridge<1024, 1024> = Bridge {
        table,
        esb_sender,
        recv,
        prpc_sender: sender,
        table_ctr: 0,
        proxy_ctr: 0,
        frag_bufs: FRAG_BUFS.take(),
    };
    bridge.run().await;
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
