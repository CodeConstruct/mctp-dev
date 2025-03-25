
use anyhow::Result;
use argh::FromArgs;
use log::{LevelFilter, info};
use mctp::{AsyncListener, AsyncRespChannel};

use mctp::{Eid,Tag,MsgType};

mod serial;
mod usbredir;

#[derive(FromArgs)]
/// Run an emulated MCTP device
struct Options {
    /// trasnsport to use: serial
    #[argh(subcommand)]
    transport: TransportSubcommand,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum TransportSubcommand {
    Serial(SerialSubcommand),
    Usb(UsbRedirSubcommand),
}

#[derive(FromArgs)]
#[argh(subcommand, name="serial")]
/// Serial transport
struct SerialSubcommand {
    /// TTY device
    #[argh(positional)]
    tty: String,
}

#[derive(FromArgs)]
#[argh(subcommand, name="usb")]
/// USB redir transport
struct UsbRedirSubcommand {
    /// path to socket
    #[argh(positional)]
    path: String,
}

enum Transport {
    Serial(serial::MctpSerialListener),
    Usb(usbredir::MctpUsbRedirListener),
}

enum TransportResp<'a> {
    Serial(serial::MctpSerialResp<'a>),
    Usb(usbredir::MctpUsbRedirResp<'a>),
}

struct TransportReq { }

impl Transport {
    async fn recv<'f>(&mut self, rx_buf: &'f mut [u8])
    -> mctp::Result<(&'f mut [u8], TransportResp, Tag, MsgType, bool)> {
        match self {
            Self::Serial(s) => {
                let (buf, resp, tag, typ, ic) = s.recv(rx_buf).await?;
                Ok((buf, TransportResp::Serial(resp), tag, typ, ic))
            }
            Self::Usb(u) => {
                let (buf, resp, tag, typ, ic) = u.recv(rx_buf).await?;
                Ok((buf, TransportResp::Usb(resp), tag, typ, ic))
            }
        }
    }
}

impl mctp::AsyncRespChannel for TransportResp<'_> {
    type ReqChannel<'a> = TransportReq where Self: 'a;

    async fn send_vectored(
        &mut self,
        typ: MsgType,
        integrity_check: bool,
        bufs: &[&[u8]],
    ) -> mctp::Result<()> {
        match self {
            Self::Serial(s) => {
                s.send_vectored(typ, integrity_check, bufs).await
            }
            Self::Usb(u) => {
                u.send_vectored(typ, integrity_check, bufs).await
            }
        }
    }

    fn req_channel(&self) -> mctp::Result<Self::ReqChannel<'_>> {
        todo!();
    }

    fn remote_eid(&self) -> mctp::Eid {
        todo!();
    }
}

impl mctp::AsyncReqChannel for TransportReq {
    async fn send_vectored(&mut self, _: MsgType, _: bool, _: &[&[u8]])
    -> mctp::Result<()> {
        unimplemented!()
    }
    async fn recv<'f>(&mut self, _: &'f mut [u8])
    -> mctp::Result<(&'f mut [u8], MsgType, Tag, bool)> {
        unimplemented!()
    }

    fn remote_eid(&self) -> Eid {
        unimplemented!()
    }
}

async fn run(mut transport: Transport)
-> std::io::Result<()> {
    loop {
        let mut rx_buf = [0u8; 4096];
        let (buf, mut resp, tag, typ, _ic) = transport.recv(&mut rx_buf).await?;
        info!("msg: {:?}", (&buf, tag, typ));
        match typ.0 {
            0 => (),
            1 => {
                let _ = resp.send(typ, buf).await?;
            },
            _ => (),
        }
    }
}

fn main() -> Result<()> {
    let opts : Options = argh::from_env();
    
    let conf = simplelog::ConfigBuilder::new().build();
    simplelog::SimpleLogger::init(LevelFilter::Debug, conf)?;

    let eid = Eid(9);

    match opts.transport {
        TransportSubcommand::Serial(s) => {
            let serial = serial::MctpSerialListener::new(eid, &s.tty)?;
            info!("Created MCTP Serial transport on {}", s.tty);
            let t = Transport::Serial(serial);
            let _ = smol::block_on(run(t));

        }
        TransportSubcommand::Usb(u) => {
            let (transport, mut port) = usbredir::MctpUsbRedir::new(eid, &u.path)?;
            let l = usbredir::MctpUsbRedirListener::new(transport);
            info!("Created MCTP USB transport on {}", u.path);
            let fut = futures::future::join(
                port.process(),
                run(Transport::Usb(l))
            );
            let _ = smol::block_on(fut);
        }
    };

    Ok(())
}
