use std::{
    env::temp_dir,
    fmt::Write,
    fs::{self, File},
    io::{Read, Write as _},
    num::ParseIntError,
    process::Command,
    str::from_utf8,
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};

use bootloader_icd::{
    scratch::BootMessage, AppPartitionInfo, BootloadEndpoint, DataChunk, EraseFlashEndpoint,
    FlashEraseCommand, FlashReadCommand, FlashWriteCommand, GetAppFlashInfoEndpoint,
    GetBootMessageEndpoint, ReadFlashEndpoint, WriteFlashEndpoint,
};
use clap::Parser;
use postcard_rpc::Endpoint;
use poststation_sdk::{connect, SquadClient};
use rand::{thread_rng, Rng, RngCore};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use tokio::time::sleep;

#[derive(Parser, Debug)]
struct Args {
    /// Bootloader serial
    #[arg(short, long)]
    boot_serial: Option<String>,

    /// Application serial
    #[arg(short, long)]
    app_serial: Option<String>,

    #[arg(short, long)]
    reset_path: Option<String>,

    #[arg(short, long)]
    reset_msg_json: Option<String>,

    elf_path: String,
}

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
        self.client
            .proxy_endpoint::<E>(self.serial, self.ctr(), req)
            .await
    }

    async fn partinfo(&self) -> Result<AppPartitionInfo, String> {
        self.proxy_ep::<GetAppFlashInfoEndpoint>(&()).await
    }

    async fn read_chunk(&self, start: u32, len: u32) -> Result<DataChunk, String> {
        self.proxy_ep::<ReadFlashEndpoint>(&FlashReadCommand { start, len })
            .await?
            .map_err(|e| format!("Error: '{e:?}'"))
    }

    async fn erase(&self, start: u32, len: u32) -> Result<(), String> {
        self.proxy_ep::<EraseFlashEndpoint>(&FlashEraseCommand {
            start,
            len,
            force: false,
        })
        .await?
        .map_err(|e| format!("Error: '{e:?}'"))
    }

    async fn write(&self, start: u32, data: &[u8]) -> Result<(), String> {
        for (i, ch) in data.chunks(512).enumerate() {
            let addr = start + (i as u32 * 512);
            let res = self
                .proxy_ep::<WriteFlashEndpoint>(&FlashWriteCommand {
                    start: addr,
                    data: ch.to_vec(),
                    force: false,
                })
                .await?;
            if let Err(e) = res {
                return Err(format!("Error: '{e:?}'"));
            }
        }
        Ok(())
    }

    async fn boot_msg(&self) -> Result<Option<BootMessage>, String> {
        self.proxy_ep::<GetBootMessageEndpoint>(&()).await
    }

    async fn boot(&self) -> Result<(), String> {
        self.proxy_ep::<BootloadEndpoint>(&())
            .await?
            .map_err(|_| "Err: Failed Sanity Check".into())
    }

    async fn dumpfmt(&self, start: u32, len: u32, chunk: u32) -> Result<String, String> {
        let mut out = String::new();
        let mut addr = start;
        let end = start + len;
        while addr < end {
            let take = (end - addr).min(chunk);
            let Ok(data) = self.read_chunk(addr, take).await else {
                return Err("Error getting data".into());
            };
            for (i, ch) in data.data.chunks(16).enumerate() {
                let base = addr + (i as u32 * 16);
                write!(&mut out, "0x{:08X} |", base).ok();
                for b in ch {
                    write!(&mut out, " {b:02X}").ok();
                }
                for _ in 0..(16 - ch.len()) {
                    write!(&mut out, "   ").ok();
                }
                write!(&mut out, " | ").ok();
                for b in ch {
                    if b.is_ascii() && !b.is_ascii_control() {
                        write!(&mut out, "{}", *b as char).ok();
                    } else {
                        write!(&mut out, "·").ok();
                    }
                }
                out += "\n";
            }

            addr += take;
        }
        Ok(out)
    }
}

async fn find_dev(client: &SquadClient, serial: u64) -> bool {
    client
        .get_devices()
        .await
        .unwrap()
        .iter()
        .any(|d| d.is_connected && d.serial == serial)
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = Args::parse();

    let app_serial: u64;
    let boot_serial: u64;
    let reset_path = args
        .reset_path
        .as_deref()
        .unwrap_or("curacao/postboot/reset");
    let reset_msg = args.reset_msg_json.as_deref().unwrap_or("{}");

    match (args.app_serial, args.boot_serial) {
        (None, None) => {
            return Err("Must provide one of --boot-serial or --app-serial".into());
        }
        (Some(a), Some(b)) => {
            app_serial = u64::from_str_radix(&a, 16).map_err(|e| format!("Error: {e:?}"))?;
            boot_serial = u64::from_str_radix(&b, 16).map_err(|e| format!("Error: {e:?}"))?;
        }
        (Some(a), None) => {
            app_serial = u64::from_str_radix(&a, 16).map_err(|e| format!("Error: {e:?}"))?;
            boot_serial = !app_serial;
        }
        (None, Some(b)) => {
            boot_serial = u64::from_str_radix(&b, 16).map_err(|e| format!("Error: {e:?}"))?;
            app_serial = !boot_serial;
        }
    }

    // create the binfile
    let mut bin_path = temp_dir();
    let rndm: u64 = thread_rng().gen();
    bin_path.push(format!("{rndm:016X}.bin"));
    Command::new("rust-objcopy")
        .args(["-O", "binary", &args.elf_path, bin_path.to_str().unwrap()])
        .output()
        .map_err(|e| format!("{e:?}"))?;

    // read it back
    let mut file = File::open(bin_path).map_err(|e| format!("{e:?}"))?;
    let mut bin_image = vec![];
    file.read_to_end(&mut bin_image).map_err(|e| format!("{e:?}"))?;
    // this is lazy
    while bin_image.len() % 4096 != 0 {
        bin_image.push(0xFF);
    }

    // Connect to device
    let client = connect("localhost:51837").await;

    // Is the app serial there?
    if find_dev(&client, app_serial).await {
        println!("Found app, sending app reset message");
        client
            .proxy_endpoint_json(app_serial, reset_path, 0, Value::from(reset_msg))
            .await?;
    }

    let start = Instant::now();
    let mut found = false;
    println!("Looking for bootloader device...");
    while start.elapsed() < Duration::from_secs(3) {
        if find_dev(&client, boot_serial).await {
            found = true;
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    if !found {
        return Err("Failed to find bootloader device!".into());
    }
    let bl = Bootloader::new(client, boot_serial);

    println!("Found device, writing {:0.02}KiB...", bin_image.len() as f32 / 1024.0);
    bl.erase(64 * 1024, bin_image.len() as u32).await.unwrap();
    bl.write(64 * 1024, &bin_image).await.unwrap();
    println!("Written. Commanding boot...");
    bl.boot().await?;
    println!("Boot command sent");

    return Ok(());

}


pub trait FromStrRadix: Sized {
    fn from_str_radix_gen(src: &str, radix: u32) -> Result<Self, ParseIntError>;
}

macro_rules! fsr_impl {
    ($($typ:ty),+) => {
        $(
            impl FromStrRadix for $typ {
                fn from_str_radix_gen(src: &str, radix: u32) -> Result<Self, ParseIntError> {
                    Self::from_str_radix(src, radix)
                }
            }
        )+
    };
}

fsr_impl!(u8, u16, u32, u64, u128, usize);

pub fn hex_or_dec<T: FromStrRadix>(mut s: &str) -> Option<T> {
    let radix;
    if s.starts_with("0x") {
        radix = 16;
        s = s.trim_start_matches("0x");
    } else if s.ends_with("h") {
        radix = 16;
        s = s.trim_end_matches("h");
    } else {
        radix = 10;
    }
    T::from_str_radix_gen(s, radix).ok()
}
