use std::{
    fmt::Write, fs::{self, File}, io::{Read, Write as _}, num::ParseIntError, str::from_utf8, sync::atomic::{AtomicU32, Ordering}, time::Instant
};

use bootloader_icd::{
    scratch::BootMessage, AppPartitionInfo, BootResult, BootloadEndpoint, DataChunk,
    EraseFlashEndpoint, FlashEraseCommand, FlashReadCommand, FlashWriteCommand,
    GetAppFlashInfoEndpoint, GetBootMessageEndpoint, ReadFlashEndpoint, WriteFlashEndpoint,
};
use postcard_rpc::Endpoint;
use poststation_sdk::{connect, SquadClient};
use rand::{thread_rng, RngCore};
use serde::{de::DeserializeOwned, Serialize};

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

#[tokio::main]
async fn main() -> Result<(), String> {
    const SERIAL: u64 = 0xB55E43E32A752E08;
    let client = connect("localhost:51837").await;
    let bl = Bootloader::new(client, SERIAL);

    'repl: loop {
        print!("> ");
        let _ = std::io::stdout().flush();

        let line = read_line().await;
        let tline = line.trim();
        let words = tline.split_whitespace().collect::<Vec<_>>();
        match words.as_slice() {
            ["dump"] => {
                let Ok(info) = bl.partinfo().await else {
                    println!("Error getting info");
                    continue 'repl;
                };
                println!("Reading...");
                match bl.dumpfmt(info.start, info.len, info.transfer_chunk).await {
                    Ok(data) => {
                        println!("{data}");
                    }
                    Err(e) => {
                        println!("Error: '{e}'");
                    }
                }
            }
            ["dumpto", path] => {
                let Ok(info) = bl.partinfo().await else {
                    println!("Error getting info");
                    continue 'repl;
                };
                let _ = fs::remove_file(path);
                let Ok(mut out) = File::create_new(path) else {
                    println!("Error opening output file");
                    continue 'repl;
                };

                println!("Reading...");
                match bl.dumpfmt(info.start, info.len, info.transfer_chunk).await {
                    Ok(data) => {
                        if out.write_all(data.as_bytes()).is_ok() {
                            println!("Wrote to '{path}'");
                        } else {
                            println!("Error writing file");
                        }
                    }
                    Err(e) => {
                        println!("Error: '{e}'");
                    }
                }
            }
            ["info"] => match bl.partinfo().await {
                Ok(info) => {
                    println!("Info:");
                    println!(
                        "  * Start: {:08X} ({:0.02}KiB)",
                        info.start,
                        info.start as f32 / 1024.0
                    );
                    println!(
                        "  * Len:   {:08X} ({:0.02}KiB)",
                        info.len,
                        info.len as f32 / 1024.0
                    );
                    println!(
                        "  * Range: {:08X}..{:08X}",
                        info.start,
                        info.start + info.len
                    );
                    println!("  * Erase: {}B", info.erase_sz);
                    println!("  * Write: {}B", info.write_sz);
                    println!("  * Align: {}B", info.align);
                    println!("  * Chunk: {}", info.transfer_chunk);
                }
                Err(e) => {
                    println!("Error: '{e}'");
                }
            },
            ["erase", from, "to", to] => {
                let Some(from) = hex_or_dec::<u32>(from) else {
                    println!("Error: invalid start");
                    continue 'repl;
                };
                let Some(to) = hex_or_dec::<u32>(to) else {
                    println!("Error: invalid end");
                    continue 'repl;
                };
                let Some(len) = to.checked_sub(from) else {
                    println!("Error: Invalid range");
                    continue 'repl;
                };
                match bl.erase(from, len).await {
                    Ok(_) => println!("Erased"),
                    Err(e) => println!("{e}"),
                }
            }
            ["bootmsg"] => {
                let Ok(m) = bl.boot_msg().await else {
                    println!("Error getting boot msg");
                    continue 'repl;
                };
                println!("Boot Message: {m:?}");
                match m {
                    Some(BootMessage::AppPanicked { uptime, reason }) => {
                        println!("App Panicked. ({uptime})");
                        if let Ok(s) = from_utf8(&reason) {
                            println!("Reason: {s}");
                        }
                    }
                    Some(BootMessage::BootPanicked { uptime, reason }) => {
                        println!("Boot Panicked. ({uptime})");
                        if let Ok(s) = from_utf8(&reason) {
                            println!("Reason: {s}");
                        }
                    }
                    _ => {}
                }
            }
            ["boot"] => {
                match bl.boot().await {
                    Ok(_) => {
                        println!("Boot accepted. Exiting");
                        std::process::exit(0);
                    },
                    Err(e) => {
                        println!("{e}");
                    },
                }
            }
            ["load", path] => {
                let Ok(mut file) = File::open(path) else {
                    println!("Error opening file");
                    continue 'repl;
                };
                let mut buf = vec![];
                let Ok(_) = file.read_to_end(&mut buf) else {
                    println!("Error reading file");
                    continue 'repl;
                };
                // this is lazy
                while buf.len() % 4096 != 0 {
                    buf.push(0xFF);
                }
                bl.erase(64 * 1024, buf.len() as u32).await.unwrap();
                bl.write(64 * 1024, &buf).await.unwrap();
            }
            ["test"] => {
                let start = Instant::now();
                let info = bl.partinfo().await.unwrap();
                println!("({:?}) Erasing full range...", start.elapsed());
                bl.erase(info.start, info.len).await.unwrap();
                println!("({:?}) Generating random data...", start.elapsed());
                let mut data = vec![0u8; info.len as usize];
                {
                    let mut rng = thread_rng();
                    rng.fill_bytes(&mut data);
                }
                println!("({:?}) Writing random data...", start.elapsed());
                bl.write(info.start, &data).await.unwrap();
                println!("({:?}) Reading back...", start.elapsed());
                let mut rback = vec![];
                let mut addr = info.start;
                let end = info.start + info.len;
                while addr < end {
                    let c = bl.read_chunk(addr, 512).await.unwrap();
                    rback.extend_from_slice(&c.data);
                    addr += 512;
                }
                assert_eq!(rback, data);
                println!("({:?}) Erasing full range...", start.elapsed());
                bl.erase(info.start, info.len).await.unwrap();
                println!("({:?}) Reading back (should be empty)...", start.elapsed());
                let mut rback = vec![];
                let mut addr = info.start;
                let end = info.start + info.len;
                while addr < end {
                    let c = bl.read_chunk(addr, 512).await.unwrap();
                    rback.extend_from_slice(&c.data);
                    addr += 512;
                }
                assert!(rback.iter().all(|b| *b == 0xFF));
                println!("({:?}) Test passed!", start.elapsed());
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
