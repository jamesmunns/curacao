#![cfg_attr(not(feature = "use-std"), no_std)]

use postcard_rpc::{endpoints, topics, TopicDirection};
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct AppPartitionInfo {
    pub start: u32,
    pub len: u32,
    pub transfer_chunk: u32,
}

#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct FlashReadCommand {
    pub start: u32,
    pub len: u32,
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

#[cfg(not(feature = "use-std"))]
pub type ReadResult<'a> = Result<DataChunk<'a>, ReadError>;

#[cfg(feature = "use-std")]
pub type ReadResult = Result<DataChunk, ReadError>;

// ---

// Endpoints spoken by our device
//
// GetUniqueIdEndpoint is mandatory, the others are examples
endpoints! {
    list = ENDPOINT_LIST;
    | EndpointTy                | RequestTy         | ResponseTy        | Path                          | Cfg                           |
    | ----------                | ---------         | ----------        | ----                          | ---                           |
    | GetUniqueIdEndpoint       | ()                | u64               | "poststation/unique_id/get"   |                               |
    | ReadFlashEndpoint         | FlashReadCommand  | ReadResult<'a>    | "bootloader/flash/read"       | cfg(not(feature = "use-std")) |
    | ReadFlashEndpoint         | FlashReadCommand  | ReadResult        | "bootloader/flash/read"       | cfg(feature = "use-std")      |
    | GetAppFlashInfoEndpoint   | ()                | AppPartitionInfo  | "bootloader/flash/info"       |                               |
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
