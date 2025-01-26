#![no_std]
#![no_main]

use app::{AppServer, AppTx, BUFSZ};
use defmt::info;
use embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice;
use embassy_executor::Spawner;
use embassy_nrf::{
    bind_interrupts,
    config::{Config as NrfConfig, HfclkSource},
    gpio::{Input, Level, Output, OutputDrive, Pull},
    pac::FICR,
    peripherals::{TWISPI0, USBD},
    spim::{self, Spim},
    usb::{self, vbus_detect::HardwareVbusDetect},
};
use embassy_sync::{
    blocking_mutex::raw::ThreadModeRawMutex,
    mutex::Mutex,
};
use embassy_time::{Delay, Duration, Instant, Ticker, Timer};
use embassy_usb::{Config, UsbDevice};
use epd_waveshare::{epd4in2::Epd4in2, prelude::WaveshareDisplay};
use postcard_rpc::{
    sender_fmt,
    server::{Dispatch, Sender, Server},
};
use static_cell::{ConstStaticCell, StaticCell};

bind_interrupts!(pub struct Irqs {
    USBD => usb::InterruptHandler<USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
    TWISPI0 => spim::InterruptHandler<TWISPI0>;
});

use {defmt_rtt as _, panic_probe as _};

pub mod app;
pub mod handlers;

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
    let p = embassy_nrf::init(Default::default());
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

    // Epaper pins
    let din = p.P1_01;
    let clk = p.P1_02;
    let csn = p.P1_03;
    let dcx = p.P1_04;
    let rst = p.P1_05;
    let bsy = p.P1_06;
    let mut spimcfg = spim::Config::default();
    spimcfg.frequency = spim::Frequency::M8;

    static SPI: StaticCell<Mutex<ThreadModeRawMutex, Spim<'static, TWISPI0>>> = StaticCell::new();
    let spi = SPI.init(Mutex::new(Spim::new_txonly(
        p.TWISPI0,
        Irqs,
        clk,
        din,
        spim::Config::default(),
    )));

    let mut spi = SpiDevice::new(spi, Output::new(csn, Level::High, OutputDrive::Standard));

    let mut display = Epd4in2::new(
        &mut spi,
        Input::new(bsy, Pull::None),
        Output::new(dcx, Level::High, OutputDrive::Standard),
        Output::new(rst, Level::High, OutputDrive::Standard),
        &mut Delay,
        None,
    )
    .await
    .unwrap();

    // put the display to sleep
    display.sleep(&mut spi).await.unwrap();

    // USB/RPC INIT
    let driver = usb::Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
    let pbufs = app::PBUFS.take();
    let config = usb_config(ser_buf);
    let led = Output::new(p.P0_13, Level::Low, OutputDrive::Standard);

    static BUFFER: ConstStaticCell<[u8; BUFSZ]> = ConstStaticCell::new([0u8; BUFSZ]);
    let dispbuf = BUFFER.take();

    let context = app::Context {
        unique_id,
        led,
        spi,
        dispbuf,
        display,
        next_draw: Instant::now() + Duration::from_secs(15),
    };

    let (device, tx_impl, rx_impl) =
        app::STORAGE.init_poststation(driver, config, pbufs.tx_buf.as_mut_slice());
    let dispatcher = app::MyApp::new(context, spawner.into());
    let vkk = dispatcher.min_key_len();
    let server: app::AppServer = Server::new(
        tx_impl,
        rx_impl,
        pbufs.rx_buf.as_mut_slice(),
        dispatcher,
        vkk,
    );
    let sender = server.sender();
    // We need to spawn the USB task so that USB messages are handled by
    // embassy-usb
    spawner.must_spawn(usb_task(device));
    spawner.must_spawn(logging_task(sender));
    spawner.must_spawn(run_server(server));
}

#[embassy_executor::task]
async fn run_server(mut server: AppServer) {
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
