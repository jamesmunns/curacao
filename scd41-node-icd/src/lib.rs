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
pub struct Scd41Reading {
    pub temp_c: f32,
    pub humi_pct: f32,
    pub co2_ppm: u16,
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
    | SleepEndpoint             | SleepMillis       | SleptMillis           | "template/sleep"              |                                   |
    | SetLedAEndpoint           | LedState          | ()                    | "curacao/led/a/set"           |                                   |
    | SetLedBEndpoint           | LedState          | ()                    | "curacao/led/b/set"           |                                   |
    | RebootToBootloader        | ()                | ()                    | "curacao/postboot/reset"      |                                   |
}

// incoming topics handled by our device
topics! {
    list = TOPICS_IN_LIST;
    direction = TopicDirection::ToServer;
    | TopicTy                   | MessageTy         | Path              |
    | -------                   | ---------         | ----              |
}

// outgoing topics handled by our device
topics! {
    list = TOPICS_OUT_LIST;
    direction = TopicDirection::ToClient;
    | TopicTy           | MessageTy     | Path                      | Cfg   |
    | -------           | ---------     | ----                      | ---   |
    | ScdReadingTopic   | Scd41Reading  | "curacao/scd41/reading"   |       |
}
