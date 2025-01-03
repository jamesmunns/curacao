//! A basic postcard-rpc/poststation-compatible application

use embassy_nrf::{gpio::Output, peripherals::PWM0, pwm::SequencePwm};
use node_icd::{
    DummyTopic, GetLedEndpoint, GetUniqueIdEndpoint, RebootToBootloader, SetAllRGBEndpoint, SetLedEndpoint, SetOneRGBEndpoint, ENDPOINT_LIST, RGB8, TOPICS_IN_LIST, TOPICS_OUT_LIST
};
use postcard_rpc::{
    define_dispatch,
    server::{
        impls::embassy_usb_v0_3::{
            dispatch_impl::{WireRxBuf, WireSpawnImpl, spawn_fn},
            PacketBuffers,
        },
        Server, SpawnContext,
    },
};
use static_cell::ConstStaticCell;

use crate::{handlers::{get_led, handle_dummy, reboot_bootloader, set_led, unique_id, set_all_rgb, set_one_rgb}, smartled::{BUF_CT, LED_CT}};
use crate::impls::{EsbRx, EsbTx};

/// Context contains the data that we will pass (as a mutable reference)
/// to each endpoint or topic handler
pub struct Context {
    /// We'll use this unique ID to identify ourselves to the poststation
    /// server. This should be unique per device.
    pub unique_id: u64,
    pub led: Output<'static>,
    pub smartled: SequencePwm<'static, PWM0>,
    pub rgb_buf: &'static mut [RGB8; LED_CT],
    pub data_buf: &'static mut [u16; BUF_CT],
    // pub esb_sender: bridge::Sender<1024>,
    // pub table: SMutex<Table>,
}

impl SpawnContext for Context {
    type SpawnCtxt = TaskContext;

    fn spawn_ctxt(&mut self) -> Self::SpawnCtxt {
        TaskContext {
            unique_id: self.unique_id,
        }
    }
}

pub struct TaskContext {
    pub unique_id: u64,
}

/// Storage describes the things we need to keep as a static, so it can be shared
/// with anyone who needs to send messages.
///
/// We can accept any mutex (this is using the thread-mode mutex, meaning that
/// it will work outside of interrupts or interrupt executors). The numeric
/// items control the buffer sizes allocated for Config, BOS, Control, and
/// MSOS USB buffers. See embassy-usb for more details on this.
// pub type AppStorage = WireStorage<ThreadModeRawMutex, AppDriver, 256, 256, 64, 256>;
/// BufStorage is the space used for receiving and sending frames. These values
/// control the largest frames we can send or receive.
pub type BufStorage = PacketBuffers<1024, 1024>;
/// AppTx is the type of our sender, which is how we send information to the client
pub type AppTx = EsbTx;
/// AppRx is the type of our receiver, which is how we receive information from the client
pub type AppRx = EsbRx;
/// AppServer is the type of the postcard-rpc server we are using
pub type AppServer = Server<AppTx, AppRx, WireRxBuf, MyApp>;

/// Statically store our packet buffers
pub static PBUFS: ConstStaticCell<BufStorage> = ConstStaticCell::new(BufStorage::new());
// /// Statically store our USB app buffers
// pub static STORAGE: AppStorage = AppStorage::new();

// This macro defines your application
define_dispatch! {
    // You can set the name of your app to any valid Rust type name. We use
    // "MyApp" here. You'll use this in `main` to create an instance of the
    // app.
    app: MyApp;
    // This chooses how we spawn functions. Here, we use the implementation
    // from the `embassy_usb_v0_3` implementation
    spawn_fn: spawn_fn;
    // This is our TX impl, which we aliased above
    tx_impl: AppTx;
    // This is our spawn impl, which also comes from `embassy_usb_v0_3`.
    spawn_impl: WireSpawnImpl;
    // This is the context type we defined above
    context: Context;

    // Endpoints are how we handle request/response pairs from the client.
    //
    // The "EndpointTy" are the names of the endpoints we defined in our ICD
    // crate. The "kind" is the kind of handler, which can be "blocking",
    // "async", or "spawn". Blocking endpoints will be called directly.
    // Async endpoints will also be called directly, but will be await-ed on,
    // allowing you to call async functions. Spawn endpoints will spawn an
    // embassy task, which allows for handling messages that may take some
    // amount of time to complete.
    //
    // The "handler"s are the names of the functions (or tasks) that will be
    // called when messages from this endpoint are received.
    endpoints: {
        // This list comes from our ICD crate. All of the endpoint handlers we
        // define below MUST be contained in this list.
        list: ENDPOINT_LIST;

        | EndpointTy                | kind      | handler                       |
        | ----------                | ----      | -------                       |
        | GetUniqueIdEndpoint       | blocking  | unique_id                     |
        | SetLedEndpoint            | blocking  | set_led                       |
        | GetLedEndpoint            | blocking  | get_led                       |
        | RebootToBootloader        | spawn     | reboot_bootloader             |
        | SetOneRGBEndpoint         | async     | set_one_rgb                   |
        | SetAllRGBEndpoint         | async     | set_all_rgb                   |
    };

    // Topics IN are messages we receive from the client, but that we do not reply
    // directly to. These have the same "kinds" and "handlers" as endpoints, however
    // these handlers never return a value
    topics_in: {
        // This list comes from our ICD crate. All of the topic handlers we
        // define below MUST be contained in this list.
        list: TOPICS_IN_LIST;

        | TopicTy                   | kind      | handler                       |
        | ----------                | ----      | -------                       |
        | DummyTopic                | blocking  | handle_dummy                  |
    };

    // Topics OUT are the messages we send to the client whenever we'd like. Since
    // these are outgoing, we do not need to define handlers for them.
    topics_out: {
        // This list comes from our ICD crate.
        list: TOPICS_OUT_LIST;
    };
}
