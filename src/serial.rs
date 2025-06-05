// SPDX-License-Identifier: GPL-3.0
//
use anyhow::{Context, Result};
use embedded_io_adapters::futures_03::FromFutures;
use mctp_estack::serial::MctpSerialHandler;
use smol::Async;

#[allow(unused)]
pub struct MctpSerial {
    mctpserial: MctpSerialHandler,
    serial: FromFutures<Async<std::fs::File>>,
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

        let mctpserial = MctpSerialHandler::new();

        Ok(Self { mctpserial, serial })
    }

    pub async fn recv(&mut self) -> mctp::Result<&[u8]> {
        self.mctpserial.recv_async(&mut self.serial).await
    }

    pub async fn send(&mut self, pkt: &[u8]) -> mctp::Result<()> {
        self.mctpserial.send_async(pkt, &mut self.serial).await
    }
}
