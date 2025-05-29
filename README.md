`mctp-dev`: MCTP device emulator
--------------------------------

`mctp-dev` implements a simple MCTP endpoint device, connected to a usbredir
session. This allows connection to qemu guests, as a simple external MCTP
device, and is intended for testing and development of the system MCTP stack.

The MCTP endpoint implements the MCTP control protocol, allowing device
discovery and enumeration.

# Building

For most systems:

```sh
cargo build
```

`mctp-dev` uses the `usbredir-rs` crate, which has a dependency on system C
libraries. You may need the `libusbredirparser-dev` and `libusbredirhost-dev`
packages installed for your distribution.

# Running

1. Run `qemu` with a usbredir connection to a pty:

        qemu-system-arm [...] -chardev pty,id=usbredir -device usb-redir,chardev=usbredir

   on starting, qemu will log a message providing a path to the new pty device:

        char device redirected to /dev/pts/0 (label usbredir)

2. Run `mctp-dev`, specifying the pty from above

        $ mctp-dev usb /dev/pts/0
        07:56:14 [INFO] Created MCTP USB transport on /dev/pts/0
        07:56:14 [INFO] MCTP Control Protocol server listening

Once the qemu guest is running, you will have an emulated USB device present:

```sh
# lsusb
Bus 001 Device 001: ID 1d6b:0001 Linux Foundation 1.1 root hub
Bus 002 Device 001: ID 1d6b:0002 Linux Foundation 2.0 root hub
Bus 002 Device 002: ID 0000:0000 mctp-dev MCTP over USB device
```

Provided you have a kernel with the MCTP-over-USB drivers present, you will also
have a MCTP link available:

```sh
# mctp link
dev lo index 1 address 00:00:00:00:00:00 net 1 mtu 65536 up
dev mctpusb0 index 6 address none net 1 mtu 68 down
```

To bring up the link and enumerate the device:

```
# mctp addr add 8 dev mctpusb0
# mctp link set mctpusb0 up
# systemctl start mctpd
# busctl call \
  au.com.codeconstruct.MCTP1 \
  /au/com/codeconstruct/mctp1/interfaces/mctpusb0 \
  au.com.codeconstruct.MCTP.BusOwner1 \
  SetupEndpoint ay 0
yisb 9 1 "/au/com/codeconstruct/mctp1/networks/1/endpoints/9" true
```


