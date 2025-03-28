// SPDX-License-Identifier: GPL-2.0
//
use anyhow::{Context, Result};
use embedded_io_adapters::futures_03::FromFutures;
use smol::Async;
use mctp::{Eid, Tag};
use mctp_estack::{Stack, SendOutput, serial::MctpSerialHandler};
use log::{trace, debug};

use std::time::Instant;

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

        Ok(Self {
            mctpserial,
            serial,
        })
    }

    pub async fn recv(&mut self) -> mctp::Result<&[u8]> {
        todo!()
    }

    pub async fn send(&mut self, _pkt: &[u8]) -> mctp::Result<()> {
        todo!()
    }

}
