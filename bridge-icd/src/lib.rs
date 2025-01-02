#![cfg_attr(not(feature = "use-std"), no_std)]

pub use postcard_rpc;
use postcard_rpc::{
    endpoints,
    header::{VarHeader, VarKey, VarSeq},
    topic, topics, Topic, TopicDirection,
};
use postcard_schema::Schema;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

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

pub use poststation_fw_icd::bridging::{BridgeTable, ProxyMessage, ProxyResult, ProxyError};

// ---

// Endpoints spoken by our device
//
// GetUniqueIdEndpoint is mandatory, the others are examples
endpoints! {
    list = ENDPOINT_LIST;
    | EndpointTy                | RequestTy         | ResponseTy            | Path                          | Cfg                               |
    | ----------                | ---------         | ----------            | ----                          | ---                               |
    | GetUniqueIdEndpoint       | ()                | u64                   | "poststation/unique_id/get"   |                                   |
    | Host2BridgeEndpoint       | ProxyMessage<'a>  | ProxyResult           | "poststation/host/to/bridge"  | cfg(not(feature = "use-std"))     |
    | Host2BridgeEndpoint       | ProxyMessage      | ProxyResult           | "poststation/host/to/bridge"  | cfg(feature = "use-std")          |
    | SleepEndpoint             | SleepMillis       | SleptMillis           | "template/sleep"              |                                   |
    | SetLedEndpoint            | LedState          | ()                    | "template/led/set"            |                                   |
    | GetLedEndpoint            | ()                | LedState              | "template/led/get"            |                                   |
    | RebootToBootloader        | ()                | ()                    | "curacao/postboot/reset"      |                                   |
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
    | TopicTy                   | MessageTy         | Path                          | Cfg                               |
    | -------                   | ---------         | ----                          | ---                               |
    | Bridge2HostTopic          | ProxyMessage<'a>  | "poststation/bridge/to/host"  | cfg(not(feature = "use-std"))     |
    | Bridge2HostTopic          | ProxyMessage      | "poststation/bridge/to/host"  | cfg(feature = "use-std")          |
    | BridgeTableTopic          | BridgeTable       | "poststation/bridge/table"    |                                   |
}

topic!(N2BTopic, Node2Bridge, "node/to/bridge");

#[derive(Debug, Serialize, Deserialize, Schema)]
pub enum Node2Bridge {
    Initialize { serial: [u8; 8] },
    Keepalive { serial: [u8; 8] },
    Proxy { part: u8, ttl_parts: u8 },
    Nop,
}

topic!(B2NTopic, Bridge2Node, "bridge/to/node");

#[derive(Debug, Serialize, Deserialize, Schema)]
pub enum Bridge2Node {
    InitializeAck { serial: [u8; 8], use_pipe: u8 },
    Keepalive { serial: [u8; 8] },
    Proxy { part: u8, ttl_parts: u8 },
    Reset,
}

pub struct TopicExtract<'a, T> {
    pub msg: T,
    pub hdr: VarHeader,
    pub remain: &'a [u8],
}

pub fn extract_topic2<T>(data: &[u8]) -> Option<TopicExtract<'_, T::Message>>
where
    T: Topic,
    T::Message: DeserializeOwned,
{
    let (hdr, remain) = VarHeader::take_from_slice(data)?;
    if hdr.key != VarKey::Key2(T::TOPIC_KEY2) {
        return None;
    }
    let (msg, remain) = postcard::take_from_bytes::<T::Message>(remain).ok()?;
    Some(TopicExtract { msg, hdr, remain })
}

pub fn write_topic2<T>(msg: &T::Message, seq_no: VarSeq, buf: &mut [u8]) -> Option<usize>
where
    T: Topic,
    T::Message: Serialize,
{
    let hdr = VarHeader {
        key: VarKey::Key2(T::TOPIC_KEY2),
        seq_no,
    };
    let (_hdr, remain) = hdr.write_to_slice(buf)?;
    let used = postcard::to_slice(&msg, remain).ok()?;
    Some(_hdr.len() + used.len())
}


#[derive(Clone, Copy)]
pub enum FragStatus {
    Idle,
    Active {
        position: usize,
        rx_frags: u8,
        ttl_frags: u8,
    }
}

pub struct FragBuf {
    data: [u8; 1024],
    status: FragStatus,
}

impl FragBuf {
    pub const fn new() -> Self {
        Self {
            data: [0u8; 1024],
            status: FragStatus::Idle,
        }
    }

    pub fn reset_frag(&mut self) {
        self.status = FragStatus::Idle;
    }

    pub fn handle_frag<'a>(&'a mut self, part: u8, ttl_parts: u8, data: &[u8]) -> Option<&'a [u8]> {
        if ttl_parts < 1 {
            defmt::error!("Why is this calling handle frag");
            self.status = FragStatus::Idle;
            return None;
        }
        match self.status {
            FragStatus::Idle if part == 0 => {
                self.data[..data.len()].copy_from_slice(data);
                self.status = FragStatus::Active { position: data.len(), rx_frags: 1, ttl_frags: ttl_parts };
                None
            },
            FragStatus::Idle => {
                defmt::warn!("Missed first frag!");
                None
            }
            FragStatus::Active { position, rx_frags, ttl_frags } => {
                if rx_frags != part || ttl_frags != ttl_parts {
                    defmt::warn!("Missed frag!");
                    if part == 0 {
                        self.data[..data.len()].copy_from_slice(data);
                        self.status = FragStatus::Active { position: data.len(), rx_frags: 1, ttl_frags: ttl_parts };
                    } else {
                        self.status = FragStatus::Idle;
                    }
                    return None;
                }
                let end = position + data.len();
                let Some(range) = self.data.get_mut(position..end) else {
                    defmt::warn!("Frag overflow!");
                    self.status = FragStatus::Idle;
                    return None;
                };
                range.copy_from_slice(data);
                if (part + 1) == ttl_parts {
                    self.status = FragStatus::Idle;
                    Some(&self.data[..end])
                } else {
                    self.status = FragStatus::Active { position: end, rx_frags: part + 1, ttl_frags: ttl_parts };
                    None
                }
            },
        }
    }
}

impl Default for FragBuf {
    fn default() -> Self {
        Self::new()
    }
}
