#![cfg_attr(not(feature = "use-std"), no_std)]

use postcard_rpc::{endpoints, topics, TopicDirection};
use postcard_schema::Schema;
use scratch::BootMessage;
use serde::{Deserialize, Serialize};
pub mod scratch;

#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct AppPartitionInfo {
    pub start: u32,
    pub len: u32,
    pub transfer_chunk: u32,
    pub write_sz: u32,
    pub erase_sz: u32,
    pub align: u32,
}

#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct FlashReadCommand {
    pub start: u32,
    pub len: u32,
}

#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct FlashEraseCommand {
    pub start: u32,
    pub len: u32,
    pub force: bool,
}

#[cfg(not(feature = "use-std"))]
#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct FlashWriteCommand<'a> {
    pub start: u32,
    pub data: &'a [u8],
    pub force: bool,
}

#[cfg(feature = "use-std")]
#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct FlashWriteCommand {
    pub start: u32,
    pub data: Vec<u8>,
    pub force: bool,
}

#[cfg(not(feature = "use-std"))]
#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct DataChunk<'a> {
    pub data: &'a [u8],
}

#[cfg(feature = "use-std")]
#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct DataChunk {
    pub data: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize, Schema)]
pub enum ReadError {
    OutOfRange {
        req_start: u32,
        req_end: u32,
        mem_start: u32,
        mem_end: u32,
    },
    TooLarge {
        req_len: u32,
        max_len: u32,
    }
}

#[derive(Debug, Serialize, Deserialize, Schema)]
pub enum EraseError {
    OutOfRange,
    StartNotAligned,
    LenNotAligned,
    HardwareError,
}

#[derive(Debug, Serialize, Deserialize, Schema)]
pub enum WriteError {
    OutOfRange,
    StartNotAligned,
    LenNotAligned,
    NeedsErase,
    HardwareError,
}

#[cfg(not(feature = "use-std"))]
pub type ReadResult<'a> = Result<DataChunk<'a>, ReadError>;

#[cfg(feature = "use-std")]
pub type ReadResult = Result<DataChunk, ReadError>;

pub type EraseResult = Result<(), EraseError>;
pub type WriteResult = Result<(), WriteError>;

#[cfg(not(feature = "use-std"))]
pub type OptBootMessage<'a> = Option<BootMessage<'a>>;

#[cfg(feature = "use-std")]
pub type OptBootMessage = Option<BootMessage>;

#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct FailedSanityCheck;

pub type BootResult = Result<(), FailedSanityCheck>;

// ---

// Endpoints spoken by our device
//
// GetUniqueIdEndpoint is mandatory, the others are examples
endpoints! {
    list = ENDPOINT_LIST;
    | EndpointTy                | RequestTy             | ResponseTy            | Path                          | Cfg                           |
    | ----------                | ---------             | ----------            | ----                          | ---                           |
    | GetUniqueIdEndpoint       | ()                    | u64                   | "poststation/unique_id/get"   |                               |
    | GetBootMessageEndpoint    | ()                    | OptBootMessage<'a>    | "bootloader/message/get"      | cfg(not(feature = "use-std")) |
    | GetBootMessageEndpoint    | ()                    | OptBootMessage        | "bootloader/message/get"      | cfg(feature = "use-std")      |
    | ReadFlashEndpoint         | FlashReadCommand      | ReadResult<'a>        | "bootloader/flash/read"       | cfg(not(feature = "use-std")) |
    | ReadFlashEndpoint         | FlashReadCommand      | ReadResult            | "bootloader/flash/read"       | cfg(feature = "use-std")      |
    | GetAppFlashInfoEndpoint   | ()                    | AppPartitionInfo      | "bootloader/flash/info"       |                               |
    | EraseFlashEndpoint        | FlashEraseCommand     | EraseResult           | "bootloader/flash/erase"      |                               |
    | WriteFlashEndpoint        | FlashWriteCommand<'a> | WriteResult           | "bootloader/flash/write"      | cfg(not(feature = "use-std")) |
    | WriteFlashEndpoint        | FlashWriteCommand     | WriteResult           | "bootloader/flash/write"      | cfg(feature = "use-std")      |
    | BootloadEndpoint          | ()                    | BootResult            | "bootloader/boot"             |                               |
}

// incoming topics handled by our device
topics! {
    list = TOPICS_IN_LIST;
    direction = TopicDirection::ToServer;
    | TopicTy                   | MessageTy     | Path              |
    | -------                   | ---------     | ----              |
}

// outgoing topics handled by our device
topics! {
    list = TOPICS_OUT_LIST;
    direction = TopicDirection::ToClient;
    | TopicTy                   | MessageTy     | Path              | Cfg                           |
    | -------                   | ---------     | ----              | ---                           |
}
