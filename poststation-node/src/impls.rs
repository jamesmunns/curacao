use bridge_icd::{
    extract_topic2,
    postcard_rpc::{
        header::{VarHeader, VarKeyKind, VarSeq},
        server::{
            AsWireRxErrorKind, AsWireTxErrorKind, WireRx, WireRxErrorKind, WireTx, WireTxErrorKind,
        },
    },
    write_topic2, B2NTopic, Bridge2Node, FragBuf, N2BTopic, Node2Bridge,
};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use esb::{
    app::{EsbAppReceiver, EsbAppSender},
    EsbHeader,
};
use serde::Serialize;
use static_cell::{ConstStaticCell, StaticCell};
struct EsbTxInner {
    sender: EsbAppSender<1024>,
    ctr: u16,
    pipe: u8,
    pid: u8,
}

#[derive(Clone)]
pub struct EsbTx {
    inner: &'static Mutex<ThreadModeRawMutex, EsbTxInner>,
    serial: u64,
}

impl EsbTx {
    pub fn new(sender: EsbAppSender<1024>, serial: u64, pipe: u8) -> Self {
        static SENDER: StaticCell<Mutex<ThreadModeRawMutex, EsbTxInner>> = StaticCell::new();
        let inner = SENDER.init(Mutex::new(EsbTxInner {
            sender,
            ctr: 0,
            pipe,
            pid: 0,
        }));
        EsbTx { inner, serial }
    }

    pub async fn send_keepalive(&self) {
        let mut guard = self.inner.lock().await;
        let pid = guard.pid();
        let pipe = guard.pipe;
        let header = EsbHeader::new(252, pid, pipe, false).unwrap();
        let mut grant = guard.sender.wait_grant_packet(header).await.unwrap();

        // First write the proxy header
        let seq_no = guard.ctr();
        let Some(used) = write_topic2::<N2BTopic>(
            &Node2Bridge::Keepalive {
                serial: self.serial.to_le_bytes(),
            },
            seq_no,
            &mut grant,
        ) else {
            panic!();
        };
        grant.commit(used);
        guard.sender.start_tx();
    }

    pub async fn send_nop(&self) {
        let mut guard = self.inner.lock().await;
        let pid = guard.pid();
        let pipe = guard.pipe;
        let header = EsbHeader::new(252, pid, pipe, false).unwrap();
        let mut grant = guard.sender.wait_grant_packet(header).await.unwrap();

        // First write the proxy header
        let seq_no = guard.ctr();
        let Some(used) = write_topic2::<N2BTopic>(&Node2Bridge::Nop, seq_no, &mut grant) else {
            panic!();
        };
        grant.commit(used);
        guard.sender.start_tx();
    }
}

pub enum EsbTxError {}

impl AsWireTxErrorKind for EsbTxError {
    fn as_kind(&self) -> WireTxErrorKind {
        WireTxErrorKind::Other
    }
}

impl EsbTxInner {
    fn ctr(&mut self) -> VarSeq {
        let n = self.ctr;
        self.ctr = self.ctr.wrapping_add(1);
        VarSeq::Seq2(n)
    }

    fn pid(&mut self) -> u8 {
        let n = self.pid;
        self.pid = self.pid.wrapping_add(1);
        n & 0b11
    }
}

impl WireTx for EsbTx {
    type Error = EsbTxError;

    async fn send<T: Serialize + ?Sized>(
        &self,
        hdr: VarHeader,
        msg: &T,
    ) -> Result<(), Self::Error> {
        // todo: bad
        let mut buf = [0u8; 1024];
        let Some((hdrb, remain)) = hdr.write_to_slice(&mut buf) else {
            panic!()
        };
        let Ok(used) = postcard::to_slice(msg, remain) else {
            defmt::warn!("skipping message badser");
            return Ok(());
        };
        let ttl = hdrb.len() + used.len();
        let used = &buf[..ttl];

        let chunks = (used.len() + 127) / 128;
        for (i, ch) in used.chunks(128).enumerate() {
            let mut guard = self.inner.lock().await;
            let pid = guard.pid();
            let pipe = guard.pipe;
            let header = EsbHeader::new(252, pid, pipe, false).unwrap();
            let mut grant = guard.sender.wait_grant_packet(header).await.unwrap();

            // First write the proxy header
            let seq_no = guard.ctr();
            let Some(used) = write_topic2::<N2BTopic>(
                &Node2Bridge::Proxy {
                    part: i as u8,
                    ttl_parts: chunks as u8,
                },
                seq_no,
                &mut grant,
            ) else {
                panic!()
            };
            let (prefix, remain) = grant.split_at_mut(used);

            remain[..ch.len()].copy_from_slice(ch);

            let ttl = prefix.len() + ch.len();
            grant.commit(ttl);
            guard.sender.start_tx();
        }

        Ok(())
    }

    async fn send_raw(&self, buf: &[u8]) -> Result<(), Self::Error> {
        let chunks = (buf.len() + 127) / 128;
        for (i, ch) in buf.chunks(128).enumerate() {
            let mut guard = self.inner.lock().await;
            let pid = guard.pid();
            let pipe = guard.pipe;
            let header = EsbHeader::new(252, pid, pipe, false).unwrap();
            let mut grant = guard.sender.wait_grant_packet(header).await.unwrap();

            // First write the proxy header
            let seq_no = guard.ctr();
            let Some(used) = write_topic2::<N2BTopic>(
                &Node2Bridge::Proxy {
                    part: i as u8,
                    ttl_parts: chunks as u8,
                },
                seq_no,
                &mut grant,
            ) else {
                panic!()
            };
            let (prefix, remain) = grant.split_at_mut(used);

            remain[..ch.len()].copy_from_slice(ch);

            let ttl = prefix.len() + ch.len();
            grant.commit(ttl);
            guard.sender.start_tx();
        }

        Ok(())
    }

    async fn send_log_str(&self, _kkind: VarKeyKind, _s: &str) -> Result<(), Self::Error> {
        defmt::error!("Not implemented");
        Ok(())
    }

    async fn send_log_fmt<'a>(
        &self,
        _kkind: VarKeyKind,
        _a: core::fmt::Arguments<'a>,
    ) -> Result<(), Self::Error> {
        defmt::error!("Not implemented");
        Ok(())
    }
}

pub enum EsbRxError {}

impl AsWireRxErrorKind for EsbRxError {
    fn as_kind(&self) -> WireRxErrorKind {
        WireRxErrorKind::Other
    }
}

pub struct EsbRx {
    inner: EsbAppReceiver<1024>,
    serial: u64,
    pipe: u8,
    frag_buf: &'static mut FragBuf,
}

static FRAG_BUF: ConstStaticCell<FragBuf> = ConstStaticCell::new(FragBuf::new());

impl EsbRx {
    pub fn new(inner: EsbAppReceiver<1024>, serial: u64, pipe: u8) -> Self {
        Self {
            inner,
            serial,
            pipe,
            frag_buf: FRAG_BUF.take(),
        }
    }
}

impl WireRx for EsbRx {
    type Error = EsbRxError;

    async fn receive<'a>(&mut self, buf: &'a mut [u8]) -> Result<&'a mut [u8], Self::Error> {
        loop {
            let grant = self.inner.wait_read_packet().await;
            if grant.pipe() != self.pipe {
                grant.release();
                continue;
            }
            if grant.is_empty() {
                grant.release();
                continue;
            }
            let Some(e) = extract_topic2::<B2NTopic>(&grant) else {
                defmt::warn!("WHAT THE HELL {=usize}", grant.len());
                grant.release();
                continue;
            };
            match e.msg {
                Bridge2Node::InitializeAck { serial, use_pipe } => {
                    if serial != self.serial.to_le_bytes() {
                        panic!();
                    }
                    if use_pipe != self.pipe {
                        panic!();
                    }
                }
                Bridge2Node::Keepalive { serial } => {
                    if serial != self.serial.to_le_bytes() {
                        panic!();
                    }
                }
                Bridge2Node::Proxy { part, ttl_parts } => {
                    if ttl_parts == 0 {
                        defmt::warn!("Bad frag {=u8}, {=u8}", part, ttl_parts);
                        continue;
                    }
                    let to_fwd = if part == 0 && ttl_parts == 1 {
                        Some(e.remain)
                    } else {
                        self.frag_buf.handle_frag(part, ttl_parts, e.remain)
                    };
                    if let Some(to_fwd) = to_fwd {
                        let used = &mut buf[..to_fwd.len()];
                        used.copy_from_slice(to_fwd);
                        grant.release();
                        return Ok(used);
                    }
                }
                Bridge2Node::Reset => panic!(),
            }
            grant.release();
        }
    }
}
