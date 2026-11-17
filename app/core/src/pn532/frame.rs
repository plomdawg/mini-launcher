//! PN532 HSU frame codec (normal information frames, ACK/NACK, error frame).
//!
//! Pure functions over byte slices — no I/O. Layout of a normal information frame:
//!
//! ```text
//! 00 00 FF LEN LCS TFI PD0..PDn DCS 00
//! ```
//!
//! * `LEN` counts `TFI` plus the data bytes; `LEN + LCS == 0 (mod 256)`.
//! * `TFI` is `D4` host → PN532 and `D5` PN532 → host.
//! * `TFI + sum(PD) + DCS == 0 (mod 256)`.
//!
//! Extended information frames (`00 00 FF FF FF ...`) are not supported.

use thiserror::Error;

/// Frame identifier for host → PN532 frames.
pub const TFI_HOST_TO_PN532: u8 = 0xD4;
/// Frame identifier for PN532 → host frames.
pub const TFI_PN532_TO_HOST: u8 = 0xD5;
/// Frame identifier of the PN532 application-level error frame.
pub const TFI_ERROR: u8 = 0x7F;

/// ACK frame.
pub const ACK: [u8; 6] = [0x00, 0x00, 0xFF, 0x00, 0xFF, 0x00];
/// NACK frame (asks the other side to resend its last frame).
pub const NACK: [u8; 6] = [0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00];
/// Application-level error frame sent by the PN532.
pub const ERROR_FRAME: [u8; 8] = [0x00, 0x00, 0xFF, 0x01, 0xFF, 0x7F, 0x81, 0x00];

/// HSU wakeup sequence: `0x55 0x55` followed by a long run of zeros. Send it
/// immediately before the first command (normally `SAMConfiguration`) to wake
/// a PN532 that is in power-down / low-VBAT mode.
pub const WAKEUP: [u8; 16] = [
    0x55, 0x55, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// Maximum number of data bytes (command code + parameters, excluding `TFI`)
/// in a normal information frame.
pub const MAX_DATA_LEN: usize = 254;

/// Errors produced by the frame codec.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FrameError {
    /// Payload does not fit in a normal information frame.
    #[error("frame data too long: {0} bytes (max {MAX_DATA_LEN})")]
    DataTooLong(usize),
    /// No `00 FF` start code found in the buffer.
    #[error("no frame start code found")]
    NoStartCode,
    /// A start code was found but the frame is not complete yet.
    #[error("incomplete frame")]
    Incomplete,
    /// `LEN + LCS` is not zero (mod 256).
    #[error("bad length checksum (LEN {len:#04x}, LCS {lcs:#04x})")]
    BadLengthChecksum { len: u8, lcs: u8 },
    /// `TFI + sum(PD) + DCS` is not zero (mod 256).
    #[error("bad data checksum")]
    BadDataChecksum,
    /// `LEN` is zero, so there is no `TFI`.
    #[error("frame has zero length")]
    EmptyFrame,
    /// Extended information frames are not supported.
    #[error("extended information frames are not supported")]
    ExtendedFrame,
    /// The frame is not addressed to the host (`TFI` is not `D5` / `7F`).
    #[error("unexpected frame identifier {0:#04x}")]
    UnexpectedTfi(u8),
}

/// A decoded host-bound frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// ACK frame.
    Ack,
    /// NACK frame.
    Nack,
    /// Application-level error frame.
    Error,
    /// Normal information frame with `TFI` `D5`; holds `PD0..PDn`
    /// (response code followed by its data).
    Response(Vec<u8>),
}

/// Encodes a host → PN532 normal information frame. `data` is `PD0..PDn`:
/// the command code followed by its parameters (without `TFI`).
pub fn encode(data: &[u8]) -> Result<Vec<u8>, FrameError> {
    if data.len() > MAX_DATA_LEN {
        return Err(FrameError::DataTooLong(data.len()));
    }
    // Fits: data.len() + 1 <= 255.
    let len = (data.len() + 1) as u8;
    let sum = data
        .iter()
        .fold(TFI_HOST_TO_PN532, |acc, &b| acc.wrapping_add(b));

    let mut out = Vec::with_capacity(data.len() + 8);
    out.extend_from_slice(&[0x00, 0x00, 0xFF, len, len.wrapping_neg(), TFI_HOST_TO_PN532]);
    out.extend_from_slice(data);
    out.push(sum.wrapping_neg());
    out.push(0x00);
    Ok(out)
}

/// Returns [`WAKEUP`] followed by the encoded command frame.
pub fn encode_with_wakeup(data: &[u8]) -> Result<Vec<u8>, FrameError> {
    let frame = encode(data)?;
    let mut out = Vec::with_capacity(WAKEUP.len() + frame.len());
    out.extend_from_slice(&WAKEUP);
    out.extend_from_slice(&frame);
    Ok(out)
}

/// Decodes the first host-bound frame in `buf`.
///
/// Leading garbage and extra preamble zeros are skipped by searching for the
/// `00 FF` start code. On success returns the frame and the number of bytes
/// consumed from the start of `buf` (including the postamble when present).
///
/// [`FrameError::Incomplete`] means more bytes are needed. Any other error
/// means the buffered bytes are unusable; the PN532 protocol recovers by
/// discarding them and sending a [`NACK`] to request a resend.
///
/// Never panics, whatever the input.
pub fn decode(buf: &[u8]) -> Result<(Frame, usize), FrameError> {
    let start = buf
        .windows(2)
        .position(|w| w == [0x00, 0xFF])
        .ok_or(FrameError::NoStartCode)?;
    let body = &buf[start + 2..];

    let (&len, &lcs) = match body {
        [len, lcs, ..] => (len, lcs),
        _ => return Err(FrameError::Incomplete),
    };
    match (len, lcs) {
        (0x00, 0xFF) => return Ok((Frame::Ack, finish(buf, start + 4))),
        (0xFF, 0x00) => return Ok((Frame::Nack, finish(buf, start + 4))),
        (0xFF, 0xFF) => return Err(FrameError::ExtendedFrame),
        _ => {}
    }
    if len.wrapping_add(lcs) != 0 {
        return Err(FrameError::BadLengthChecksum { len, lcs });
    }
    if len == 0 {
        return Err(FrameError::EmptyFrame);
    }

    // TFI + PD0..PDn + DCS
    let payload = body
        .get(2..2 + usize::from(len) + 1)
        .ok_or(FrameError::Incomplete)?;
    if payload.iter().fold(0u8, |acc, &b| acc.wrapping_add(b)) != 0 {
        return Err(FrameError::BadDataChecksum);
    }
    let tfi = payload[0];
    let data = &payload[1..payload.len() - 1];
    let end = finish(buf, start + 4 + payload.len());

    match tfi {
        TFI_PN532_TO_HOST => Ok((Frame::Response(data.to_vec()), end)),
        TFI_ERROR if data.is_empty() => Ok((Frame::Error, end)),
        other => Err(FrameError::UnexpectedTfi(other)),
    }
}

/// Extends `end` over the `00` postamble if it is present.
fn finish(buf: &[u8], end: usize) -> usize {
    if buf.get(end) == Some(&0x00) {
        end + 1
    } else {
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GetFirmwareVersion request.
    const GFV_REQ: [u8; 9] = [0x00, 0x00, 0xFF, 0x02, 0xFE, 0xD4, 0x02, 0x2A, 0x00];
    /// GetFirmwareVersion response from a PN532 v1.6 (IC 0x32, support 0x07).
    const GFV_RESP: [u8; 13] = [
        0x00, 0x00, 0xFF, 0x06, 0xFA, 0xD5, 0x03, 0x32, 0x01, 0x06, 0x07, 0xE8, 0x00,
    ];

    #[test]
    fn encodes_get_firmware_version() {
        assert_eq!(encode(&[0x02]).unwrap(), GFV_REQ);
    }

    #[test]
    fn encodes_sam_configuration() {
        // SAMConfiguration: normal mode, timeout 0x14, use IRQ.
        let expected = [
            0x00, 0x00, 0xFF, 0x05, 0xFB, 0xD4, 0x14, 0x01, 0x14, 0x01, 0x02, 0x00,
        ];
        assert_eq!(encode(&[0x14, 0x01, 0x14, 0x01]).unwrap(), expected);
    }

    #[test]
    fn encode_with_wakeup_prefixes_preamble() {
        let out = encode_with_wakeup(&[0x02]).unwrap();
        assert_eq!(&out[..WAKEUP.len()], &WAKEUP);
        assert_eq!(&out[WAKEUP.len()..], &GFV_REQ);
    }

    #[test]
    fn encode_rejects_oversize_data() {
        assert!(encode(&[0u8; MAX_DATA_LEN]).is_ok());
        assert_eq!(
            encode(&[0u8; MAX_DATA_LEN + 1]),
            Err(FrameError::DataTooLong(MAX_DATA_LEN + 1))
        );
    }

    #[test]
    fn decodes_get_firmware_version_response() {
        let (frame, used) = decode(&GFV_RESP).unwrap();
        assert_eq!(frame, Frame::Response(vec![0x03, 0x32, 0x01, 0x06, 0x07]));
        assert_eq!(used, GFV_RESP.len());
    }

    #[test]
    fn decodes_ack_nack_and_error() {
        assert_eq!(decode(&ACK), Ok((Frame::Ack, 6)));
        assert_eq!(decode(&NACK), Ok((Frame::Nack, 6)));
        assert_eq!(decode(&ERROR_FRAME), Ok((Frame::Error, 8)));
    }

    #[test]
    fn decodes_ack_followed_by_response() {
        let mut buf = ACK.to_vec();
        buf.extend_from_slice(&GFV_RESP);
        let (first, used) = decode(&buf).unwrap();
        assert_eq!(first, Frame::Ack);
        let (second, used2) = decode(&buf[used..]).unwrap();
        assert!(matches!(second, Frame::Response(_)));
        assert_eq!(used + used2, buf.len());
    }

    #[test]
    fn skips_leading_garbage_and_extra_preamble() {
        let mut buf = vec![0x55, 0x13, 0xFF, 0x00, 0x00, 0x00, 0x00];
        buf.extend_from_slice(&GFV_RESP);
        let (frame, used) = decode(&buf).unwrap();
        assert!(matches!(frame, Frame::Response(_)));
        assert_eq!(used, buf.len());
    }

    #[test]
    fn postamble_is_optional() {
        let short = &GFV_RESP[..GFV_RESP.len() - 1];
        assert_eq!(decode(short).unwrap().1, short.len());
        assert_eq!(decode(&ACK[..5]), Ok((Frame::Ack, 5)));
    }

    #[test]
    fn rejects_bad_length_checksum() {
        let mut buf = GFV_RESP;
        buf[4] = 0xFB;
        assert_eq!(
            decode(&buf),
            Err(FrameError::BadLengthChecksum {
                len: 0x06,
                lcs: 0xFB
            })
        );
    }

    #[test]
    fn rejects_bad_data_checksum() {
        let mut buf = GFV_RESP;
        buf[11] ^= 0x01;
        assert_eq!(decode(&buf), Err(FrameError::BadDataChecksum));
        let mut buf = GFV_RESP;
        buf[7] ^= 0x01; // corrupted data byte
        assert_eq!(decode(&buf), Err(FrameError::BadDataChecksum));
    }

    #[test]
    fn rejects_truncated_frames() {
        // Cut after the start code but before DCS.
        for cut in 3..GFV_RESP.len() - 1 {
            assert_eq!(
                decode(&GFV_RESP[..cut]),
                Err(FrameError::Incomplete),
                "{cut}"
            );
        }
    }

    #[test]
    fn rejects_len_beyond_buffer() {
        // LEN 0xFE claims far more data than is present.
        let buf = [0x00, 0x00, 0xFF, 0xFE, 0x02, 0xD5, 0x03, 0x28, 0x00];
        assert_eq!(decode(&buf), Err(FrameError::Incomplete));
    }

    #[test]
    fn rejects_extended_and_empty_frames() {
        let ext = [0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x01, 0x00, 0xFF, 0xD5, 0x2B];
        assert_eq!(decode(&ext), Err(FrameError::ExtendedFrame));
        assert_eq!(
            decode(&[0x00, 0x00, 0xFF, 0x00, 0x00, 0x00]),
            Err(FrameError::EmptyFrame)
        );
    }

    #[test]
    fn rejects_host_to_pn532_frame() {
        assert_eq!(decode(&GFV_REQ), Err(FrameError::UnexpectedTfi(0xD4)));
    }

    #[test]
    fn rejects_junk() {
        assert_eq!(decode(&[]), Err(FrameError::NoStartCode));
        assert_eq!(decode(&[0x00]), Err(FrameError::NoStartCode));
        assert_eq!(
            decode(&[0x12, 0x34, 0x56, 0xFF]),
            Err(FrameError::NoStartCode)
        );
        assert!(decode(&[0xFF; 64]).is_err());
        assert!(decode(&[0x00; 64]).is_err());
    }

    /// Tiny xorshift PRNG so the fuzz test needs no extra dependencies.
    struct XorShift(u64);
    impl XorShift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    #[test]
    fn never_panics_on_random_input() {
        let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
        // Bias towards bytes that matter to the framing.
        let interesting = [0x00, 0xFF, 0xD5, 0xD4, 0x7F, 0x01, 0xFE];
        for _ in 0..20_000 {
            let len = (rng.next() % 300) as usize;
            let buf: Vec<u8> = (0..len)
                .map(|_| {
                    let r = rng.next();
                    if r & 1 == 0 {
                        interesting[(r >> 8) as usize % interesting.len()]
                    } else {
                        (r >> 8) as u8
                    }
                })
                .collect();
            if let Ok((_, used)) = decode(&buf) {
                assert!(used <= buf.len());
            }
        }
    }

    #[test]
    fn never_panics_on_mutated_frames() {
        for pos in 0..GFV_RESP.len() {
            for byte in 0..=255u8 {
                let mut buf = GFV_RESP;
                buf[pos] = byte;
                for cut in 0..=buf.len() {
                    if let Ok((_, used)) = decode(&buf[..cut]) {
                        assert!(used <= cut);
                    }
                }
            }
        }
    }

    #[test]
    fn round_trips_every_length() {
        for n in 0..=MAX_DATA_LEN {
            let data: Vec<u8> = (0..n).map(|i| (i * 7) as u8).collect();
            let mut frame = encode(&data).unwrap();
            // Re-address as a PN532 -> host frame: TFI +1, DCS -1.
            frame[5] = TFI_PN532_TO_HOST;
            let dcs = frame.len() - 2;
            frame[dcs] = frame[dcs].wrapping_sub(1);
            assert_eq!(decode(&frame), Ok((Frame::Response(data), frame.len())));
        }
    }
}
