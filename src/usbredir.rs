
use anyhow::{Context, Result};
use mctp::Eid;
use mctp_estack::{Stack, SendOutput, usb::MctpUsbHandler, usb::MctpUsbXfer};
use std::time::Instant;
#[allow(unused_imports)]
use log::{debug, info, trace, warn};
use usbredirparser::{self, Parser};
use std::io::{Read as _, Write as _};
use futures::{FutureExt, select};

struct Handler {
    stream: std::fs::File,
    out_chan: async_channel::Sender<Vec<u8>>,
    in_chan: async_channel::Sender<(u64, usbredirparser::BulkPacket)>,
}

const USB_CLASS_MCTP: u8 = 0x14;
const USB_PROTO_MCTP_V1: u8 = 1;

const USB_CTRL_GET_DESCRIPTOR: u8 = 6;

const EP_ADDR_OUT: u8 = 0x01;
const EP_ADDR_IN: u8 = 0x81;

#[allow(unused)]
pub struct MctpUsbRedir {
    mctp: mctp_estack::Stack,
    mctpusb: MctpUsbHandler,
    start_time: Instant,
    parser: Box<usbredirparser::Parser>,
    stream: smol::Async<std::fs::File>,
    usb_out_chan: async_channel::Receiver<Vec<u8>>,
    usb_in_chan: async_channel::Receiver<(u64, usbredirparser::BulkPacket)>,
}

impl usbredirparser::ParserHandler for Handler {
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

impl Handler {
    fn control_get_descriptor(
        &mut self,
        parser: &Parser,
        id: u64,
        req: &usbredirparser::ControlPacket
    ) {
        let mut resp = usbredirparser::ControlPacket { ..*req };
        let (desc_type, desc_idx) : (u8, u8) = (
            ((req.value >> 8) & 0xff) as u8,
            ((req.value >> 0) & 0xff) as u8
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
                v[3] = (len >> 8) as u8 & 0xff;
                v[2] = (len >> 0) as u8 & 0xff;
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

    pub fn new(own_eid: mctp::Eid, path: &str) -> Result<Self> {
        let fd = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .open(path)
            .context("Can't open tty device")?;

        let start_time = Instant::now();
        let mtu = 64;
        let eid = own_eid;
        let mctp = Stack::new(eid, mtu, 0);

        let fd2 = fd.try_clone()?;
        let (out_sender, out_receiver) = async_channel::unbounded();
        let (in_sender, in_receiver) = async_channel::unbounded();

        let handler = Handler {
            out_chan: out_sender,
            in_chan: in_sender,
            stream: fd
        };
        let parser = usbredirparser::Parser::new(handler);

        Ok(Self {
            mctp,
            mctpusb: MctpUsbHandler::new(),
            start_time,
            parser,
            stream: smol::Async::new(fd2)?,
            usb_out_chan: out_receiver,
            usb_in_chan: in_receiver,
        })
    }

    fn now(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    pub fn disconnect(&mut self) {
        self.parser.send_device_disconnect();

        while self.parser.has_data_to_write() != 0 {
            let res = self.parser.do_write();
            match res {
                Err(e) => {
                    warn!("write error {e:?}");
                    break;
                }
                Ok(_) => (),
            }
        }
    }

    pub async fn recv(&mut self)
    -> mctp::Result<(mctp_estack::MctpMessage, mctp_estack::ReceiveHandle)> {
        loop {
            select!(
                r = self.stream.readable().fuse() => {
                    if let Err(e) = r {
                        warn!("io error {e:?}");
                        break Err(mctp::Error::RxFailure);
                    }

                    let res = self.parser.do_read();
                    match res {
                        Err(e) => {
                            warn!("parse error {e:?}");
                            break Err(mctp::Error::RxFailure);
                        }
                        Ok(_) => (),
                    }

                },
                r = self.usb_out_chan.recv().fuse() => {
                    trace!("out_chan: {r:?}");

                    let _ = self.mctp.update(self.now());
                    if let Ok(data) = r {
                        let m = MctpUsbHandler::receive(
                            data.as_slice(),
                            &mut self.mctp,
                        );
                        if let Ok(Some((_msg, handle))) = m {
                            let msg = self.mctp.fetch_message(&handle);
                            debug!("msg: {msg:?}");
                            return Ok((msg, handle));
                        }
                    }
                }
            );
            if self.parser.has_data_to_write() != 0 {
                let res = self.parser.do_write();
                match res {
                    Err(e) => {
                        warn!("write error {e:?}");
                        break Err(mctp::Error::RxFailure);
                    }
                    Ok(_) => (),
                }
            }
        }
    }

    async fn send_vectored(
        &mut self,
        eid: Eid,
        typ: mctp::MsgType,
        tag: Option<mctp::Tag>,
        integrity_check: bool,
        bufs: &[&[u8]],
    ) -> Result<mctp::Tag> {
        let _ = self.mctp.update(self.now());
        let cookie = None;
        let mut buf = Vec::new();
        for b in bufs {
            buf.extend_from_slice(b);
        }
        let mut xfer = MctpUsbRedirXfer {
            parser: &self.parser,
            chan: &self.usb_in_chan,
        };
        let r = self.mctpusb.send_fill(eid, typ,
            tag, integrity_check, cookie,
            &mut xfer, &mut self.mctp,
            |v| {
                for b in bufs {
                    v.extend_from_slice(b).ok()?
                }
                trace!("v len {}", v.len());
                Some(())
            });


        match r {
            SendOutput::Packet(_) => unreachable!(),
            SendOutput::Complete { tag, .. } => Ok(tag),
            SendOutput::Error { err, .. } => Err(err.into()),
        }
    }
}

pub struct MctpUsbRedirListener {
    usb: MctpUsbRedir,
}

impl MctpUsbRedirListener {
    pub fn new(own_eid: mctp::Eid, path: &str) -> Result<Self> {
        Ok(Self {
            usb: MctpUsbRedir::new(own_eid, path)?,
        })
    }

    pub fn disconnect(&mut self) {
        self.usb.disconnect();
    }
}

pub struct MctpUsbRedirReq {
    usb: MctpUsbRedir,
    eid: mctp::Eid,
    sent_tv: Option<mctp::TagValue>,
    timeout: Option<core::time::Duration>
}

impl mctp::AsyncReqChannel for MctpUsbRedirReq {
    fn remote_eid(&self) -> mctp::Eid {
        return self.eid
    }
    async fn recv<'f>(
        &mut self,
        _buf: &'f mut [u8],
    ) -> mctp::Result<(&'f mut [u8], mctp::MsgType, mctp::Tag, bool)> {
        todo!();
    }
    async fn send_vectored(
        &mut self,
        _typ: mctp::MsgType,
        _integrity_check: bool,
        _bufs: &[&[u8]],
    ) -> mctp::Result<()> {
        todo!();
    }
}

pub struct MctpUsbRedirResp<'a> {
    eid: mctp::Eid,
    tv: mctp::TagValue,
    usb: &'a mut MctpUsbRedir,
}

impl mctp::AsyncRespChannel for MctpUsbRedirResp<'_> {
    type ReqChannel<'a> = MctpUsbRedirReq where Self: 'a;

    async fn send_vectored(
        &mut self,
        typ: mctp::MsgType,
        integrity_check: bool,
        bufs: &[&[u8]],
    ) -> mctp::Result<()> {
        let _ = self.usb.mctp.update(self.usb.now());
        let cookie = None;
        let mut xfer = MctpUsbRedirXfer {
            parser: &self.usb.parser,
            chan: &self.usb.usb_in_chan,
        };
        let r = self.usb.mctpusb.send_fill(self.eid, typ,
            Some(mctp::Tag::Unowned(self.tv)), integrity_check, cookie,
            &mut xfer, &mut self.usb.mctp,
            |v| {
                for b in bufs {
                    v.extend_from_slice(b).ok()?
                }
                trace!("v len {}", v.len());
                Some(())
            });

        match r {
            SendOutput::Packet(_) => unreachable!(),
            SendOutput::Complete { .. } => Ok(()),
            SendOutput::Error { err, .. } => Err(err),
        }
    }
    fn req_channel(&self) -> mctp::Result<Self::ReqChannel<'_>> {
        todo!();
    }
    fn remote_eid(&self) -> mctp::Eid {
        todo!();
    }
}

impl mctp::AsyncListener for MctpUsbRedirListener {
    type RespChannel<'a> = MctpUsbRedirResp<'a> where Self: 'a;

    async fn recv<'f>(&mut self, buf: &'f mut [u8])
    -> mctp::Result<(&'f mut [u8], Self::RespChannel<'_>, mctp::Tag, mctp::MsgType, bool)> {
        loop {
            let (msg, handle) = self.usb.recv().await?;

            if msg.tag.is_owner() {
                let tag = msg.tag;
                let ic = msg.ic;
                let typ = msg.typ;
                let b = buf.get_mut(..msg.payload.len()).ok_or(mctp::Error::NoSpace)?;
                b.copy_from_slice(msg.payload);
                let eid = msg.source;
                self.usb.mctp.finished_receive(handle);
                let resp = MctpUsbRedirResp {
                    eid,
                    tv: tag.tag(),
                    usb: &mut self.usb,
                };
                return Ok((b, resp, tag, typ, ic));
            } else {
                trace!("Discarding unmatched message {msg:?}");
                self.usb.mctp.finished_receive(handle);
            }
        }
    }
}

/*
 * Facility for sending through the MctpUsbXfer trait. We split the
 * parser and channel out of the core MctpUsbRedir to satisfy borrow
 * rules
 */
struct MctpUsbRedirXfer<'a> {
    parser: &'a usbredirparser::Parser,
    chan: &'a async_channel::Receiver<(u64, usbredirparser::BulkPacket)>,
}

impl MctpUsbXfer for MctpUsbRedirXfer<'_> {
    fn send_xfer(&mut self, buf: &[u8]) -> mctp::Result<()> {
        debug!("USB pkt xfer: {buf:x?}");
        let res = self.chan.try_recv();
        let (id, mut pkt) = match res {
            Err(_) => {
                debug!("no in urb available");
                return Err(mctp::Error::TxFailure);
            }
            Ok(p) => p,
        };

        pkt.status = usbredirparser::STATUS_SUCCESS;
        pkt.length = buf.len() as u16;

        let r = self.parser.send_bulk_packet(id, &pkt, buf);

        if self.parser.has_data_to_write() != 0 {
            let res = self.parser.do_write();
            match res {
                Err(e) => {
                    warn!("write error {e:?}");
                    return Err(mctp::Error::RxFailure.into());
                }
                Ok(_) => (),
            }
        }

        Ok(())
    }
}
