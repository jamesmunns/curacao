use postcard_rpc::Key;
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

pub const BOOT_KEY: Key = Key::for_path::<BootMessage>("boot message");

#[cfg(not(feature = "use-std"))]
#[derive(Debug, Serialize, Deserialize, Schema, Clone)]
pub enum BootMessage<'a> {
    StayInBootloader,
    JustBoot,
    BootAttempted,
    AppPanicked {
        uptime: u64,
        reason: &'a [u8],
    },
    BootPanicked {
        uptime: u64,
        reason: &'a [u8],
    }
}

#[cfg(feature = "use-std")]
#[derive(Debug, Serialize, Deserialize, Schema, Clone)]
pub enum BootMessage {
    StayInBootloader,
    JustBoot,
    BootAttempted,
    AppPanicked {
        uptime: u64,
        reason: Vec<u8>,
    },
    BootPanicked {
        uptime: u64,
        reason: Vec<u8>,
    }
}
