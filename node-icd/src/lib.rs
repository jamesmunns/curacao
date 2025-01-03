#![cfg_attr(not(feature = "use-std"), no_std)]

pub use postcard_rpc;
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

#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct Dummy {
    pub data: [u8; 16],
}

#[derive(Debug, Serialize, Deserialize, Schema, Copy, Clone)]
pub struct RGB8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct SetRGBCommand {
    pub pos: u16,
    pub color: RGB8,
}

#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct InvalidIndex;

pub type SetRGBResult = Result<(), InvalidIndex>;

// ---

// Endpoints spoken by our device
//
// GetUniqueIdEndpoint is mandatory, the others are examples
endpoints! {
    list = ENDPOINT_LIST;
    | EndpointTy                | RequestTy         | ResponseTy            | Path                          | Cfg                               |
    | ----------                | ---------         | ----------            | ----                          | ---                               |
    | GetUniqueIdEndpoint       | ()                | u64                   | "poststation/unique_id/get"   |                                   |
    | SleepEndpoint             | SleepMillis       | SleptMillis           | "template/sleep"              |                                   |
    | SetLedEndpoint            | LedState          | ()                    | "template/led/set"            |                                   |
    | GetLedEndpoint            | ()                | LedState              | "template/led/get"            |                                   |
    | RebootToBootloader        | ()                | ()                    | "curacao/postboot/reset"      |                                   |
    | SetOneRGBEndpoint         | SetRGBCommand     | SetRGBResult          | "curacao/rgb/one/set"         |                                   |
    | SetAllRGBEndpoint         | RGB8              | ()                    | "curacao/rgb/all/set"         |                                   |
}

// incoming topics handled by our device
topics! {
    list = TOPICS_IN_LIST;
    direction = TopicDirection::ToServer;
    | TopicTy                   | MessageTy         | Path              |
    | -------                   | ---------         | ----              |
    | DummyTopic                | Dummy             | "dummy"           |
}

// outgoing topics handled by our device
topics! {
    list = TOPICS_OUT_LIST;
    direction = TopicDirection::ToClient;
    | TopicTy                   | MessageTy         | Path              | Cfg                               |
    | -------                   | ---------         | ----              | ---                               |
}
