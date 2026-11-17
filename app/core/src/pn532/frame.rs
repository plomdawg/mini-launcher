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
    /// No `00 FF` start code (or prefix of one) found in the buffer.
    #[error("no frame start code found")]
    NoStartCode,
    /// More bytes are needed to finish (or find) the frame.
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

/// A [`decode`] failure: what went wrong, and how many leading bytes of the
/// buffer are safe to discard before decoding again.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{kind} (discard {skip} bytes)")]
pub struct DecodeError {
    /// What went wrong.
    pub kind: FrameError,
    /// Number of leading bytes that can never be part of a valid frame.
    /// Always at least 1 unless `kind` is [`FrameError::Incomplete`].
    pub skip: usize,
}

fn fail(kind: FrameError, skip: usize) -> DecodeError {
    DecodeError { kind, skip }
}

/// Decodes the first host-bound frame in `buf`.
///
/// Leading garbage and extra preamble zeros are skipped by searching for the
/// `00 FF` start code. On success returns the frame and the number of bytes
/// consumed from the start of `buf` (including the postamble when present).
///
/// Intended streaming use: append bytes from the port to a buffer, call
/// `decode`, and drain the consumed count on success, or `err.skip` on
/// error. On [`FrameError::Incomplete`] wait for more bytes (`skip` may
/// still drop leading garbage); on any other error decode again right away —
/// `skip` only covers the bad prefix, so a valid frame behind it survives.
///
/// A corrupted `LEN` whose `LCS` still matches makes the decoder wait for up
/// to 255 more bytes, so readers need a timeout (then drop the buffer and
/// send a [`NACK`]).
///
/// Never panics, whatever the input.
pub fn decode(buf: &[u8]) -> Result<(Frame, usize), DecodeError> {
    use FrameError::*;
    let Some(start) = buf.windows(2).position(|w| w == [0x00, 0xFF]) else {
        // A trailing 0x00 may be the first half of a start code.
        return match buf.last() {
            None => Err(fail(Incomplete, 0)),
            Some(0x00) => Err(fail(Incomplete, buf.len() - 1)),
            Some(_) => Err(fail(NoStartCode, buf.len())),
        };
    };
    let body = &buf[start + 2..];
    // A bad candidate's `00 FF` can't start any other frame; skip past it.
    let bad = |kind| Err(fail(kind, start + 2));

    let (&len, &lcs) = match body {
        [len, lcs, ..] => (len, lcs),
        _ => return Err(fail(Incomplete, start)),
    };
    match (len, lcs) {
        (0x00, 0xFF) => return Ok((Frame::Ack, finish(buf, start + 4))),
        (0xFF, 0x00) => return Ok((Frame::Nack, finish(buf, start + 4))),
        (0xFF, 0xFF) => return bad(ExtendedFrame),
        _ => {}
    }
    if len.wrapping_add(lcs) != 0 {
        return bad(BadLengthChecksum { len, lcs });
    }
    if len == 0 {
        return bad(EmptyFrame);
    }

    // TFI + PD0..PDn + DCS
    let Some(payload) = body.get(2..2 + usize::from(len) + 1) else {
        return Err(fail(Incomplete, start));
    };
    if payload.iter().fold(0u8, |acc, &b| acc.wrapping_add(b)) != 0 {
        return bad(BadDataChecksum);
    }
    let tfi = payload[0];
    let data = &payload[1..payload.len() - 1];
    let end = finish(buf, start + 4 + payload.len());

    match tfi {
        // May be empty (LEN = 1): callers must not assume a response code.
        TFI_PN532_TO_HOST => Ok((Frame::Response(data.to_vec()), end)),
        TFI_ERROR if data.is_empty() => Ok((Frame::Error, end)),
        // Well-formed but not for us (e.g. an echoed command): skip it whole.
        other => Err(fail(UnexpectedTfi(other), end)),
    }
}

/// Extends `end` over the `00` postamble if it is present. If the postamble
/// was lost this may eat the next frame's preamble byte, which is harmless:
/// the start-code search does not need it.
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
    fn encodes_fixtures() {
        assert_eq!(encode(&[0x02]).unwrap(), GFV_REQ);
        // SAMConfiguration: normal mode, timeout 0x14, use IRQ.
        let sam = [
            0x00, 0x00, 0xFF, 0x05, 0xFB, 0xD4, 0x14, 0x01, 0x14, 0x01, 0x02, 0x00,
        ];
        assert_eq!(encode(&[0x14, 0x01, 0x14, 0x01]).unwrap(), sam);
        assert_eq!(
            encode_with_wakeup(&[0x02]).unwrap(),
            [&WAKEUP[..], &GFV_REQ].concat()
        );
        assert!(encode(&[0u8; MAX_DATA_LEN]).is_ok());
        let too_long = encode(&[0u8; MAX_DATA_LEN + 1]);
        assert_eq!(too_long, Err(FrameError::DataTooLong(MAX_DATA_LEN + 1)));
    }

    /// `(kind, skip)` of a failed decode.
    fn err(buf: &[u8]) -> (FrameError, usize) {
        let e = decode(buf).unwrap_err();
        (e.kind, e.skip)
    }

    /// Accumulate/decode/drain loop as a serial reader would run it.
    fn stream<'a>(chunks: impl IntoIterator<Item = &'a [u8]>) -> Vec<Frame> {
        let (mut buf, mut out) = (Vec::new(), Vec::new());
        for chunk in chunks {
            buf.extend_from_slice(chunk);
            loop {
                match decode(&buf) {
                    Ok((frame, used)) => {
                        assert!(used > 0 && used <= buf.len());
                        out.push(frame);
                        buf.drain(..used);
                    }
                    Err(e) => {
                        assert!(e.skip <= buf.len());
                        buf.drain(..e.skip);
                        if e.kind == FrameError::Incomplete {
                            break;
                        }
                        assert!(e.skip > 0, "no progress on {:?}", e.kind);
                    }
                }
            }
        }
        out
    }

    #[test]
    fn decodes_fixtures() {
        let resp = Frame::Response(vec![0x03, 0x32, 0x01, 0x06, 0x07]);
        assert_eq!(decode(&GFV_RESP), Ok((resp, GFV_RESP.len())));
        assert_eq!(decode(&ACK), Ok((Frame::Ack, 6)));
        assert_eq!(decode(&NACK), Ok((Frame::Nack, 6)));
        assert_eq!(decode(&ERROR_FRAME), Ok((Frame::Error, 8)));
        assert_eq!(decode(&ACK[..5]), Ok((Frame::Ack, 5))); // postamble optional
    }

    /// Junk, a stray `00 FF` with a bad header, then ACK + response.
    fn noisy_stream() -> Vec<u8> {
        let junk = [0x55, 0x13, 0x00, 0xFF, 0x05, 0x00, 0xFF, 0x00, 0x00];
        [&junk[..], &ACK, &GFV_RESP].concat()
    }

    #[test]
    fn resyncs_past_bad_candidate() {
        let s = noisy_stream();
        let bad_lcs = FrameError::BadLengthChecksum {
            len: 0x05,
            lcs: 0x00,
        };
        assert_eq!(err(&s), (bad_lcs, 4));
        let expected = vec![Frame::Ack, Frame::Response(GFV_RESP[6..11].to_vec())];
        assert_eq!(stream([&s[..]]), expected);
    }

    #[test]
    fn survives_split_reads() {
        let s = noisy_stream();
        let expected = stream([&s[..]]);
        for i in 0..=s.len() {
            assert_eq!(stream([&s[..i], &s[i..]]), expected, "split at {i}");
        }
        assert_eq!(stream(s.chunks(1)), expected);
    }

    #[test]
    fn possible_start_code_prefix_is_incomplete() {
        use FrameError::Incomplete;
        assert_eq!(err(&[]), (Incomplete, 0));
        assert_eq!(err(&[0x00, 0x00]), (Incomplete, 1));
        assert_eq!(err(&[0x12, 0x00]), (Incomplete, 1));
        // A partial frame is kept; only the junk before it may be dropped.
        assert_eq!(err(&[0x12, 0x00, 0x00, 0xFF, 0x06]), (Incomplete, 2));
        for cut in 3..GFV_RESP.len() - 1 {
            assert_eq!(err(&GFV_RESP[..cut]), (Incomplete, 1), "cut {cut}");
        }
        // LEN 0xFE claims far more data than is present.
        let buf = [0x00, 0x00, 0xFF, 0xFE, 0x02, 0xD5, 0x03, 0x28, 0x00];
        assert_eq!(err(&buf), (Incomplete, 1));
    }

    #[test]
    fn rejects_bad_checksums() {
        let mut buf = GFV_RESP;
        buf[4] = 0xFB;
        let bad_lcs = FrameError::BadLengthChecksum {
            len: 0x06,
            lcs: 0xFB,
        };
        assert_eq!(err(&buf), (bad_lcs, 3));
        let mut buf = GFV_RESP;
        buf[11] ^= 0x01;
        assert_eq!(err(&buf), (FrameError::BadDataChecksum, 3));
        let mut buf = GFV_RESP;
        buf[7] ^= 0x01; // corrupted data byte
        assert_eq!(err(&buf), (FrameError::BadDataChecksum, 3));
    }

    #[test]
    fn rejects_unsupported_frames_and_junk() {
        let ext = [0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x01, 0x00, 0xFF, 0xD5, 0x2B];
        assert_eq!(err(&ext), (FrameError::ExtendedFrame, 3));
        let empty = [0x00, 0x00, 0xFF, 0x00, 0x00, 0x00];
        assert_eq!(err(&empty), (FrameError::EmptyFrame, 3));
        // A well-formed host -> PN532 frame (e.g. an echo) is skipped whole.
        assert_eq!(err(&GFV_REQ), (FrameError::UnexpectedTfi(0xD4), 9));
        assert_eq!(err(&[0x12, 0x34, 0xFF]), (FrameError::NoStartCode, 3));
        assert_eq!(err(&[0xFF; 64]), (FrameError::NoStartCode, 64));
        assert_eq!(err(&[0x00; 64]), (FrameError::Incomplete, 63));
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
            let split = (rng.next() as usize) % (buf.len() + 1);
            stream([&buf[..split], &buf[split..]]);
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
