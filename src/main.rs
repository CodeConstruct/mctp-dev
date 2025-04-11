// SPDX-License-Identifier: GPL-2.0

use anyhow::Result;
use argh::FromArgs;
use futures::{join, select, FutureExt};
use log::{debug, info, warn, LevelFilter};
use mctp::{AsyncListener, AsyncRespChannel};
use mctp_estack::routing::{PortBottom, PortBuilder, PortId, PortLookup, PortStorage, Router};
use std::time::Instant;

use mctp::{Eid, MsgType};

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
#[argh(subcommand, name = "serial")]
/// Serial transport
struct SerialSubcommand {
    /// TTY device
    #[argh(positional)]
    tty: String,
}

#[derive(FromArgs)]
#[argh(subcommand, name = "usb")]
/// USB redir transport
struct UsbRedirSubcommand {
    /// path to socket
    #[argh(positional)]
    path: String,
}

#[allow(clippy::large_enum_variant)]
enum Transport {
    Serial(serial::MctpSerial),
    Usb(usbredir::MctpUsbRedir),
}

impl Transport {
    async fn recv(&mut self) -> mctp::Result<&[u8]> {
        match self {
            Self::Serial(s) => s.recv().await,
            Self::Usb(u) => u.recv().await,
        }
    }

    async fn send(&mut self, pkt: &[u8]) -> mctp::Result<()> {
        match self {
            Self::Serial(s) => s.send(pkt).await,
            Self::Usb(u) => u.send(pkt).await,
        }
    }
}

struct Routes {}

impl PortLookup for Routes {
    fn by_eid(&mut self, _eid: Eid, _source_port: Option<PortId>) -> Option<PortId> {
        Some(PortId(0))
    }
}

async fn update_router_time(router: &Router<'_>, start_time: Instant) {
    let ms = (Instant::now() - start_time).as_millis() as u64;
    let r = router.update_time(ms).await;
    if let Err(e) = r {
        warn!("time update failure: {e}");
    }
}

async fn run<'a>(
    mut transport: Transport,
    mut port: PortBottom<'_>,
    router: &'a Router<'a>,
    start_time: Instant,
) -> std::io::Result<()> {
    let portid = PortId(0);
    loop {
        select!(
            r = transport.recv().fuse() => {
                update_router_time(router, start_time).await;
                if let Ok(pkt) = r {
                    router.inbound(pkt, portid).await;
                }
            }
            (pkt, _dest) = port.outbound().fuse() => {
                update_router_time(router, start_time).await;
                let _ = transport.send(pkt).await;
                port.outbound_done();
            }
        );
    }
}

#[allow(unused)]
async fn echo<'a>(router: &'a Router<'a>) -> std::io::Result<()> {
    let mut l = router.listener(MsgType(1))?;

    info!("echo server listening");
    let mut buf = [0u8; 100];
    loop {
        let Ok((msg, mut resp, _tag, typ, _ic)) = l.recv(&mut buf).await else {
            continue;
        };

        if let Err(_e) = resp.send(typ, msg).await {
            debug!("listener reply fail");
        }
    }
}

async fn control<'a>(router: &'a Router<'a>) -> std::io::Result<()> {
    let mut l = router.listener(mctp::MCTP_TYPE_CONTROL)?;
    let mut c = mctp_estack::control::MctpControl::new(router);
    let u = uuid::Uuid::new_v4();

    let _ = c.set_message_types(&[mctp::MCTP_TYPE_CONTROL]);
    c.set_uuid(&u);

    info!("MCTP Control Protocol server listening");
    let mut buf = [0u8; 256];
    loop {
        let Ok((msg, resp, _tag, _typ, _ic)) = l.recv(&mut buf).await else {
            continue;
        };

        let r = c.handle_async(msg, resp).await;

        if let Err(e) = r {
            info!("control handler failure: {e}");
        }
    }
}

fn main() -> Result<()> {
    let opts: Options = argh::from_env();

    let conf = simplelog::ConfigBuilder::new().build();
    simplelog::SimpleLogger::init(LevelFilter::Debug, conf)?;

    let eid = Eid(0);
    let mtu = 68usize;

    let mut port_storage = PortStorage::<4>::new();
    let mut port = PortBuilder::new(&mut port_storage);
    let (port_top, port_bottom) = port.build(mtu).unwrap();
    let ports = [port_top];

    let start_time = Instant::now();

    let stack = mctp_estack::Stack::new(eid, mtu, 0u64);

    let mut routes = Routes {};
    let router = Router::new(stack, &ports, &mut routes);

    let (transport, mut port) = match opts.transport {
        TransportSubcommand::Serial(s) => {
            let serial = serial::MctpSerial::new(&s.tty)?;
            info!("Created MCTP Serial transport on {}", s.tty);
            let t = Transport::Serial(serial);
            (t, None)
        }
        TransportSubcommand::Usb(u) => {
            let (usbredir, port) = usbredir::MctpUsbRedir::new(&u.path)?;
            info!("Created MCTP USB transport on {}", u.path);
            let t = Transport::Usb(usbredir);
            (t, Some(port))
        }
    };

    let fut = match port {
        Some(ref mut p) => futures::future::Either::Left(p.process()),
        None => futures::future::Either::Right(futures::future::pending()),
    };

    let _ = smol::block_on(async {
        join!(
            fut,
            run(transport, port_bottom, &router, start_time),
            control(&router),
        )
    });

    Ok(())
}
