use std::{io::Write, sync::atomic::{AtomicU32, Ordering}};

use bootloader_icd::{AppPartitionInfo, GetAppFlashInfoEndpoint};
use postcard_rpc::Endpoint;
// use bootloader_icd::{Position, Rgb8, SetRgbLed, SetRgbLedEndpoint, SwitchStateTopic};
use poststation_sdk::{connect, SquadClient, StreamListener};
use rand::Rng;
use serde::{de::DeserializeOwned, Serialize};
use smart_leds::hsv::{hsv2rgb, Hsv};

struct Bootloader {
    serial: u64,
    client: SquadClient,
    ctr: AtomicU32,
}

impl Bootloader {
    pub fn new(client: SquadClient, serial: u64) -> Self {
        Self {
            serial,
            client,
            ctr: AtomicU32::new(0),
        }
    }

    #[inline(always)]
    fn ctr(&self) -> u32 {
        self.ctr.fetch_add(1, Ordering::Relaxed)
    }

    async fn proxy_ep<E>(&self, req: &E::Request) -> Result<E::Response, String>
    where
        E: Endpoint,
        E::Request: Serialize,
        E::Response: DeserializeOwned,
    {
        self.client.proxy_endpoint::<E>(self.serial, self.ctr(), req).await
    }

    async fn partinfo(&self) -> Result<AppPartitionInfo, String> {
        self.proxy_ep::<GetAppFlashInfoEndpoint>(&()).await
    }

    // async fn all_black(&self) -> Result<(), String> {
    //     const ALL_POS: [Position; 3] = [Position::One, Position::Two, Position::Three];
    //     for pos in ALL_POS {
    //         self.set_black(pos).await?;
    //     }
    //     Ok(())
    // }

    // async fn set_black(&self, position: Position) -> Result<(), String> {
    //     self.client
    //         .proxy_endpoint::<SetRgbLedEndpoint>(
    //             self.serial,
    //             self.ctr(),
    //             &SetRgbLed {
    //                 position,
    //                 color: Rgb8 { r: 0, g: 0, b: 0 },
    //             },
    //         )
    //         .await
    // }

    // async fn set_random_color(&self, position: Position) -> Result<(), String> {
    //     let mut rng = rand::thread_rng();
    //     let hue = rng.gen::<u8>();
    //     // bias saturation closer to 1.0 to pick more colors than white
    //     let sat = 1.0f32 - (rng.gen_range(0.0f32..1.0f32).powf(2.0));
    //     let sat = (255.0f32 * sat).round() as u8;
    //     let color = hsv2rgb(Hsv { hue, sat, val: 255 });
    //     self.client
    //         .proxy_endpoint::<SetRgbLedEndpoint>(
    //             self.serial,
    //             self.ctr(),
    //             &SetRgbLed {
    //                 position,
    //                 color: Rgb8 {
    //                     r: color.r,
    //                     g: color.g,
    //                     b: color.b,
    //                 },
    //             },
    //         )
    //         .await
    // }

    // async fn subscribe_switches(&self) -> Result<StreamListener<SwitchStateTopic>, String> {
    //     self.client
    //         .stream_topic::<SwitchStateTopic>(self.serial)
    //         .await
    // }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    const SERIAL: u64 = 0xB55E43E32A752E08;
    let client = connect("localhost:51837").await;
    let bl = Bootloader::new(client, SERIAL);
    // keyboard.all_black().await?;

    // let mut sub = keyboard.subscribe_switches().await?;

    // while let Some(val) = sub.recv().await {
    //     println!("Position {:?}, is_pressed: {}", val.position, val.pressed);
    //     match val.pressed {
    //         true => keyboard.set_random_color(val.position).await?,
    //         false => keyboard.set_black(val.position).await?,
    //     }
    // }

    'repl: loop {
        print!("> ");
        let _ = std::io::stdout().flush();

        let line = read_line().await;
        let tline = line.trim();
        let words = tline.split_whitespace().collect::<Vec<_>>();
        match words.as_slice() {
            ["info"] => {
                match bl.partinfo().await {
                    Ok(info) => {
                        println!("Info:");
                        println!("  * Start: {:08X} ({:0.02}KiB)", info.start, info.start as f32 / 1024.0);
                        println!("  * Len:   {:08X} ({:0.02}KiB)", info.len, info.len as f32 / 1024.0);
                        println!("  * Range: {:08X}..{:08X}", info.start, info.start + info.len);
                        println!("  * Chunk: {}", info.transfer_chunk);
                    }
                    Err(e) => {
                        println!("Error: '{e}'");
                    }
                }
            }
            other => println!("Error, unknown: '{other:?}'"),
        }
    }
}

async fn read_line() -> String {
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        line
    })
    .await
    .unwrap()
}
