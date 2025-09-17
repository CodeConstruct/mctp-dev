// SPDX-License-Identifier: GPL-3.0

use anyhow::Result;
use argh::FromArgs;
use futures::{select, FutureExt};
use log::{debug, info, warn, LevelFilter};
use mctp::{AsyncListener, AsyncRespChannel, Eid};
use mctp_estack::{
    control::{ControlEvent, MctpControl},
    router::{Port, PortId, PortLookup, PortTop, Router},
};
use std::time::Instant;

#[cfg(feature = "nvme-mi")]
use nvme_mi_dev::{
    CommandEffectError, ManagementEndpoint, PciePort, PortType, Subsystem,
    SubsystemInfo, TwoWirePort,
};

mod serial;
mod usbredir;

#[derive(FromArgs)]
/// Run an emulated MCTP device
struct Options {
    /// MCTP transport to use
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
    fn by_eid(
        &self,
        _eid: Eid,
        source_port: Option<PortId>,
    ) -> (Option<PortId>, Option<usize>) {
        // we're an endpoint device, don't forward packets from other ports
        if source_port.is_some() {
            return (None, None);
        }
        (Some(PortId(0)), None)
    }
}

async fn update_router_time(router: &Router<'_>, start_time: Instant) {
    let ms = (Instant::now() - start_time).as_millis() as u64;
    let r = router.update_time(ms).await;
    if let Err(e) = r {
        warn!("time update failure: {e}");
    }
}

async fn run(
    mut transport: Transport,
    mut port: Port<'_>,
    router: &Router<'_>,
) -> std::io::Result<()> {
    let portid = PortId(0);
    let start_time = Instant::now();
    loop {
        select!(
            r = transport.recv().fuse() => {
                update_router_time(router, start_time).await;
                let pkt = r?;
                router.inbound(pkt, portid).await;
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
    const VENDOR_SUBTYPE_ECHO: [u8; 3] = [0xcc, 0xde, 0xf0];
    let mut l = router.listener(mctp::MCTP_TYPE_VENDOR_PCIE)?;

    info!("echo server listening");
    let mut buf = [0u8; 100];
    loop {
        let Ok((_typ, _ic, msg, mut resp)) = l.recv(&mut buf).await else {
            continue;
        };

        if !msg.starts_with(&VENDOR_SUBTYPE_ECHO) {
            continue;
        }

        if let Err(_e) = resp.send(msg).await {
            debug!("listener reply fail");
        }
    }
}

async fn control(
    router: &Router<'_>,
    ctrl_ev_sender: async_channel::Sender<ControlEvent>,
) -> std::io::Result<()> {
    let mut l = router.listener(mctp::MCTP_TYPE_CONTROL)?;
    let mut c = MctpControl::new(router);
    let u = uuid::Uuid::new_v4();

    let types = [
        mctp::MCTP_TYPE_CONTROL,
        #[cfg(feature = "nvme-mi")]
        mctp::MCTP_TYPE_NVME,
    ];

    c.set_message_types(&types)?;
    c.set_uuid(&u);

    info!("MCTP Control Protocol server listening");
    let mut buf = [0u8; 256];
    loop {
        let Ok((_typ, _ic, msg, resp)) = l.recv(&mut buf).await else {
            continue;
        };

        let r = c.handle_async(msg, resp).await;

        match r {
            Err(e) => info!("control handler failure: {e}"),
            Ok(Some(ev)) => {
                let _ = ctrl_ev_sender.force_send(ev);
            }
            Ok(None) => (),
        }
    }
}

#[cfg(feature = "nvme-mi")]
async fn nvme_mi(router: &Router<'_>) -> std::io::Result<()> {
    let mut l = router.listener(mctp::MCTP_TYPE_NVME)?;

    let mut subsys = Subsystem::new(SubsystemInfo::environment());
    let ppid = subsys
        .add_port(PortType::Pcie(PciePort::new()))
        .expect("Unable to create PCIe port");
    let ctlrid = subsys
        .add_controller(ppid)
        .expect("Unable to create controller");
    let nsid = subsys
        .add_namespace(1024)
        .expect("Unable to create namespace");
    subsys
        .add_namespace(2048)
        .expect("Unable to create namespace");
    subsys
        .controller_mut(ctlrid)
        .attach_namespace(nsid)
        .unwrap_or_else(|_| {
            panic!(
                "Unable to attach namespace {nsid:?} to controller {ctlrid:?}"
            )
        });
    let twpid = subsys
        .add_port(PortType::TwoWire(TwoWirePort::new()))
        .expect("Unable to create TwoWire port");
    let mut mep = ManagementEndpoint::new(twpid);

    debug!("NVMe-MI endpoint listening");

    let mut buf = [0u8; 4224];
    loop {
        let Ok((_typ, ic, msg, resp)) = l.recv(&mut buf).await else {
            debug!("recv() failed");
            continue;
        };

        debug!("Handling NVMe-MI message: {msg:x?}");
        mep.handle_async(&mut subsys, msg, ic, resp, async |ce| match ce {
            nvme_mi_dev::CommandEffect::SetMtu { port_id, mtus } => {
                if port_id != twpid {
                    warn!("NVMe-MI: Bad Port ID for Set MTU: {port_id:?}");
                    return Err(CommandEffectError::InternalError);
                }

                if mtus != 64 {
                    warn!("NVMe-MI: Application lacks support for MTU ({mtus}) != BTU (64)");
                    return Err(CommandEffectError::Unsupported);
                }

                Ok(())
            }
            nvme_mi_dev::CommandEffect::SetSmbusFreq { port_id: _, freq } => {
                use nvme_mi_dev::nvme::mi::SmbusFrequency;

                if freq != SmbusFrequency::Freq100Khz {
                    warn!("NVMe-MI: Application lacks support for I2C bus frequency {:?}", freq);
                    return Err(CommandEffectError::Unsupported);
                }

                Ok(())
            }
        })
        .await;
    }
}
#[cfg(not(feature = "nvme-mi"))]
async fn nvme_mi(_router: &Router<'_>) -> std::io::Result<()> {
    futures::future::pending().await
}

#[cfg(feature = "pldm")]
mod pldm;
#[cfg(not(feature = "pldm"))]
mod pldm {
    pub async fn pldm(
        _router: &super::Router<'_>,
        _recv: async_channel::Receiver<super::ControlEvent>,
    ) -> std::io::Result<()> {
        futures::future::pending().await
    }
}

fn main() -> Result<()> {
    let opts: Options = argh::from_env();

    let conf = simplelog::ConfigBuilder::new().build();
    simplelog::SimpleLogger::init(LevelFilter::Debug, conf)?;

    let eid = Eid(0);

    let mut port_top = PortTop::new();
    let routes = Routes {};
    let mut router = Router::new(eid, &routes, 0);
    let port_id = router.add_port(&mut port_top)?;
    let port = router.port(port_id)?;

    let (transport, mut t_port) = match opts.transport {
        TransportSubcommand::Serial(s) => {
            let serial = serial::MctpSerial::new(&s.tty)?;
            info!("Created MCTP Serial transport on {}", s.tty);
            let t = Transport::Serial(serial);
            (t, None)
        }
        TransportSubcommand::Usb(u) => {
            let (usbredir, t_port) = usbredir::MctpUsbRedir::new(&u.path)?;
            info!("Created MCTP USB transport on {}", u.path);
            let t = Transport::Usb(usbredir);
            (t, Some(t_port))
        }
    };

    let fut = match t_port {
        Some(ref mut p) => futures::future::Either::Left(p.process()),
        None => futures::future::Either::Right(futures::future::pending()),
    };

    let (ctrl_ev_tx, ctrl_ev_rx) = async_channel::bounded(1);

    smol::block_on(async {
        select!(
            _ = fut.fuse() => (),
            _ = run(transport, port, &router).fuse() => (),
            _ = control(&router, ctrl_ev_tx).fuse() => (),
            _ = nvme_mi(&router).fuse() => (),
            _ = pldm::pldm(&router, ctrl_ev_rx).fuse() => (),
        );
        Ok::<_, anyhow::Error>(())
    })?;

    Ok(())
}
