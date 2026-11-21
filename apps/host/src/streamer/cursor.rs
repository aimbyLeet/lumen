//! The pointer to the client: its position and shapes, out ahead of frame data.

use anyhow::Result;
use protocol::{CursorShape, Flags, PacketKind};
use transport::{PacedHandle, Packetizer};

use super::Shared;
use crate::control::{SharedSealer, control_datagram, seal};

/// The pointer's position (and a new shape) to the client, ahead of frame data.
pub(super) fn send_cursor(
    shared: &Shared,
    packetizer: &mut Packetizer,
    update: crate::cursor::CursorUpdate,
) -> Result<()> {
    if let Some(shape) = &update.new_shape {
        send_cursor_shape(packetizer, &shared.out, &shared.sealer, shape)?;
    }
    shared.out.send_now(control_datagram(
        &shared.seq,
        shared.session,
        &shared.sealer,
        &update.position,
    ));
    Ok(())
}

/// Cursor shapes travel like video payloads (fragmented, with FEC), ahead of frame data.
pub(crate) fn send_cursor_shape(
    packetizer: &mut Packetizer,
    out: &PacedHandle,
    sealer: &SharedSealer,
    shape: &CursorShape,
) -> Result<()> {
    let mut payload = Vec::with_capacity(shape.data.len() + 32);
    shape.encode(&mut payload);
    let mut dgrams = Vec::new();
    packetizer.packetize(
        PacketKind::CursorShape,
        0,
        shape.id,
        Flags::empty(),
        0,
        &payload,
        &mut dgrams,
    )?;
    for mut d in dgrams {
        // Not through the pacer: numbered here if the packetizer leaves the
        // numbers to the pacer (a stream's).
        if packetizer.defers_seq() {
            renumber(&mut d, packetizer.take_seq());
        }
        seal(sealer, &mut d);
        out.send_now(d);
    }
    Ok(())
}

/// Puts sequence number `seq` into a datagram's header (before sealing).
pub(crate) fn renumber(d: &mut [u8], seq: u64) {
    if let Ok((mut h, _)) = protocol::PacketHeader::decode(d) {
        h.seq = seq;
        let mut head = Vec::with_capacity(protocol::HEADER_LEN);
        h.encode(&mut head);
        d[..protocol::HEADER_LEN].copy_from_slice(&head);
    }
}
