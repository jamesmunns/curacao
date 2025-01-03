use embassy_nrf::{peripherals::PWM0, pwm::{
    SequenceConfig, SequencePwm, SingleSequenceMode, SingleSequencer,
}};
use embassy_time::Timer;
use node_icd::RGB8;
use static_cell::ConstStaticCell;

// WS2812B LED light demonstration. Drives just one light.
// The following reference on WS2812B may be of use:
// https://cdn-shop.adafruit.com/datasheets/WS2812B.pdf.
// This demo lights up a single LED in blue. It then proceeds
// to pulsate the LED rapidly.
//
// /!\ NOTE FOR nRF52840-DK users /!\
//
// If you're using the nRF52840-DK, the default "Vdd" power source
// will set the GPIO I/O voltage to 3.0v, using the onboard regulator.
// This can sometimes not be enough to drive the WS2812B signal if you
// are not using an external level shifter. If you set the board to "USB"
// power instead (and provide power via the "nRF USB" connector), the board
// will instead power the I/Os at 3.3v, which is often enough (but still
// out of official spec) for the WS2812Bs to work properly.

// In the following declarations, setting the high bit tells the PWM
// to reverse polarity, which is what the WS2812B expects.

const T1H: u16 = 0x8000 | 13; // Duty = 13/20 ticks (0.8us/1.25us) for a 1
const T0H: u16 = 0x8000 | 7; // Duty 7/20 ticks (0.4us/1.25us) for a 0
pub const RES: u16 = 0x8000;

pub const LED_CT: usize = 120;
pub const BUF_CT: usize = 24 * LED_CT + 1;

pub static PWM_BUF: ConstStaticCell<[u16; BUF_CT]> = ConstStaticCell::new([0u16; BUF_CT]);

fn fill(rgb: &[RGB8; LED_CT], buf: &mut [u16; BUF_CT]) {
    for (RGB8 { r, g, b }, ch) in rgb.iter().zip(buf.chunks_mut(24)) {
        for (c, ch) in [r, g, b].iter().zip(ch.chunks_mut(8)) {
            let mut c = **c;
            for w in ch.iter_mut() {
                *w = if c & 0x80 != 0 {
                    T1H
                } else {
                    T0H
                };
                c <<= 1;
            }
        }
    }
}

// Provides data to a WS2812b (Neopixel) LED and makes it go blue. The data
// line is assumed to be P1_05.
pub async fn smartled(
    pwm: &mut SequencePwm<'static, PWM0>,
    rgb_buf: &[RGB8; LED_CT],
    data_buf: &mut [u16; BUF_CT],
) {
    let mut seq_config = SequenceConfig::default();
    seq_config.end_delay = 799; // 50us (20 ticks * 40) - 1 tick because we've already got one RES;

    fill(rgb_buf, data_buf);

    let sequences = SingleSequencer::new(pwm, data_buf, seq_config.clone());
    sequences.start(SingleSequenceMode::Times(1)).unwrap();
    Timer::after_micros((30 * LED_CT + 200) as u64).await;
}
