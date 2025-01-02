use bridge_icd::{
    extract_topic2, write_topic2, B2NTopic, Bridge2HostTopic, Bridge2Node, BridgeTable, BridgeTableTopic, FragBuf, N2BTopic, Node2Bridge, ProxyMessage, TopicExtract
};
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, mutex::Mutex};
use embassy_time::{Duration, Ticker};
use esb::{
    app::{EsbAppReceiver, EsbAppSender},
    payload::PayloadR,
    EsbHeader,
};
use postcard_rpc::{header::VarSeq, server::Sender as PrpcSender};
use static_cell::ConstStaticCell;

use crate::{
    app::AppTx,
    table::{PipeAlloc, Table},
};

pub type SMutex<T> = &'static Mutex<ThreadModeRawMutex, T>;
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Sender<const OUT: usize> {
    pub sender: SMutex<EsbAppSender<OUT>>,
}

const fn fbufs() -> [FragBuf; 7] {
    [const { FragBuf::new() }; 7]
}

pub static FRAG_BUFS: ConstStaticCell<[FragBuf; 7]> = ConstStaticCell::new(fbufs());

pub struct Bridge<const OUT: usize, const IN: usize> {
    pub table: SMutex<Table>,
    pub esb_sender: Sender<OUT>,
    pub recv: EsbAppReceiver<IN>,
    pub prpc_sender: PrpcSender<AppTx>,
    pub table_ctr: u16,
    pub proxy_ctr: u16,
    pub frag_bufs: &'static mut [FragBuf; 7],
}

impl<const OUT: usize, const IN: usize> Bridge<OUT, IN> {
    fn table_ctr(&mut self) -> u16 {
        let n = self.table_ctr;
        self.table_ctr = self.table_ctr.wrapping_add(1);
        n
    }

    fn proxy_ctr(&mut self) -> u16 {
        let n = self.proxy_ctr;
        self.proxy_ctr = self.proxy_ctr.wrapping_add(1);
        n
    }

    pub async fn run(&mut self) {
        let mut table_ticker = Ticker::every(Duration::from_secs(5));
        loop {
            match select(table_ticker.next(), self.recv.wait_read_packet()).await {
                Either::First(()) => self.table_tick().await,
                Either::Second(msg) => {
                    if let Some(pkt) = extract_topic2::<N2BTopic>(&msg) {
                        self.handle(&pkt, &msg).await;
                    } else {
                        defmt::warn!("Bad message");
                    };

                    msg.release();
                }
            }
        }
    }

    /// Helper function for table maintenance task
    async fn table_tick(&mut self) {
        let mut tout = BridgeTable {
            sers: heapless::Vec::new(),
        };
        // Perform table ops in a single mutex lock
        {
            let mut guard = self.table.lock().await;
            guard.cull_older_than(TIMEOUT);
            guard.extract_table(&mut tout.sers);
        }
        // send it up
        let seq = VarSeq::Seq2(self.table_ctr());
        let _ = self
            .prpc_sender
            .publish::<BridgeTableTopic>(seq, &tout)
            .await;
    }

    /// Handle incoming messages after they have been deserialized
    async fn handle(&mut self, extract: &TopicExtract<'_, Node2Bridge>, grant: &PayloadR<IN>) {
        let reset = || Some(Bridge2Node::Reset);

        let reply = match (grant.pipe(), &extract.msg) {
            (0, Node2Bridge::Initialize { serial }) => self.init_serial(serial).await,
            (0, _) => reset(),
            (_, Node2Bridge::Initialize { .. }) => reset(),
            (n, Node2Bridge::Keepalive { serial }) => {
                let update_ok = { self.table.lock().await.update_time(n, serial) };
                if update_ok {
                    // reply keepalive
                    defmt::info!("Pipe {=u8} is alive", n);
                    Some(Bridge2Node::Keepalive { serial: *serial })
                } else {
                    reset()
                }
            }
            (n, Node2Bridge::Proxy { part, ttl_parts }) => self.proxy(n, extract.remain, *part, *ttl_parts).await,
            // TODO: validate Nops?
            (_n, Node2Bridge::Nop) => None,
        };

        let Some(reply) = reply else {
            return;
        };

        let Ok(header) = EsbHeader::new(252, grant.pid(), grant.pipe(), true) else {
            defmt::error!("Bad header?");
            return;
        };

        let mut guard = self.esb_sender.sender.lock().await;
        let Ok(mut wgr) = guard.wait_grant_packet(header).await else {
            return;
        };

        let res = write_topic2::<B2NTopic>(&reply, VarSeq::Seq2(self.proxy_ctr()), &mut wgr);

        if let Some(used) = res {
            wgr.commit(used);
        }
    }

    /// Helper function for handling Initialize requests
    async fn init_serial(&mut self, serial: &[u8; 8]) -> Option<Bridge2Node> {
        let alloc_res = { self.table.lock().await.allocate_pipe(&serial) };
        match alloc_res {
            Some(PipeAlloc::New(pipe)) => {
                defmt::info!("Allocating pipe {=u8}", pipe);
                Some(Bridge2Node::InitializeAck {
                    serial: *serial,
                    use_pipe: pipe,
                })
            }
            Some(PipeAlloc::Existing(pipe)) => {
                // Send init ack
                Some(Bridge2Node::InitializeAck {
                    serial: *serial,
                    use_pipe: pipe,
                })
            }
            None => Some(Bridge2Node::Reset),
        }
    }

    /// Helper function for handling Proxy requests
    async fn proxy(
        &mut self,
        pipe: u8,
        remain: &[u8],
        part: u8,
        ttl_parts: u8
    ) -> Option<Bridge2Node> {
        let ser_for_pipe = { self.table.lock().await.serial_for_pipe(pipe) };
        if let Some(ser) = ser_for_pipe {
            if ttl_parts == 0 {
                defmt::warn!("Malformed frag?");
                return None;
            }
            let Bridge { prpc_sender, frag_bufs, proxy_ctr, .. } = self;

            let to_fwd = if part == 0 && ttl_parts == 1 {
                Some(remain)
            } else {
                frag_bufs[(pipe - 1) as usize].handle_frag(part, ttl_parts, remain)
            };

            if let Some(to_fwd) = to_fwd {
                let seq = VarSeq::Seq2({
                    let n = *proxy_ctr;
                    *proxy_ctr = proxy_ctr.wrapping_add(1);
                    n
                });
                // todo: validate `remain` has valid contents for a postcard-rpc message?
                let _ = prpc_sender
                    .publish::<Bridge2HostTopic>(
                        seq,
                        &ProxyMessage {
                            serial: ser,
                            msg: to_fwd,
                        },
                    )
                    .await;
            }

            // TODO: ack proxy requests?
            None
        } else {
            Some(Bridge2Node::Reset)
        }
    }
}
