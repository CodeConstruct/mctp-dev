`mctp-dev`: MCTP device emulator
--------------------------------

`mctp-dev` is a small Linux application that implements a simple MCTP endpoint
device, connected to a usbredir (or serial) session.

Since the qemu emulator can speak usbredir too, this allows us to create a MCTP
device that can be connected to a qemu quest, and is intended for testing and
development of the guest system's MCTP stack.

The MCTP endpoint implements the MCTP control protocol, allowing device
discovery and enumeration. New MCTP-based protocols should be straightforward to
add to the `mctp-dev` framework.

When built with the `nvme-mi` feature, the MCTP endpoint also supports the
NVMe-MI protocol, emulating a simple MCTP-managed storage device. The NVMe-MI
responder implementation is provided by the
[`nvme-mi-dev`](https://github.com/CodeConstruct/nvme-mi-dev) crate.

# Building

For most systems:

```sh
cargo build
```

`mctp-dev` uses the `usbredir-rs` crate, which has a dependency on system C
libraries. You may need the `libusbredirparser-dev` and `libusbredirhost-dev`
packages installed for your distribution.

To incorporate NVMe-MI responder support, add the `nvme-mi` feature:

```sh
cargo build --features nvme-mi
```

We also support a test client for PLDM for File Transfer (type 7). When
`mctp-dev` is assigned an MCTP EID, it will perform PLDM operations to
read a file from the bus owner EID.

The PLDM client expects to find the File record as the first PDR entry, and
will attempt to transfer the entire file. Upon completion, the file size and
sha256 checksum will be printed:

```
11:06:34 [INFO] Transfer complete. 16384 bytes, sha256 b4d3f1859dc8170c1e1f34b936aff05339a7723b6680894380c23dd84ff7e22b
```

To enable the PLDM File functionality, add the `pldm` feature:

```sh
cargo build --features pldm
```

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


