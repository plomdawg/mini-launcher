use super::*;

/// Pads a TLV area to NTAG213 user memory size (144 bytes), as a full read returns.
fn tag(bytes: &[u8]) -> Vec<u8> {
    let mut mem = bytes.to_vec();
    mem.resize(144, 0x00);
    mem
}

/// NFC Tools "Text" record, language `en`, text `steam://440`.
const NFC_TOOLS_TEXT: &[u8] = &[
    0x03, 0x12, 0xD1, 0x01, 0x0E, 0x54, 0x02, b'e', b'n', b's', b't', b'e', b'a', b'm', b':', b'/',
    b'/', b'4', b'4', b'0', 0xFE,
];

/// NFC Tools "Custom URI" record, prefix code 0x00, URI `steam://440`.
const NFC_TOOLS_URI: &[u8] = &[
    0x03, 0x10, 0xD1, 0x01, 0x0C, 0x55, 0x00, b's', b't', b'e', b'a', b'm', b':', b'/', b'/', b'4',
    b'4', b'0', 0xFE,
];

/// NFC Tools "URL" record, prefix code 0x04 (`https://`), URI `example.com`.
const NFC_TOOLS_URL: &[u8] = &[
    0x03, 0x10, 0xD1, 0x01, 0x0C, 0x55, 0x04, b'e', b'x', b'a', b'm', b'p', b'l', b'e', b'.', b'c',
    b'o', b'm', 0xFE,
];

fn text(s: &str) -> Result<NdefRecord, NdefError> {
    Ok(NdefRecord::Text(s.to_string()))
}

fn uri(s: &str) -> Result<NdefRecord, NdefError> {
    Ok(NdefRecord::Uri(s.to_string()))
}

/// Early-stop property: wherever a reader may stop (`tlv_area_complete` on
/// a prefix), the prefix must decode exactly like the full memory, so
/// stopping early never changes the result or skips a check. `step` is the
/// prefix stride (16 = one 4-page READ).
fn assert_early_stop_agrees(full: &[u8], step: usize) {
    let want = parse_user_memory(full);
    for n in (0..=full.len()).step_by(step) {
        let prefix = &full[..n];
        if tlv_area_complete(prefix) {
            assert_eq!(parse_user_memory(prefix), want, "stop at {n}: {full:02X?}");
        }
    }
}

/// Wraps an NDEF message in a 1-byte-length NDEF TLV + terminator.
fn wrap(msg: &[u8]) -> Vec<u8> {
    let mut out = vec![0x03, u8::try_from(msg.len()).unwrap()];
    out.extend_from_slice(msg);
    out.push(0xFE);
    out
}

#[test]
fn phone_written_fixtures() {
    let cases: &[(&[u8], Result<NdefRecord, NdefError>)] = &[
        (NFC_TOOLS_TEXT, text("steam://440")),
        (NFC_TOOLS_URI, uri("steam://440")),
        (NFC_TOOLS_URL, uri("https://example.com")),
    ];
    for (bytes, want) in cases {
        assert_eq!(&parse_user_memory(&tag(bytes)), want);
        // Unpadded (exact-length) reads parse the same.
        assert_eq!(&parse_user_memory(bytes), want);
    }
    assert_eq!(
        parse_user_memory(NFC_TOOLS_TEXT).unwrap().as_str(),
        "steam://440"
    );
}

#[test]
fn skips_null_lock_and_memory_control_tlvs() {
    let mut mem = vec![
        0x00, 0x00, 0x01, 0x03, 0xA0, 0x0C, 0x34, 0x02, 0x03, 0x00, 0x00, 0x00,
    ];
    mem.extend_from_slice(NFC_TOOLS_URI);
    assert_eq!(parse_user_memory(&tag(&mem)), uri("steam://440"));
}

#[test]
fn three_byte_tlv_length() {
    let mut mem = vec![0x03, 0xFF, 0x00, 0x10];
    mem.extend_from_slice(&NFC_TOOLS_URI[2..]);
    assert_eq!(parse_user_memory(&tag(&mem)), uri("steam://440"));
}

#[test]
fn normal_record_header_and_id_field() {
    // MB|ME|IL, TNF 1, 4-byte payload length, 2-byte ID.
    let msg = [
        0xC9, 0x01, 0x00, 0x00, 0x00, 0x05, 0x02, 0x54, b'i', b'd', 0x00, b'i', b'd', b':', b'x',
    ];
    assert_eq!(parse_user_memory(&wrap(&msg)), text("id:x"));
}

#[test]
fn every_uri_prefix_expands() {
    for code in 0u8..=0x23 {
        let msg = [0xD1, 0x01, 0x02, 0x55, code, b'x'];
        let got = parse_user_memory(&wrap(&msg)).unwrap();
        assert_eq!(
            got.as_str(),
            format!("{}x", URI_PREFIXES[usize::from(code)])
        );
    }
    assert_eq!(URI_PREFIXES[0x23], "urn:nfc:");
}

#[test]
fn first_usable_record_wins() {
    let msg = [
        0x91, 0x01, 0x02, 0x55, 0x24, b'x', // reserved URI prefix: skipped
        0x11, 0x01, 0x03, 0x54, 0x80, b'a', b'b', // UTF-16 text: skipped
        0x11, 0x01, 0x02, 0x54, 0x00, 0xFF, // invalid UTF-8 text: skipped
        0x12, 0x01, 0x00, b'X', // media record: ignored
        0x11, 0x01, 0x03, 0x54, 0x00, b'o', b'k', // usable
        0x51, 0x01, 0x02, 0x54, 0x00, b'n', // ME, never reached
    ];
    assert_eq!(parse_user_memory(&wrap(&msg)), text("ok"));
}

#[test]
fn unusable_records() {
    let cases: &[&[u8]] = &[
        &[0xD1, 0x01, 0x02, 0x55, 0x24, b'x'], // reserved URI prefix
        &[0xD1, 0x01, 0x02, 0x55, 0xFF, b'x'], // reserved URI prefix
        &[0xD1, 0x01, 0x03, 0x54, 0x80, b'a', 0x00], // UTF-16 text
        &[0xD1, 0x01, 0x00, 0x55],             // URI without prefix byte
        &[0xD1, 0x01, 0x02, 0x55, 0x00, 0xC3], // invalid UTF-8 URI
        &[0xD1, 0x01, 0x02, 0x54, 0x05, b'e'], // language code past payload
        &[0xD1, 0x01, 0x00, 0x54],             // empty text payload
        &[0xF1, 0x01, 0x01, 0x54, 0x00],       // chunked
        &[0xD0, 0x00, 0x00],                   // empty record (erased tag)
        &[],                                   // empty message
    ];
    for msg in cases {
        assert_eq!(
            parse_user_memory(&wrap(msg)),
            Err(NdefError::NoUsableRecord),
            "{msg:02X?}"
        );
    }
}

#[test]
fn hostile_inputs_are_errors() {
    let cases: &[(&[u8], NdefError)] = &[
        (&[], NdefError::NoNdefMessage),
        (&[0x00; 144], NdefError::NoNdefMessage),
        (&[0xFE, 0x03, 0x01, 0x00], NdefError::NoNdefMessage),
        (&[0xFF; 144], NdefError::UnknownTlv(0xFF)),
        (&[0x03], NdefError::TruncatedTlv),
        (&[0x03, 0x20, 0xD1], NdefError::TruncatedTlv),
        (&[0x03, 0xFF, 0x00], NdefError::TruncatedTlv),
        (&[0x03, 0xFF, 0xFF, 0xFF, 0xD1], NdefError::TruncatedTlv),
        (&[0x01, 0x10, 0x00], NdefError::TruncatedTlv),
        (&[0x03, 0x01, 0xD1], NdefError::TruncatedRecord),
        (&[0x03, 0x02, 0xD1, 0x01], NdefError::TruncatedRecord),
        (
            &[0x03, 0x04, 0xD1, 0x01, 0x09, 0x54],
            NdefError::TruncatedRecord,
        ),
        (
            &[0x03, 0x05, 0xC1, 0x01, 0xFF, 0xFF, 0xFF],
            NdefError::TruncatedRecord,
        ),
        (
            &[0x03, 0x07, 0xC1, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0x54],
            NdefError::TruncatedRecord,
        ),
        (&[0x03, 0x03, 0xD9, 0x01, 0x01], NdefError::TruncatedRecord),
    ];
    for (mem, want) in cases {
        assert_eq!(parse_user_memory(mem), Err(*want), "{mem:02X?}");
    }
}

#[test]
fn tlv_area_completeness() {
    let complete: &[&[u8]] = &[
        NFC_TOOLS_TEXT,
        // Whole message TLV, no terminator yet.
        &NFC_TOOLS_TEXT[..NFC_TOOLS_TEXT.len() - 1],
        &[0x00, 0xFE],
        &[0x01, 0x03, 0xA0, 0x0C, 0x34, 0xFE],
        &[0xFF, 0x00],
        // Whole but unusable message: more pages will not fix it.
        &[0x03, 0x01, 0xD1],
    ];
    for mem in complete {
        assert!(tlv_area_complete(mem), "{mem:02X?}");
    }
    let incomplete: &[&[u8]] = &[
        &[],
        &[0x00; 48],
        &[0x01, 0x03, 0xA0, 0x0C, 0x34],
        &[0x03],
        &[0x03, 0xFF, 0x00],
        &NFC_TOOLS_TEXT[..NFC_TOOLS_TEXT.len() - 2],
    ];
    for mem in incomplete {
        assert!(!tlv_area_complete(mem), "{mem:02X?}");
    }
    // A prefix of a complete area is complete exactly when it parses the same.
    assert_early_stop_agrees(&tag(NFC_TOOLS_URI), 1);
}

#[test]
fn every_truncation_and_byte_mutation_is_handled() {
    for fixture in [NFC_TOOLS_TEXT, NFC_TOOLS_URI, NFC_TOOLS_URL] {
        for len in 0..fixture.len() {
            let _ = parse_user_memory(&fixture[..len]);
            let _ = tlv_area_complete(&fixture[..len]);
        }
        let mut mem = fixture.to_vec();
        for i in 0..mem.len() {
            let original = mem[i];
            for b in 0..=u8::MAX {
                mem[i] = b;
                let _ = parse_user_memory(&mem);
                let _ = tlv_area_complete(&mem);
                // Every stop point, as read from a padded NTAG213.
                assert_early_stop_agrees(&tag(&mem), 1);
            }
            mem[i] = original;
        }
    }
}

#[test]
fn random_buffers_never_panic() {
    // xorshift64: deterministic, no dependency.
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..50_000 {
        let len = (next() % 160) as usize;
        let mut mem: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        // Bias towards structure so the record parser gets exercised too.
        if len > 2 && next() % 2 == 0 {
            mem[0] = 0x03;
            mem[1] = (len - 2) as u8;
        }
        let _ = parse_user_memory(&mem);
        let _ = tlv_area_complete(&mem);
        // The page-aligned stop points the reader actually uses.
        assert_early_stop_agrees(&mem, 16);
    }
}

mod encode {
    use super::*;
    use crate::payload::{Payload, Slug};

    fn round_trip(p: &Payload) {
        let mem = encode_payload(p).expect("encodable");
        assert!(mem.len() <= 80, "{p:?}: {} bytes", mem.len());
        assert!(tlv_area_complete(&mem));
        let record = parse_user_memory(&tag(&mem)).unwrap();
        assert_eq!(&Payload::parse(record.as_str()), p, "{mem:02X?}");
        // Every strict prefix is incomplete, so a torn read never parses.
        for n in 0..mem.len() - 1 {
            assert!(parse_user_memory(&mem[..n]).is_err(), "prefix {n}");
        }
    }

    #[test]
    fn fixtures() {
        // Byte-identical to what NFC Tools writes for "Custom URI".
        let steam = encode_payload(&Payload::Steam(440)).unwrap();
        assert_eq!(steam, NFC_TOOLS_URI);
        let id = encode_payload(&Payload::Id(Slug::new("my-game").unwrap())).unwrap();
        let want = [
            &[0x03, 0x11, 0xD1, 0x01, 0x0D, b'T', 0x02, b'e', b'n'][..],
            b"id:my-game",
            &[0xFE],
        ]
        .concat();
        assert_eq!(id, want);
        assert_eq!(parse_user_memory(&id), text("id:my-game"));
    }

    #[test]
    fn round_trips_appids_and_slugs() {
        for appid in [1, 9, 10, 440, 1_234_567, u32::MAX - 1, u32::MAX] {
            round_trip(&Payload::Steam(appid));
        }
        for slug in ["0", "a", "a-", "9-9", "half-life-2", &"z".repeat(64)] {
            round_trip(&Payload::Id(Slug::new(slug).unwrap()));
        }
        // Pseudo-random appids and valid slugs (xorshift, fixed seed).
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        const FIRST: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        for _ in 0..5_000 {
            round_trip(&Payload::Steam((next() as u32).max(1)));
            let len = 1 + (next() % 64) as usize;
            let slug: String = (0..len)
                .map(|i| {
                    let r = next() as usize;
                    match (i, r % 37) {
                        (0, _) => FIRST[r % FIRST.len()] as char,
                        (_, 36) => '-',
                        (_, k) => FIRST[k] as char,
                    }
                })
                .collect();
            round_trip(&Payload::Id(Slug::new(&slug).unwrap()));
        }
    }

    #[test]
    fn unusable_is_not_encodable() {
        assert_eq!(encode_payload(&Payload::Unusable), None);
    }

    #[test]
    fn zero_length_tlv_is_nothing_usable() {
        // The tag writer's intermediate state (TLV length 0, record bytes
        // after it) must read as an empty tag, never as a payload.
        for p in [
            Payload::Steam(440),
            Payload::Id(Slug::new("my-game").unwrap()),
        ] {
            let mut mem = encode_payload(&p).unwrap();
            mem[1] = 0;
            assert!(tlv_area_complete(&mem));
            assert_eq!(
                parse_user_memory(&tag(&mem)),
                Err(NdefError::NoUsableRecord)
            );
        }
    }
}
