
use anyhow::{Context, Result};
use mctp_estack::{Stack, SendOutput, usb::MctpUsbHandler, usb::MctpUsbXfer};
#[allow(unused_imports)]
use log::{debug, info, trace, warn};
use usbredirparser::{self, Parser};
use std::io::{Read as _, Write as _};
use futures::{FutureExt, select, future};
use std::collections::VecDeque;
use std::pin::Pin;

struct UsbRedirHandler {
    stream: std::fs::File,
    out_chan: async_channel::Sender<Vec<u8>>,
    in_chan: async_channel::Sender<(u64, usbredirparser::BulkPacket)>,
}

const USB_CLASS_MCTP: u8 = 0x14;
const USB_PROTO_MCTP_V1: u8 = 1;

const USB_CTRL_GET_DESCRIPTOR: u8 = 6;

const EP_ADDR_OUT: u8 = 0x01;
const EP_ADDR_IN: u8 = 0x81;

const USB_XFER_SIZE: usize = 512;

/* contains the usbredir state, and handles async processing */
pub struct MctpUsbRedirPort {
    parser: Pin<Box<usbredirparser::Parser>>,
    stream: smol::Async<std::fs::File>,
    in_xfer_queue: VecDeque<(u64, usbredirparser::BulkPacket)>,

    /* usbredir interactions, connected to the usbredir handler. We use a
     * channel for this as the handler object gets stashed away within the
     * usbredirparser callback API
     */
    redir_out_chan: async_channel::Receiver<Vec<u8>>,
    redir_in_chan: async_channel::Receiver<(u64, usbredirparser::BulkPacket)>,

    /* usb transfer interactions, connected to the higher-level objects */
    xfer_tx_chan: async_channel::Receiver<Vec<u8>>,
    xfer_rx_chan: async_channel::Sender<Vec<u8>>,
}

#[allow(unused)]
pub struct MctpUsbRedir {
    // mctp: mctp_estack::Stack,
    // start_time: Instant,
    mctpusb: MctpUsbHandler,
    rx_buf: [u8; USB_XFER_SIZE],
    rx_remain: std::ops::Range<usize>,

    xfer_tx_chan: async_channel::Sender<Vec<u8>>,
    xfer_rx_chan: async_channel::Receiver<Vec<u8>>,
}

impl usbredirparser::ParserHandler for UsbRedirHandler {
    fn read(&mut self, _parser: &Parser, buf: &mut [u8]) -> std::io::Result<usize> {
        let res = self.stream.read(buf);
        trace!("read:in:{:x?}", buf);
        match res {
            Ok(0) => Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "disconnected",
                      )),
            r => r,
        }
    }
    fn write(&mut self, _parser: &Parser, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buf)
    }
    fn hello(&mut self, parser: &Parser, hello: &usbredirparser::Hello) {
        debug!("hello: {:?}", hello.version);
        self.send_config(parser);

        let chdr = usbredirparser::DeviceConnect {
            speed: usbredirparser::SPEED_HIGH,
            device_class: 0,
            device_subclass: 0,
            device_protocol: 0,
            vendor_id: 0xcc00,
            product_id: 0xcc00,
            device_version_bcd: 0x0,
        };
        parser.send_device_connect(&chdr)
    }

    fn reset(&mut self, _parser: &Parser) {
        debug!("reset");
    }

    fn control_packet(&mut self, parser: &Parser, id: u64,
        pkt: &usbredirparser::ControlPacket, data: &[u8]) {
        debug!("control packet {id} {pkt:x?}, data: {data:x?}");
        if pkt.request == USB_CTRL_GET_DESCRIPTOR {
            self.control_get_descriptor(parser, id, pkt)
        }
    }

    fn bulk_packet(&mut self, parser: &Parser, id: u64,
        pkt: &usbredirparser::BulkPacket, data: &[u8]) {
        debug!("bulk packet {id} {pkt:x?}, data: {data:x?}");
        match pkt.endpoint {
            EP_ADDR_IN => {
                self.in_chan.send_blocking((id, *pkt))
                    .expect("can't send to in channel");
            }
            EP_ADDR_OUT => {
                let mut v = Vec::with_capacity(data.len());
                v.extend_from_slice(data);
                self.out_chan.send_blocking(v)
                    .expect("can't send to out channel");
                /* ack */
                let resp = usbredirparser::BulkPacket {
                    status: 0,
                    length: 0,
                    length_high: 0,
                    .. *pkt
                };
                parser.send_bulk_packet(id, &resp, &[]);
            }
            _ => {
                warn!("unknown bulk packet for ep {:02x}", pkt.endpoint);
            }
        }
    }

    fn cancel_data_packet(&mut self, _parser: &Parser, id: u64) {
        debug!("cancel packet {id}");
    }

    fn set_configuration(
        &mut self,
        parser: &Parser,
        id: u64,
        cfg: &usbredirparser::SetConfiguration)
    {
        debug!("set configuration {}", cfg.configuration);

        let mut cfg_status = usbredirparser::ConfigurationStatus {
            configuration: cfg.configuration,
            status: 1,
        };

        if cfg.configuration == 1 {
            self.send_config(parser);
            cfg_status.status = 0;
        }

        parser.send_configuration_status(id, &cfg_status)
    }
}

const USB_DESC_TYPE_DEVICE : u8 = 1;
const USB_DESC_TYPE_CONFIGURATION : u8 = 2;
const USB_DESC_TYPE_STRING : u8 = 3;
const USB_DESC_TYPE_INTERFACE : u8 = 4;
const USB_DESC_TYPE_ENDPOINT : u8 = 5;

const DEV_DESC : [u8; 18] = [
    18, /* bLength */
    USB_DESC_TYPE_DEVICE, /* bDescriptorTYpe */
    0x00, 0x02, /* bcdUSB */
    0x00, /* bDeviceClass */
    0x00, /* bDeviceSubClass */
    0x00, /* bDeviceProtocol */
    0x40, /* bMaxPacketSize0 */
    0x00, 0xcc, /* idVendor */
    0x00, 0xcc, /* idProduct */
    0x13, 0x06, /* bcdDevice */
    0x01, /* iManufacturer */
    0x02, /* iProduct */
    0x03, /* iSerialNumber */
    0x01, /* bNumConfigurations */
];

const CONFIG_DESC : [u8; 9] = [
    0x09, /* bLength */
    USB_DESC_TYPE_CONFIGURATION, /* bDescriptorType */
    0x00, 0x00, /* wTotalLength */
    0x01, /* bNumInterfaces */
    0x01, /* bConfigurationValue */
    0x00, /* iConfiguration */
    0x80, /* bmAttributes: bus powered */
    0x01, /* bMaxPower: 2ma */
];

const IFACE_DESC :  [u8; 9] = [
    0x09, /* bLength */
    USB_DESC_TYPE_INTERFACE, /* bDescriptorType */
    0x00, /* bInterfaceNumber */
    0x00, /* bAlternateSetting */
    0x02, /* bNumEndpoints */
    USB_CLASS_MCTP, /* bInterfaceClass */
    0x00, /* bInterfaceSubClass */
    USB_PROTO_MCTP_V1, /* bInterfaceProtocol */
    0x04, /* iInterface */
];

const EP_DESCS : [[u8; 7]; 2] = [
    [
        0x07, /* bLength */
        USB_DESC_TYPE_ENDPOINT, /* bDescriptorType */
        EP_ADDR_OUT, /* bEndpointAddress */
        0x02, /* bmAttributes */
        0x00, 0x02, /* wMaxPacketSize */
        0 /* bInterval */
    ],
    [
        0x07, /* bLength */
        USB_DESC_TYPE_ENDPOINT, /* bDescriptorType */
        EP_ADDR_IN, /* bEndpointAddress */
        0x02, /* bmAttributes */
        0x00, 0x02, /* wMaxPacketSize: 512 */
        0 /* bInterval */
    ],

];

const STRINGS : &[&str] = &[
    "Code Construct",
    "MCTP over USB device",
    "sn0000",
    "MCTP over USB",
];

const STRING_LANGS : [u8; 4] = [
    4, /* bLength */
    USB_DESC_TYPE_STRING, /* bDescriptorType */
    0x09, 0x04, /* en */
];

impl UsbRedirHandler {
    fn control_get_descriptor(
        &mut self,
        parser: &Parser,
        id: u64,
        req: &usbredirparser::ControlPacket
    ) {
        let mut resp = usbredirparser::ControlPacket { ..*req };
        let (desc_type, desc_idx) : (u8, u8) = (
            ((req.value >> 8) & 0xff) as u8,
            ((req.value     ) & 0xff) as u8
        );
        debug!("desc request for type {desc_type:02x} idx {desc_idx:02x}");
        let mut v = Vec::new();
        let mut data = match desc_type {
            USB_DESC_TYPE_DEVICE => {
                DEV_DESC.as_slice()
            },
            USB_DESC_TYPE_STRING => {
                let s_idx = desc_idx as usize;
                if s_idx == 0 {
                    STRING_LANGS.as_slice()
                } else if s_idx - 1 < STRINGS.len() {
                    v.extend_from_slice(&[0, USB_DESC_TYPE_STRING]);
                    for b in STRINGS[s_idx - 1].encode_utf16() {
                        v.extend_from_slice(&b.to_le_bytes());
                    }
                    v[0] = (v.len() & 0xff) as u8;
                    v.as_slice()
                } else {
                    &[]
                }
            }
            USB_DESC_TYPE_CONFIGURATION => {
                v.extend_from_slice(&CONFIG_DESC);
                v.extend_from_slice(&IFACE_DESC);
                v.extend_from_slice(&EP_DESCS[0]);
                v.extend_from_slice(&EP_DESCS[1]);
                /* set total length */
                let len = v.len() as u16;
                v[3] = ((len >> 8) & 0xff) as u8;
                v[2] = ((len     ) & 0xff) as u8;
                v.as_slice()
            }
            _ => {
                warn!("unsupported descriptor {desc_type:02x}");
                &[]
            }
        };
        let req_len = req.length as usize;
        if req_len < data.len() {
            data = &data[..req_len];
        }
        resp.length = data.len() as u16;
        parser.send_control_packet(id, &resp, data)
    }

    fn send_config(&mut self, parser: &Parser) {
        let mut if_info = usbredirparser::InterfaceInfo {
            interface_count: 0,
            interface: [0; 32],
            interface_class: [0; 32],
            interface_subclass: [0; 32],
            interface_protocol: [0; 32],
        };
        if_info.interface_count = 1;
        if_info.interface[0] = 0;
        if_info.interface_class[0] = USB_CLASS_MCTP;
        if_info.interface_protocol[0] = USB_PROTO_MCTP_V1;
        parser.send_interface_info(&if_info);

        let mut ep_info = usbredirparser::EPInfo {
            type_: [usbredirparser::TYPE_INVALID; 32],
            interval: [0; 32],
            interface: [0; 32],
            max_packet_size: [0; 32],
            max_streams: [0; 32],
        };
        /* control */
        ep_info.type_[0] = usbredirparser::TYPE_CONTROL;
        ep_info.max_packet_size[0] = 16;
        ep_info.type_[16] = usbredirparser::TYPE_CONTROL;
        ep_info.max_packet_size[16] = 16;
        /* bulk in/out */
        ep_info.type_[1] = usbredirparser::TYPE_BULK;
        ep_info.max_packet_size[1] = 512;
        ep_info.type_[17] = usbredirparser::TYPE_BULK;
        ep_info.max_packet_size[17] = 512;
        parser.send_ep_info(&ep_info);
    }
}

impl MctpUsbRedir {

    pub fn new(path: &str) -> Result<(Self, MctpUsbRedirPort)> {
        let fd = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .open(path)
            .context("Can't open tty device")?;

        let fd2 = fd.try_clone()?;
        let (redir_out_sender, redir_out_receiver) = async_channel::unbounded();
        let (redir_in_sender, redir_in_receiver) = async_channel::unbounded();

        let handler = UsbRedirHandler {
            out_chan: redir_out_sender,
            in_chan: redir_in_sender,
            stream: fd
        };
        let parser = usbredirparser::Parser::new(
            handler,
            usbredirparser::DeviceType::Host,
        );

        let (xfer_out_sender, xfer_out_receiver) = async_channel::unbounded();
        let (xfer_in_sender, xfer_in_receiver) = async_channel::unbounded();
        let port = MctpUsbRedirPort {
            parser,
            stream: smol::Async::new(fd2)?,
            in_xfer_queue: VecDeque::new(),
            redir_out_chan: redir_out_receiver,
            redir_in_chan: redir_in_receiver,
            xfer_rx_chan: xfer_out_sender,
            xfer_tx_chan: xfer_in_receiver,
        };

        Ok((Self {
            mctpusb: MctpUsbHandler::new(),
            rx_buf: [0u8; USB_XFER_SIZE ],
            rx_remain: std::ops::Range { start: 0, end: 0 },
            xfer_tx_chan: xfer_in_sender,
            xfer_rx_chan: xfer_out_receiver,
        }, port))
    }

    pub async fn recv(&mut self) -> mctp::Result<&[u8]> {
        if self.rx_remain.is_empty() {
            let r = self.xfer_rx_chan.recv().await
                .or(Err(mctp::Error::RxFailure))?;
            let len = r.len();
            if len > self.rx_buf.len() {
                return Err(mctp::Error::RxFailure);
            }
            self.rx_buf.split_at_mut(len).0.clone_from_slice(&r);
            self.rx_remain = std::ops::Range { start: 0, end: len };
        }

        let data = &self.rx_buf[self.rx_remain.clone()];
        match MctpUsbHandler::decode(data) {
            Ok((pkt, rem)) => {
                self.rx_remain.start = self.rx_remain.end - rem.len();
                Ok(pkt)
            }
            Err(_) => {
                self.rx_remain = std::ops::Range { start: 0, end: 0 };
                Err(mctp::Error::RxFailure)
            }
        }

    }

    pub async fn send(&mut self, pkt: &[u8]) -> mctp::Result<()> {
        let total = pkt.len().checked_add(4).ok_or(mctp::Error::NoSpace)?;
        let mut tx_buf = Vec::with_capacity(total);
        let mut hdr = [0u8; 4];
        MctpUsbHandler::header(pkt.len(), &mut hdr)?;
        tx_buf.extend_from_slice(&hdr);
        tx_buf.extend_from_slice(pkt);
        self.xfer_tx_chan.send(tx_buf).await.or(Err(mctp::Error::TxFailure))?;
        Ok(())
    }
}

impl MctpUsbRedirPort {

    async fn process_one(&mut self) -> mctp::Result<()> {
        // we only poll on the tx future (outgoing USB transfers from the MCTP
        // stack) if we have a usbredir IN transfer queued and ready to go.
        let tx_fut = if self.in_xfer_queue.is_empty() {
            future::Either::Left(future::pending())
        } else {
            future::Either::Right(self.xfer_tx_chan.recv())
        };

        select!(
            // socket activity
            r = self.stream.readable().fuse() => {
                if let Err(e) = r {
                    warn!("io error {e:?}");
                    return Err(mctp::Error::RxFailure);
                }

                let res = self.parser.do_read();
                if let Err(e) = res {
                    warn!("parse error {e:?}");
                    return Err(mctp::Error::RxFailure);
                }
            },

            // tx from redir
            r = self.redir_out_chan.recv().fuse() => {
                if let Ok(xfer) = r {
                    let _ = self.xfer_rx_chan.send(xfer).await;
                }
            }

            // rx from redir
            r = self.redir_in_chan.recv().fuse() => {
                if let Ok((id, pkt)) = r {
                    self.in_xfer_queue.push_back((id, pkt));
                }
            }

            // tx from MCTP stack
            r = tx_fut.fuse() => {
                if let Ok(xfer) = r {
                    // unwrap(): we have already confirmed we have an entry in
                    // the in_xfer_queue
                    let (id, mut pkt) = self.in_xfer_queue.pop_front().unwrap();

                    pkt.status = usbredirparser::STATUS_SUCCESS;
                    pkt.length = xfer.len() as u16;

                    debug!("tx xfer: {xfer:02x?}");
                    self.parser.send_bulk_packet(id, &pkt, &xfer);
                } else {
                    warn!("tx/xfer failure: {r:?}");
                    return Err(mctp::Error::TxFailure);
                }

            }
        );

        while self.parser.has_data_to_write() != 0 {
            let res = self.parser.do_write();
            if let Err(e) = res {
                warn!("write error {e:?}");
                break;
            }
        }
        Ok(())
    }

    pub async fn process(&mut self) -> mctp::Result<()> {
        loop {
            self.process_one().await?
        }

    }
}
