//! PN532 command layer over any byte stream (`Read + Write`).
//!
//! Every command is: send the frame, expect an ACK, then read the response
//! frame and check that its code is the command code + 1. All bytes from the
//! device are untrusted: lengths are checked before indexing and the receive
//! buffer is bounded.

use std::io::{self, ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use thiserror::Error;

use super::frame::{self, Frame, FrameError, ACK, NACK, WAKEUP};

/// Default overall time limit for one command (ACK + response).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1000);
/// How many times a checksum-corrupted frame is NACKed before giving up.
pub const MAX_NACK_RETRIES: u32 = 3;
/// Upper bound for `MxRtyPassiveActivation`; `0xFF` would mean "retry forever".
pub const MAX_PASSIVE_RETRIES: u8 = 0xFE;
/// Longest NFCID1 allowed by ISO/IEC 14443-3 (triple-size UID).
pub const MAX_UID_LEN: usize = 10;
/// Upper bound for [`Pn532::set_timeout`], so deadline math cannot overflow.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(3600);
/// Receive buffer cap: one maximal normal frame plus slack for line noise.
const MAX_BUF: usize = 512;

const CMD_GET_FIRMWARE_VERSION: u8 = 0x02;
const CMD_SAM_CONFIGURATION: u8 = 0x14;
const CMD_RF_CONFIGURATION: u8 = 0x32;
const CMD_IN_DATA_EXCHANGE: u8 = 0x40;
const CMD_IN_LIST_PASSIVE_TARGET: u8 = 0x4A;
const CMD_IN_RELEASE: u8 = 0x52;
const NTAG_READ: u8 = 0x30;
/// Target number used for data exchange: we only ever list one target.
const TG: u8 = 0x01;

/// Errors from the PN532 driver.
#[derive(Debug, Error)]
pub enum Pn532Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("cannot encode command: {0}")]
    Encode(#[from] FrameError),
    #[error("timed out waiting for the PN532")]
    Timeout,
    #[error("PN532 did not acknowledge the command")]
    NoAck,
    #[error("frame checksum still bad after {MAX_NACK_RETRIES} NACK retries")]
    Checksum,
    #[error("PN532 sent an error frame")]
    ErrorFrame,
    #[error("unexpected response code {got:?} (expected {expected:#04x})")]
    UnexpectedResponse { expected: u8, got: Option<u8> },
    #[error("malformed response to command {cmd:#04x}: {len} bytes")]
    Malformed { cmd: u8, len: usize },
    #[error("PN532 reported status {0:#04x}")]
    Status(u8),
}

/// Result of `GetFirmwareVersion`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareVersion {
    pub ic: u8,
    pub ver: u8,
    pub rev: u8,
    pub support: u8,
}

/// An ISO/IEC 14443 type A target found by `InListPassiveTarget`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Logical target number assigned by the PN532.
    pub tg: u8,
    /// NFCID1 (4, 7 or 10 bytes).
    pub uid: Vec<u8>,
    /// SENS_RES, as sent by the PN532.
    pub atqa: [u8; 2],
    /// SEL_RES.
    pub sak: u8,
}

/// Byte stream to a PN532, plus a way to drop unread input.
pub trait Transport: Read + Write {
    /// Discards bytes received but not yet read, so a late frame from an
    /// earlier command cannot be taken as the reply to the next one.
    fn clear_input(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// PN532 driver over an HSU byte stream.
///
/// Reads may return data, `0` bytes, `TimedOut` or `WouldBlock`; the driver
/// keeps polling until the per-command timeout expires.
pub struct Pn532<T> {
    io: T,
    buf: Vec<u8>,
    timeout: Duration,
}

impl<T: Transport> Pn532<T> {
    pub fn new(io: T) -> Self {
        Self {
            io,
            buf: Vec::with_capacity(MAX_BUF),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Sets the overall per-command timeout (capped at [`MAX_TIMEOUT`]).
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout.min(MAX_TIMEOUT);
    }

    /// Returns the underlying transport.
    pub fn into_inner(self) -> T {
        self.io
    }

    /// Sends the HSU wakeup sequence. Call right before the first command
    /// (normally [`Self::sam_configuration`]).
    pub fn wakeup(&mut self) -> Result<(), Pn532Error> {
        self.io.write_all(&WAKEUP)?;
        self.io.flush()?;
        Ok(())
    }

    pub fn get_firmware_version(&mut self) -> Result<FirmwareVersion, Pn532Error> {
        match self.command(&[CMD_GET_FIRMWARE_VERSION])?[..] {
            [ic, ver, rev, support] => Ok(FirmwareVersion {
                ic,
                ver,
                rev,
                support,
            }),
            ref d => Err(malformed(CMD_GET_FIRMWARE_VERSION, d)),
        }
    }

    /// SAMConfiguration: normal mode (no SAM), timeout byte 0x14 (1 s,
    /// only used in virtual-card mode), IRQ pin enabled.
    pub fn sam_configuration(&mut self) -> Result<(), Pn532Error> {
        self.command(&[CMD_SAM_CONFIGURATION, 0x01, 0x14, 0x01])?;
        Ok(())
    }

    /// RFConfiguration item `MaxRetries`: how many times `InListPassiveTarget`
    /// retries activation. Clamped to [`MAX_PASSIVE_RETRIES`] so a poll can
    /// never block forever.
    ///
    /// Activation retries run inside one `InListPassiveTarget`, so a large
    /// value (e.g. 0xFE) can outlast the command timeout; the caller then
    /// gets [`Pn532Error::Timeout`] (and the command is aborted), not "no
    /// tag". Pollers should use a small count, or a command timeout longer
    /// than the retry budget.
    pub fn rf_configuration_max_retries(&mut self, passive: u8) -> Result<(), Pn532Error> {
        let passive = passive.min(MAX_PASSIVE_RETRIES);
        // CfgItem 0x05: MxRtyATR, MxRtyPSL, MxRtyPassiveActivation.
        self.command(&[CMD_RF_CONFIGURATION, 0x05, 0x02, 0x01, passive])?;
        Ok(())
    }

    /// Looks for one ISO/IEC 14443 type A target at 106 kbps.
    pub fn in_list_passive_target_106a(&mut self) -> Result<Option<Target>, Pn532Error> {
        const CMD: u8 = CMD_IN_LIST_PASSIVE_TARGET;
        let d = self.command(&[CMD, 0x01, 0x00])?;
        // NbTg, then Tg, SENS_RES (2), SEL_RES, NFCIDLength, NFCID1, [ATS].
        match d[..] {
            [0, ..] => Ok(None),
            [_, tg, a0, a1, sak, uid_len, ref rest @ ..] => {
                let uid_len = usize::from(uid_len);
                if uid_len == 0 || uid_len > MAX_UID_LEN || uid_len > rest.len() {
                    return Err(malformed(CMD, &d));
                }
                Ok(Some(Target {
                    tg,
                    uid: rest[..uid_len].to_vec(),
                    atqa: [a0, a1],
                    sak,
                }))
            }
            _ => Err(malformed(CMD, &d)),
        }
    }

    /// NTAG `READ` of 4 pages (16 bytes) starting at `page`, on target 1.
    ///
    /// Reads wrap around past the last page (0x2C on NTAG213) back to page 0.
    /// A start page beyond the last page makes the tag NAK, which surfaces as
    /// [`Pn532Error::Status`] or [`Pn532Error::Malformed`].
    pub fn ntag_read(&mut self, page: u8) -> Result<[u8; 16], Pn532Error> {
        let d = self.command(&[CMD_IN_DATA_EXCHANGE, TG, NTAG_READ, page])?;
        let (&status, data) = d
            .split_first()
            .ok_or_else(|| malformed(CMD_IN_DATA_EXCHANGE, &d))?;
        check_status(status)?;
        data.try_into()
            .map_err(|_| malformed(CMD_IN_DATA_EXCHANGE, &d))
    }

    /// Releases all targets.
    pub fn in_release(&mut self) -> Result<(), Pn532Error> {
        match self.command(&[CMD_IN_RELEASE, 0x00])?[..] {
            [status] => check_status(status),
            ref d => Err(malformed(CMD_IN_RELEASE, d)),
        }
    }

    /// Sends `data` (command code + params), waits for ACK and the matching
    /// response, and returns the response data after the response code.
    fn command(&mut self, data: &[u8]) -> Result<Vec<u8>, Pn532Error> {
        let expected = data[0].wrapping_add(1);
        let request = frame::encode(data)?;
        self.buf.clear();
        self.io.clear_input()?;
        self.io.write_all(&request)?;
        self.io.flush()?;
        let deadline = Instant::now() + self.timeout;

        // Nothing but our ACK can legitimately come first: anything else is a
        // stale frame from an earlier command, so drop it and keep waiting.
        // Checksum errors here are not NACKed (undefined before the ACK).
        loop {
            match self.read_frame(deadline, false) {
                Ok(Frame::Ack) => break,
                Ok(_) => {}
                Err(Pn532Error::Timeout) => {
                    self.abort();
                    return Err(Pn532Error::NoAck);
                }
                Err(e) => return Err(e),
            }
        }
        let resp = loop {
            match self.read_frame(deadline, true) {
                Ok(Frame::Response(resp)) => break resp,
                Ok(Frame::Ack) => {} // stray duplicate
                Ok(Frame::Error) => return Err(Pn532Error::ErrorFrame),
                Ok(Frame::Nack) => {
                    return Err(Pn532Error::UnexpectedResponse {
                        expected,
                        got: None,
                    })
                }
                Err(Pn532Error::Timeout) => {
                    self.abort();
                    return Err(Pn532Error::Timeout);
                }
                Err(e) => return Err(e),
            }
        };
        match resp.split_first() {
            Some((&code, rest)) if code == expected => Ok(rest.to_vec()),
            got => Err(Pn532Error::UnexpectedResponse {
                expected,
                got: got.map(|(&c, _)| c),
            }),
        }
    }

    /// Best effort: an ACK from the host aborts the command in progress.
    fn abort(&mut self) {
        let _ = self.io.write_all(&ACK).and_then(|()| self.io.flush());
    }

    /// Reads the next frame. Checksum failures are NACKed (bounded retries)
    /// if `nack` is set, otherwise just skipped.
    fn read_frame(&mut self, deadline: Instant, nack: bool) -> Result<Frame, Pn532Error> {
        let mut nacks = 0;
        loop {
            match frame::decode(&self.buf) {
                Ok((frame, used)) => {
                    self.buf.drain(..used);
                    return Ok(frame);
                }
                Err(e) => {
                    self.buf.drain(..e.skip);
                    match e.kind {
                        FrameError::Incomplete => self.fill(deadline)?,
                        FrameError::BadDataChecksum | FrameError::BadLengthChecksum { .. }
                            if nack =>
                        {
                            nacks += 1;
                            if nacks > MAX_NACK_RETRIES {
                                return Err(Pn532Error::Checksum);
                            }
                            self.io.write_all(&NACK)?;
                            self.io.flush()?;
                        }
                        _ => {} // skip already applied; decode again
                    }
                }
            }
        }
    }

    /// Reads more bytes into the buffer, or fails once `deadline` passes.
    fn fill(&mut self, deadline: Instant) -> Result<(), Pn532Error> {
        if Instant::now() >= deadline {
            self.buf.clear();
            return Err(Pn532Error::Timeout);
        }
        let mut chunk = [0u8; 64];
        match self.io.read(&mut chunk) {
            Ok(0) => std::thread::sleep(Duration::from_millis(1)),
            Ok(n) => {
                if self.buf.len() + n > MAX_BUF {
                    // Only reachable with a runaway stream; resync from scratch.
                    self.buf.clear();
                }
                self.buf.extend_from_slice(&chunk[..n]);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }
}

fn malformed(cmd: u8, data: &[u8]) -> Pn532Error {
    Pn532Error::Malformed {
        cmd,
        len: data.len(),
    }
}

/// The low 6 bits of a PN532 status byte are the error code.
fn check_status(status: u8) -> Result<(), Pn532Error> {
    match status & 0x3F {
        0 => Ok(()),
        _ => Err(Pn532Error::Status(status)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    // Fixtures; checksums hand-verified (LEN + LCS == 0, TFI + PD + DCS == 0).
    const GFV_REQ: &[u8] = &[0x00, 0x00, 0xFF, 0x02, 0xFE, 0xD4, 0x02, 0x2A, 0x00];
    const GFV_RESP: &[u8] = &[
        0x00, 0x00, 0xFF, 0x06, 0xFA, 0xD5, 0x03, 0x32, 0x01, 0x06, 0x07, 0xE8, 0x00,
    ];
    const SAM_REQ: &[u8] = &[
        0x00, 0x00, 0xFF, 0x05, 0xFB, 0xD4, 0x14, 0x01, 0x14, 0x01, 0x02, 0x00,
    ];
    const SAM_RESP: &[u8] = &[0x00, 0x00, 0xFF, 0x02, 0xFE, 0xD5, 0x15, 0x16, 0x00];
    const RF_REQ: &[u8] = &[
        0x00, 0x00, 0xFF, 0x06, 0xFA, 0xD4, 0x32, 0x05, 0x02, 0x01, 0xFE, 0xF4, 0x00,
    ];
    const RF_RESP: &[u8] = &[0x00, 0x00, 0xFF, 0x02, 0xFE, 0xD5, 0x33, 0xF8, 0x00];
    const LIST_REQ: &[u8] = &[
        0x00, 0x00, 0xFF, 0x04, 0xFC, 0xD4, 0x4A, 0x01, 0x00, 0xE1, 0x00,
    ];
    const LIST_NONE: &[u8] = &[0x00, 0x00, 0xFF, 0x03, 0xFD, 0xD5, 0x4B, 0x00, 0xE0, 0x00];
    /// One target: Tg 1, SENS_RES 00 44, SEL_RES 00, 7-byte UID 04 A1 B2 C3 D4 E5 80.
    const LIST_NTAG: &[u8] = &[
        0x00, 0x00, 0xFF, 0x0F, 0xF1, 0xD5, 0x4B, 0x01, 0x01, 0x00, 0x44, 0x00, 0x07, 0x04, 0xA1,
        0xB2, 0xC3, 0xD4, 0xE5, 0x80, 0x40, 0x00,
    ];
    /// Same, but NFCIDLength claims 8 bytes when only 7 follow.
    const LIST_BAD_UID: &[u8] = &[
        0x00, 0x00, 0xFF, 0x0F, 0xF1, 0xD5, 0x4B, 0x01, 0x01, 0x00, 0x44, 0x00, 0x08, 0x04, 0xA1,
        0xB2, 0xC3, 0xD4, 0xE5, 0x80, 0x3F, 0x00,
    ];
    const READ_REQ: &[u8] = &[
        0x00, 0x00, 0xFF, 0x05, 0xFB, 0xD4, 0x40, 0x01, 0x30, 0x04, 0xB7, 0x00,
    ];
    /// Status 00 + bytes 10..1F.
    const READ_RESP: &[u8] = &[
        0x00, 0x00, 0xFF, 0x13, 0xED, 0xD5, 0x41, 0x00, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16,
        0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x72, 0x00,
    ];
    /// Status 00 + only 15 bytes.
    const READ_SHORT: &[u8] = &[
        0x00, 0x00, 0xFF, 0x12, 0xEE, 0xD5, 0x41, 0x00, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16,
        0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x91, 0x00,
    ];
    /// Status 01 (timeout), no data.
    const READ_FAIL: &[u8] = &[0x00, 0x00, 0xFF, 0x03, 0xFD, 0xD5, 0x41, 0x01, 0xE9, 0x00];
    const REL_REQ: &[u8] = &[0x00, 0x00, 0xFF, 0x03, 0xFD, 0xD4, 0x52, 0x00, 0xDA, 0x00];
    const REL_RESP: &[u8] = &[0x00, 0x00, 0xFF, 0x03, 0xFD, 0xD5, 0x53, 0x00, 0xD8, 0x00];
    /// Response frame with no response code (LEN 1).
    const EMPTY_RESP: &[u8] = &[0x00, 0x00, 0xFF, 0x01, 0xFF, 0xD5, 0x2B, 0x00];

    /// Replays canned read chunks and records everything written. An empty
    /// chunk is a 0-byte read; once the script runs out, reads time out.
    struct Mock {
        reads: VecDeque<Vec<u8>>,
        written: Vec<u8>,
    }

    impl Read for Mock {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let Some(mut chunk) = self.reads.pop_front() else {
                return Err(ErrorKind::TimedOut.into());
            };
            let n = chunk.len().min(buf.len());
            buf[..n].copy_from_slice(&chunk[..n]);
            if n < chunk.len() {
                self.reads.push_front(chunk.split_off(n));
            }
            Ok(n)
        }
    }

    impl Transport for Mock {}

    impl Write for Mock {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Driver fed `chunks` as-is, and one fed the same bytes one at a time.
    fn drivers(chunks: &[&[u8]]) -> [Pn532<Mock>; 2] {
        let one_byte = chunks.concat().into_iter().map(|b| vec![b]).collect();
        let whole = chunks.iter().map(|c| c.to_vec()).collect();
        [whole, one_byte].map(|reads| {
            let mut p = Pn532::new(Mock {
                reads,
                written: Vec::new(),
            });
            p.set_timeout(Duration::from_millis(30));
            p
        })
    }

    fn written(p: Pn532<Mock>) -> Vec<u8> {
        p.into_inner().written
    }

    #[test]
    fn firmware_version() {
        for mut p in drivers(&[&ACK, &[], GFV_RESP]) {
            let fw = p.get_firmware_version().unwrap();
            assert_eq!((fw.ic, fw.ver, fw.rev, fw.support), (0x32, 1, 6, 7));
            assert_eq!(written(p), GFV_REQ);
        }
    }

    #[test]
    fn setup_commands() {
        for mut p in drivers(&[&ACK, SAM_RESP, &ACK, RF_RESP, &ACK, REL_RESP]) {
            p.wakeup().unwrap();
            p.sam_configuration().unwrap();
            p.rf_configuration_max_retries(0xFF).unwrap(); // clamped to 0xFE
            p.in_release().unwrap();
            assert_eq!(written(p), [&WAKEUP, SAM_REQ, RF_REQ, REL_REQ].concat());
        }
    }

    #[test]
    fn no_tag_present() {
        for mut p in drivers(&[&ACK, LIST_NONE]) {
            assert_eq!(p.in_list_passive_target_106a().unwrap(), None);
            assert_eq!(written(p), LIST_REQ);
        }
    }

    #[test]
    fn ntag_present_and_read() {
        for mut p in drivers(&[&ACK, LIST_NTAG, &ACK, READ_RESP]) {
            let t = p.in_list_passive_target_106a().unwrap().unwrap();
            assert_eq!(t.tg, 1);
            assert_eq!(t.uid, [0x04, 0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0x80]);
            assert_eq!((t.atqa, t.sak), ([0x00, 0x44], 0x00));
            let page: Vec<u8> = (0x10..0x20).collect();
            assert_eq!(p.ntag_read(4).unwrap()[..], page[..]);
            assert_eq!(written(p), [LIST_REQ, READ_REQ].concat());
        }
    }

    #[test]
    fn rejects_bad_lengths_and_status() {
        let uid = drivers(&[&ACK, LIST_BAD_UID]).map(|mut p| p.in_list_passive_target_106a());
        let short = drivers(&[&ACK, READ_SHORT]).map(|mut p| p.ntag_read(4));
        let fail = drivers(&[&ACK, READ_FAIL]).map(|mut p| p.ntag_read(4));
        for i in 0..2 {
            assert!(matches!(
                uid[i],
                Err(Pn532Error::Malformed { cmd: 0x4A, .. })
            ));
            assert!(matches!(
                short[i],
                Err(Pn532Error::Malformed { cmd: 0x40, len: 16 })
            ));
            assert!(matches!(fail[i], Err(Pn532Error::Status(0x01))));
        }
    }

    #[test]
    fn stale_and_stray_frames_do_not_desync() {
        let mut bad = GFV_RESP.to_vec();
        bad[7] ^= 0x01;
        // Stale SAM reply + corrupted frame + error frame before the ACK,
        // a duplicate ACK before the response, then normal traffic.
        let chunks: &[&[u8]] = &[
            SAM_RESP,
            &bad,
            &frame::ERROR_FRAME,
            &ACK,
            GFV_RESP,
            &ACK,
            &ACK,
            GFV_RESP,
            &ACK,
            REL_RESP,
        ];
        for mut p in drivers(chunks) {
            assert_eq!(p.get_firmware_version().unwrap().ic, 0x32);
            assert_eq!(p.get_firmware_version().unwrap().ic, 0x32);
            p.in_release().unwrap();
            // No NACK was sent for the pre-ACK corruption.
            assert_eq!(written(p), [GFV_REQ, GFV_REQ, REL_REQ].concat());
        }
    }

    #[test]
    fn huge_timeout_does_not_overflow() {
        let [mut p, _] = drivers(&[&ACK, GFV_RESP]);
        p.set_timeout(Duration::MAX);
        assert!(p.get_firmware_version().is_ok());
    }

    #[test]
    fn missing_ack() {
        for chunks in [&[GFV_RESP][..], &[]] {
            for mut p in drivers(chunks) {
                assert!(matches!(p.get_firmware_version(), Err(Pn532Error::NoAck)));
                assert_eq!(written(p), [GFV_REQ, &ACK].concat()); // abort sent
            }
        }
    }

    #[test]
    fn wrong_or_empty_response_code() {
        for (resp, got) in [(SAM_RESP, Some(0x15)), (EMPTY_RESP, None)] {
            for mut p in drivers(&[&ACK, resp]) {
                let err = p.get_firmware_version().unwrap_err();
                assert!(
                    matches!(err, Pn532Error::UnexpectedResponse { expected: 0x03, got: g } if g == got),
                    "{err:?}"
                );
            }
        }
    }

    #[test]
    fn short_response() {
        // GetFirmwareVersion answered with only IC + Ver; InRelease with no status.
        let gfv = &[
            0x00, 0x00, 0xFF, 0x04, 0xFC, 0xD5, 0x03, 0x32, 0x01, 0xF5, 0x00,
        ];
        let rel = &[0x00, 0x00, 0xFF, 0x02, 0xFE, 0xD5, 0x53, 0xD8, 0x00];
        for mut p in drivers(&[&ACK, gfv, &ACK, rel]) {
            let err = p.get_firmware_version().unwrap_err();
            assert!(matches!(err, Pn532Error::Malformed { cmd: 0x02, len: 2 }));
            assert!(matches!(
                p.in_release(),
                Err(Pn532Error::Malformed { len: 0, .. })
            ));
        }
    }

    #[test]
    fn nacks_corrupted_frame_then_retries() {
        let mut bad = GFV_RESP.to_vec();
        bad[7] ^= 0x01;
        for mut p in drivers(&[&ACK, &bad, GFV_RESP]) {
            assert_eq!(p.get_firmware_version().unwrap().ic, 0x32);
            assert_eq!(written(p), [GFV_REQ, &NACK].concat());
        }
        let many: Vec<&[u8]> = [&ACK[..]].into_iter().chain([&bad[..]; 4]).collect();
        for mut p in drivers(&many) {
            assert!(matches!(
                p.get_firmware_version(),
                Err(Pn532Error::Checksum)
            ));
            let nacks = [&NACK[..]; MAX_NACK_RETRIES as usize].concat();
            assert_eq!(written(p), [GFV_REQ, &nacks].concat());
        }
    }

    #[test]
    fn response_timeout_aborts_command() {
        for mut p in drivers(&[&ACK, &GFV_RESP[..7]]) {
            assert!(matches!(p.get_firmware_version(), Err(Pn532Error::Timeout)));
            assert_eq!(written(p), [GFV_REQ, &ACK].concat());
        }
    }

    #[test]
    fn error_frame() {
        for mut p in drivers(&[&ACK, &frame::ERROR_FRAME]) {
            assert!(matches!(
                p.get_firmware_version(),
                Err(Pn532Error::ErrorFrame)
            ));
        }
    }
}
