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

// ---

// Endpoints spoken by our device
//
// GetUniqueIdEndpoint is mandatory, the others are examples
endpoints! {
    list = ENDPOINT_LIST;
    | EndpointTy                | RequestTy         | ResponseTy            | Path                          | Cfg                               |
    | ----------                | ---------         | ----------            | ----                          | ---                               |
    | GetUniqueIdEndpoint       | ()                | u64                   | "poststation/unique_id/get"   |                                   |
    | RebootToPicoBoot          | ()                | ()                    | "template/picoboot/reset"     |                                   |
    | SleepEndpoint             | SleepMillis       | SleptMillis           | "template/sleep"              |                                   |
    | SetLedEndpoint            | LedState          | ()                    | "template/led/set"            |                                   |
    | GetLedEndpoint            | ()                | LedState              | "template/led/get"            |                                   |
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
