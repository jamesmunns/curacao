#![cfg_attr(not(feature = "use-std"), no_std)]

use postcard_rpc::{endpoints, topics, TopicDirection};
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct SleepMillis {
    pub millis: u16,
}

#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct SleptMillis {
    pub millis: u16,
}

#[derive(Debug, Serialize, Deserialize, Schema)]
pub enum LedState {
    Off,
    On,
}

#[derive(Serialize, Deserialize, Debug, Schema)]
pub enum DecodeError {
    DecodeOrEof,
    Overflow,
}

pub fn decode_to(mut data: &[u8], mut buf: &mut [u8]) -> Result<(), DecodeError> {
    while !data.is_empty() {
        let (val, remain) = postcard::take_from_bytes::<i32>(data)
            .map_err(|_| DecodeError::DecodeOrEof)?;
        data = remain;
        let len = val.unsigned_abs() as usize;
        if len > buf.len() {
            return Err(DecodeError::Overflow);
        }
        let (now, later) = buf.split_at_mut(len);
        buf = later;
        if val.is_negative() {
            if len > data.len() {
                return Err(DecodeError::DecodeOrEof);
            }
            let (inow, ilater) = data.split_at(len);
            data = ilater;
            now.copy_from_slice(inow);
        } else {
            let Some((val, later)) = data.split_first() else {
                return Err(DecodeError::DecodeOrEof);
            };
            data = later;
            now.iter_mut().for_each(|b| *b = *val);
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Schema)]
#[cfg(feature = "use-std")]
pub struct DisplayCommand {
    pub offset: u32,
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Schema)]
#[cfg(not(feature = "use-std"))]
pub struct DisplayCommand<'a> {
    pub offset: u32,
    pub data: &'a [u8],
}

#[derive(Serialize, Deserialize, Debug, Schema)]
pub struct TooSoon;

pub type DisplayResult = Result<(), DecodeError>;
pub type DrawResult = Result<(), TooSoon>;

// ---

// Endpoints spoken by our device
//
// GetUniqueIdEndpoint is mandatory, the others are examples
endpoints! {
    list = ENDPOINT_LIST;
    | EndpointTy                | RequestTy             | ResponseTy            | Path                          | Cfg                           |
    | ----------                | ---------             | ----------            | ----                          | ---                           |
    | GetUniqueIdEndpoint       | ()                    | u64                   | "poststation/unique_id/get"   |                               |
    | RebootToPicoBoot          | ()                    | ()                    | "template/picoboot/reset"     |                               |
    | SleepEndpoint             | SleepMillis           | SleptMillis           | "template/sleep"              |                               |
    | SetLedEndpoint            | LedState              | ()                    | "template/led/set"            |                               |
    | GetLedEndpoint            | ()                    | LedState              | "template/led/get"            |                               |
    | SetDisplay                | DisplayCommand        | DisplayResult         | "display/set"                 | cfg(feature = "use-std")      |
    | SetDisplay                | DisplayCommand<'a>    | DisplayResult         | "display/set"                 | cfg(not(feature = "use-std")) |
    | DrawDisplay               | ()                    | DrawResult            | "display/draw"                |                               |
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
