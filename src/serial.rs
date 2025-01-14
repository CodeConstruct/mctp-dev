use anyhow::{Context, Result};
use embedded_io_adapters::futures_03::FromFutures;
use smol::Async;
use mctp::{Eid, Tag};
use mctp_estack::{Stack, SendOutput, serial::MctpSerialHandler};
use log::{trace, debug};
use crate::MctpMessage;

use std::time::Instant;

pub struct MctpSerial {
    mctp: mctp_estack::Stack,
    mctpserial: MctpSerialHandler,
    serial: FromFutures<Async<std::fs::File>>,
    start_time: Instant,
}

impl MctpSerial {
    pub fn new(tty: &str) -> Result<Self> {

        let serial = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .open(tty)
            .context("Can't open tty device")?;
        let serial = smol::Async::new(serial)?;
        let serial = embedded_io_adapters::futures_03::FromFutures::new(serial);

        let start_time = Instant::now();
        let mtu = 64;
        let eid = Eid(9);
        let mctp = Stack::new(eid, mtu, 0);
        let mctpserial = MctpSerialHandler::new();

        Ok(Self {
            mctp,
            mctpserial,
            serial,
            start_time,
        })
    }

    fn now(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    pub async fn recv(&mut self) -> Result<MctpMessage> {
        loop {
            let _ = self.mctp.update(self.now());

            let r = self.mctpserial.receive_async(
                &mut self.serial,
                &mut self.mctp
            ).await?;

            if let Some((_msg, handle)) = r {
                let msg = self.mctp.fetch_message(&handle);
                trace!("msg: {msg:?}");

                if let Tag::Unowned(_) = msg.tag {
                    trace!("!TO");
                    self.mctp.finished_receive(handle);
                    continue;
                }

                let res = MctpMessage::from_stack(&msg);
                self.mctp.finished_receive(handle);
                return Ok(res)
            }
        }
    }

    #[allow(unused)]
    async fn send(&mut self, msg: MctpMessage) -> Result<()> {
        let _ = self.mctp.update(self.now());
        let r = self.mctpserial.send_fill(msg.dest, msg.typ, Some(msg.tag),
            msg.ic, None, &mut self.serial, &mut self.mctp,
            |v| {
                let _ = v.extend_from_slice(&msg.payload);
                Some(())
            });

        match r.await {
            SendOutput::Packet(_) => unreachable!(),
            SendOutput::Complete { .. } => {
                debug!("tx complete");
                Ok(())
            }
            SendOutput::Error { err, .. } => {
                debug!("tx error {err:?}");
                Err(err.into())
            }
        }
    }
}
