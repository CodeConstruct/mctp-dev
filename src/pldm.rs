// SPDX-License-Identifier: GPL-3.0

use anyhow::{Context, Result};
use log::{debug, info, warn};

use mctp_estack::{control::ControlEvent, router::Router};
use pldm::{control::requester::negotiate_transfer_parameters, PldmError};
use pldm_file::{
    client::{df_close, df_open, df_read_with},
    proto::{DfCloseAttributes, DfOpenAttributes, FileIdentifier},
};

async fn pldm_control(chan: &mut impl mctp::AsyncReqChannel) -> Result<()> {
    let req_types = [pldm_file::PLDM_TYPE_FILE_TRANSFER];
    let mut buf = [0u8];

    let (size, neg_types) =
        negotiate_transfer_parameters(chan, &req_types, &mut buf, 512)
            .await
            .context("Negotiate transfer parameters failed")?;

    debug!("Negotiated multipart size {size} for types {neg_types:?}");

    Ok(())
}

async fn pldm_pdr(
    mut _chan: &mut impl mctp::AsyncReqChannel,
) -> Result<(FileIdentifier, usize)> {
    Ok((FileIdentifier(0), 4096))
}

async fn pldm_file(
    chan: &mut impl mctp::AsyncReqChannel,
    file: FileIdentifier,
    _size: usize,
) -> Result<()> {
    let attrs = DfOpenAttributes::empty();
    let fd = df_open(chan, file, attrs).await.context("DfOpen failed")?;

    debug!("Open: {fd:?}");

    let mut buf = Vec::new();
    let req_len = 4096;

    debug!("Reading...");
    let res = df_read_with(chan, fd, 0, req_len, |part| {
        debug!("  {} bytes", part.len());
        if buf.len() + part.len() > req_len {
            warn!("  data overflow!");
            Err(PldmError::NoSpace)
        } else {
            buf.extend_from_slice(part);
            Ok(())
        }
    })
    .await;

    debug!("Read: {res:?}");

    let attrs = DfCloseAttributes::empty();
    let res = df_close(chan, fd, attrs).await;

    debug!("Close: {res:?}");

    Ok(())
}

// Run one PLDM session: perform base setup, find our file in the PDR and
// transfer it.
async fn pldm_session(mut chan: impl mctp::AsyncReqChannel) -> Result<()> {
    pldm_control(&mut chan)
        .await
        .context("PLDM control discovery failed")?;

    let (file_desc, file_size) = pldm_pdr(&mut chan)
        .await
        .context("PLDM PDR query for file info failed")?;

    pldm_file(&mut chan, file_desc, file_size)
        .await
        .context("PLDM file transfer failed")?;

    Ok(())
}

pub async fn pldm<'a>(
    router: &'a Router<'a>,
    ctrl_ev_receiver: async_channel::Receiver<ControlEvent>,
) -> std::io::Result<()> {
    loop {
        let peer = loop {
            let res = ctrl_ev_receiver.recv().await;

            if let Ok(ControlEvent::SetEndpointId { bus_owner, .. }) = res {
                info!("PLDM: new bus owner {bus_owner}");
                break bus_owner;
            };
        };

        let chan = router.req(peer);

        if let Err(e) = pldm_session(chan).await {
            warn!("PLDM session failed: {e}");
            info!("Restarting wait for EID");
        }
    }
}
