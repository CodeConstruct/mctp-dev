
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

// owned version of mctp_estack::MctpMessage
#[derive(Debug)]
#[allow(unused)]
struct MctpMessage {
    pub source: Eid,
    pub dest: Eid,
    pub tag: Tag,
    pub typ: MsgType,
    pub ic: bool,

    payload: Vec<u8>,
}

/*
enum Transport {
    Serial(serial::MctpSerial),
    Usb(usbredir::MctpUsbRedir),
}

impl Transport {
    async fn recv(&mut self) -> Result<MctpMessage> {
        match self {
            Self::Serial(s) => s.recv().await,
            Self::Usb(u) => u.recv().await,
        }
    }
}
*/

impl MctpMessage {
    fn from_stack(src: &mctp_estack::MctpMessage) -> Self {
        let mut m = Self {
            source: src.source,
            dest: src.dest,
            tag: src.tag,
            typ: src.typ,
            ic: src.ic,
            payload: Vec::new(),
        };
        m.payload.extend_from_slice(src.payload);
        m
    }
}

async fn run(mut transport: usbredir::MctpUsbRedirListener)
-> std::io::Result<()> {
    loop {
        let mut rx_buf = [0u8; 4096];
        let (buf, mut resp, tag, typ, _ic) = transport.recv(&mut rx_buf).await?;
        info!("msg: {:?}", (&buf, tag, typ));
        match typ.0 {
            0 => (),
            1 => {
                let r = resp.send(typ, buf).await?;
                info!("send res: {r:?}");
            },
            _ => (),
        }
    }
}

fn main() -> Result<()> {
    let opts : Options = argh::from_env();
    
    let conf = simplelog::ConfigBuilder::new().build();
    simplelog::SimpleLogger::init(LevelFilter::Debug, conf)?;

    let transport = match opts.transport {
        TransportSubcommand::Serial(s) => {
            /*
            let serial = serial::MctpSerial::new(&s.tty)?;
            info!("Created MCTP Serial transport on {}", s.tty);
            Transport::Serial(serial)
            */
            unimplemented!();
        }
        TransportSubcommand::Usb(u) => {
            let usbredir = usbredir::MctpUsbRedirListener::new(Eid(9), &u.path)?;
            info!("Created MCTP USB transport on {}", u.path);
            usbredir
        }
    };

    smol::block_on(run(transport))?;

    Ok(())
}
