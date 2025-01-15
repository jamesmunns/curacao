#![no_std]
#![no_main]

use core::{fmt::Write, panic::PanicInfo};

use bootloader_icd::scratch::BootMessage;
use cortex_m::{asm::bootload, peripheral::SCB};
use embassy_executor::Spawner;
use embassy_nrf::{
    bind_interrupts,
    config::{Config as NrfConfig, HfclkSource},
    gpio::{Input, Level, Output, OutputDrive, Pull},
    nvmc::Nvmc,
    pac::{power::regs::Resetreas, FICR, POWER},
    peripherals::USBD,
    usb::{self, vbus_detect::HardwareVbusDetect},
};
use embassy_time::{Instant, Timer};
use embassy_usb::{Config, UsbDevice};
use postcard_rpc::server::{Dispatch, Server};
use static_cell::{ConstStaticCell, StaticCell};
use storage::{
    app_sanity_check, clear_message, read_message, write_message, BOOT_FLASH_SIZE, MEM_SCRATCH_SIZE,
};

bind_interrupts!(pub struct Irqs {
    USBD => usb::InterruptHandler<USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
});

pub mod app;
pub mod handlers;
pub mod storage;

fn usb_config(serial: &'static str) -> Config<'static> {
    let mut config = Config::new(0x16c0, 0x27DD);
    config.manufacturer = Some("OneVariable");
    config.product = Some("poststation-nrfboot");
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
    // Load the boot message before we initialize the system
    static BMSG_BUF: ConstStaticCell<[u8; MEM_SCRATCH_SIZE]> =
        ConstStaticCell::new([0u8; MEM_SCRATCH_SIZE]);
    let boot_msg = read_message(BMSG_BUF.take());
    let reset_reas = POWER.resetreas().read();
    let pin_reset = reset_reas.resetpin();
    // write reasons back to clear
    POWER.resetreas().write_value(reset_reas);

    match &boot_msg {
        Some(msg) => match msg {
            BootMessage::JustBoot => {
                // yolo
                let msg = BootMessage::BootAttempted;
                if write_message(&msg) {
                    unsafe {
                        bootload(BOOT_FLASH_SIZE as *const u32);
                    }
                }
            }

            // In any of these cases, we want to stay in the bootloader
            BootMessage::StayInBootloader => {}
            BootMessage::BootAttempted => {}
            BootMessage::AppPanicked { .. } => {}
            BootMessage::BootPanicked { .. } => {}
        },
        None => {
            // Does the app look reasonable?
            let msg = BootMessage::BootAttempted;
            if !pin_reset && app_sanity_check() && write_message(&msg) {
                unsafe {
                    bootload(BOOT_FLASH_SIZE as *const u32);
                }
            }
        }
    }
    // Clear the message to avoid reading stale values
    clear_message();

    // SYSTEM INIT
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

    // USB/RPC INIT
    let driver = usb::Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
    let pbufs = app::PBUFS.take();
    let config = usb_config(ser_buf);
    let led = Output::new(p.P0_13, Level::Low, OutputDrive::Standard);
    static SCRATCH: ConstStaticCell<[u8; 4096]> = ConstStaticCell::new([0u8; 4096]);

    let context = app::Context {
        unique_id,
        led,
        buf: SCRATCH.take(),
        nvmc: Nvmc::new(p.NVMC),
        boot_message: boot_msg,
    };

    let boot_pin = Input::new(p.P0_29, Pull::Up);
    spawner.must_spawn(button_boot(boot_pin));

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
    // We need to spawn the USB task so that USB messages are handled by
    // embassy-usb
    spawner.must_spawn(usb_task(device));

    // Begin running!
    loop {
        // If the host disconnects, we'll return an error here.
        // If this happens, just wait until the host reconnects
        let _ = server.run().await;
        Timer::after_millis(100).await;
    }
}

#[embassy_executor::task]
pub async fn button_boot(mut p: Input<'static>) {
    Timer::after_secs(3).await;
    loop {
        p.wait_for_falling_edge().await;
        let msg = BootMessage::JustBoot;
        if app_sanity_check() && write_message(&msg) {
            cortex_m::interrupt::disable();
            SCB::sys_reset();
        }
    }
}

/// This handles the low level USB management
#[embassy_executor::task]
pub async fn usb_task(mut usb: UsbDevice<'static, app::AppDriver>) {
    usb.run().await;
}

fn get_unique_id() -> u64 {
    let lower = FICR.deviceid(0).read() as u64;
    let upper = FICR.deviceid(1).read() as u64;
    // As a bootloader, let's provide a different unique_id so we don't have a
    // weird device history
    !((upper << 32) | lower)
}

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
