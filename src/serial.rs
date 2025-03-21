use anyhow::{Context, Result};
use embedded_io_adapters::futures_03::FromFutures;
use smol::Async;
use mctp::{Eid, Tag};
use mctp_estack::{Stack, SendOutput, serial::MctpSerialHandler};
use log::{trace, debug};

use std::time::Instant;

pub struct MctpSerial {
    mctp: mctp_estack::Stack,
    start_time: Instant,
    mctpserial: MctpSerialHandler,
    serial: FromFutures<Async<std::fs::File>>,
}

impl MctpSerial {
    pub fn new(eid: mctp::Eid, tty: &str) -> Result<Self> {

        let serial = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .open(tty)
            .context("Can't open tty device")?;
        let serial = smol::Async::new(serial)?;
        let serial = embedded_io_adapters::futures_03::FromFutures::new(serial);

        let start_time = Instant::now();
        let mtu = 64;
        let mctp = Stack::new(eid, mtu, 0);
        let mctpserial = MctpSerialHandler::new();

        Ok(Self {
            mctp,
            start_time,
            mctpserial,
            serial,
        })
    }

    fn now(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    pub async fn recv(&mut self)
    -> mctp::Result<(mctp_estack::MctpMessage, mctp_estack::ReceiveHandle)> {
        loop {
            let _ = self.mctp.update(self.now());

            let r = self.mctpserial.receive_async(
                &mut self.serial,
                &mut self.mctp
            ).await;

            if let Ok(Some((_msg, handle))) = r {
                let msg = self.mctp.fetch_message(&handle);
                trace!("msg: {msg:?}");
                return Ok((msg, handle));
            }
        }
    }

}

pub struct MctpSerialListener {
    serial: MctpSerial,
}

impl MctpSerialListener {
    pub fn new(eid: mctp::Eid, tty: &str) -> Result<Self> {
        Ok(Self {
            serial: MctpSerial::new(eid, tty)?,
        })
    }
}

impl mctp::AsyncListener for MctpSerialListener {
    type RespChannel<'a> = MctpSerialResp<'a> where Self: 'a;

    async fn recv<'f>(&mut self, buf: &'f mut [u8])
    -> mctp::Result<(&'f mut [u8], Self::RespChannel<'_>, Tag, mctp::MsgType, bool)> {
        loop {
            let (msg, handle) = self.serial.recv().await?;

            if msg.tag.is_owner() {
                let tag = msg.tag;
                let ic = msg.ic;
                let typ = msg.typ;
                let b = buf.get_mut(..msg.payload.len()).ok_or(mctp::Error::NoSpace)?;
                b.copy_from_slice(msg.payload);
                let eid = msg.source;
                self.serial.mctp.finished_receive(handle);
                let resp = MctpSerialResp {
                    eid,
                    tv: tag.tag(),
                    serial: &mut self.serial,
                };
                return Ok((b, resp, tag, typ, ic));
            } else {
                trace!("Discarding unmatched message {msg:?}");
                self.serial.mctp.finished_receive(handle);
            }

        }
    }

}

pub struct MctpSerialResp<'a> {
    eid: mctp::Eid,
    tv: mctp::TagValue,
    serial: &'a mut MctpSerial,
}

impl mctp::AsyncRespChannel for MctpSerialResp<'_> {
    type ReqChannel<'a> = MctpSerialReq where Self: 'a;

    async fn send_vectored(
        &mut self,
        typ: mctp::MsgType,
        integrity_check: bool,
        bufs: &[&[u8]],
    ) -> mctp::Result<()> {
        let r = self.serial.mctpserial.send_fill(self.eid, typ,
            Some(mctp::Tag::Unowned(self.tv)), integrity_check,
            None, &mut self.serial.serial, &mut self.serial.mctp,
            |v| {
                for b in bufs {
                    v.extend_from_slice(b).ok()?
                }
                trace!("v len {}", v.len());
                Some(())
            }).await;

        match r {
            SendOutput::Packet(_) => unreachable!(),
            SendOutput::Complete { .. } => Ok(()),
            SendOutput::Error { err, .. } => Err(err.into()),
        }
    }

    fn req_channel(&self) -> mctp::Result<Self::ReqChannel<'_>> {
        todo!();
    }

    fn remote_eid(&self) -> mctp::Eid {
        todo!();
    }
}

pub struct MctpSerialReq {
}

impl mctp::AsyncReqChannel for MctpSerialReq {    
    async fn send_vectored(&mut self, _: mctp::MsgType, _: bool, _: &[&[u8]])
    -> mctp::Result<()> {
        todo!()
    }
    async fn recv<'f>(&mut self, _: &'f mut [u8])
    -> mctp::Result<(&'f mut [u8], mctp::MsgType, Tag, bool)> { todo!() }

    fn remote_eid(&self) -> Eid { todo!() }
}
