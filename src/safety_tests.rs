//! Safety tests: the allowlist, the wire format and robustness against hostile
//! input. These complement the unit tests in each module with exhaustive and
//! randomised checks. Everything here is test-only.
//!
//! What is checked:
//! 1. Every byte string of length 0–3 is accepted **exactly** when it is one of the
//!    allowed read forms, and accepted requests are sent byte for byte as given.
//!    Longer strings are checked with large random samples.
//! 2. Every AT command of up to four characters (and every `ATSH`/`ATCRA` with a
//!    3-digit ID) is accepted exactly when it is on the list.
//! 3. Every command that can be constructed produces a wire string that is plain
//!    ASCII letters/digits, non-empty, and parses back to itself.
//! 4. Text tricks (control characters, separators, look-alike digits) are rejected.
//! 5. Random and malformed responses never make the parsers or a module scan panic.
//! 6. Tripwires in the source code: only one place opens or writes to the serial
//!    port, and every transport goes through `checked_wire`.

use crate::command::*;
use crate::dtc;
use crate::elm::{self, Bus, Elm};
use crate::isotp;
use crate::replay;
use crate::transport::MockTransport;

/// Small deterministic random number generator (xorshift64*), so the random tests
/// are reproducible without extra dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The complete set of allowed diagnostic request forms, written out
/// independently of the parser in `command.rs`.
fn is_allowed_form(b: &[u8]) -> bool {
    matches!(
        b,
        [0x01, _]
            | [0x03]
            | [0x07]
            | [0x09, _]
            | [0x0A]
            | [0x19, 0x01, _]
            | [0x19, 0x02, _]
            | [0x19, 0x0A]
            | [0x22, _, _]
            | [0x3E, 0x00]
    )
}

/// Services that are read-only by definition. Anything accepted must start with
/// one of these.
const READ_ONLY_SERVICES: [u8; 8] = [0x01, 0x03, 0x07, 0x09, 0x0A, 0x19, 0x22, 0x3E];

/// Services that must never be accepted in any form.
const FORBIDDEN: [u8; 16] = [
    0x04, 0x08, 0x10, 0x11, 0x14, 0x27, 0x28, 0x2E, 0x2F, 0x31, 0x34, 0x35, 0x36, 0x37, 0x3D, 0x85,
];

fn check_bytes(b: &[u8]) {
    let parsed = DiagRequest::from_bytes(b);
    assert_eq!(
        parsed.is_ok(),
        is_allowed_form(b),
        "bytes {b:02X?}: accepted = {}",
        parsed.is_ok()
    );
    if let Ok(req) = parsed {
        assert_eq!(req.to_bytes(), b, "accepted request must be sent exactly as given");
        assert!(READ_ONLY_SERVICES.contains(&b[0]));
        assert!(checked_wire(&Command::Diag(req)).is_ok());
    }
}

// ---------------------------------------------------------------------------
// 1. Diagnostic requests
// ---------------------------------------------------------------------------

#[test]
fn every_request_up_to_three_bytes_is_classified_exactly() {
    check_bytes(&[]);
    for a in 0..=255u8 {
        check_bytes(&[a]);
        for b in 0..=255u8 {
            check_bytes(&[a, b]);
        }
    }
    // All 16.7 million 3-byte strings.
    for a in 0..=255u8 {
        for b in 0..=255u8 {
            for c in 0..=255u8 {
                let bytes = [a, b, c];
                // Inline the cheap part of check_bytes for speed.
                let ok = DiagRequest::from_bytes(&bytes).is_ok();
                assert_eq!(ok, is_allowed_form(&bytes), "bytes {bytes:02X?}");
            }
        }
    }
}

#[test]
fn longer_requests_are_always_rejected() {
    // No allowed form is longer than 3 bytes. Random lengths 4–64, plus strings
    // that start with an allowed service (the most interesting case).
    let mut rng = Rng(0x5EED_0001);
    for i in 0..500_000 {
        let len = 4 + rng.below(61);
        let mut b: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
        if i % 2 == 0 {
            b[0] = READ_ONLY_SERVICES[rng.below(READ_ONLY_SERVICES.len())];
        }
        assert!(
            DiagRequest::from_bytes(&b).is_err(),
            "bytes {b:02X?} should be rejected"
        );
    }
}

#[test]
fn forbidden_services_are_rejected_with_any_arguments() {
    let mut rng = Rng(0x5EED_0002);
    for &s in &FORBIDDEN {
        for len in 0..=16 {
            for _ in 0..200 {
                let mut b = vec![s];
                b.extend((0..len).map(|_| rng.byte()));
                assert!(
                    matches!(DiagRequest::from_bytes(&b), Err(Forbidden::ForbiddenService(..))),
                    "{b:02X?}"
                );
                // Also as text, with and without spaces.
                let hex: String = b.iter().map(|x| format!("{x:02X} ")).collect();
                assert!(Command::parse(&hex).is_err(), "{hex}");
                assert!(Command::parse(&hex.replace(' ', "")).is_err(), "{hex}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 2. AT commands
// ---------------------------------------------------------------------------

/// The complete set of allowed AT commands (after removing whitespace and
/// upper-casing), written out independently of the parser.
fn is_allowed_at(rest: &str) -> bool {
    const FIXED: [&str; 16] = [
        "Z", "E0", "L0", "S0", "S1", "H0", "H1", "SP0", "SP6", "SPB", "PB8104", "CAF1", "CRA", "RV", "I", "DP",
    ];
    if FIXED.contains(&rest) {
        return true;
    }
    let hex = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit());
    let diag_id = |s: &str| s.len() == 3 && hex(s) && (0x700..=0x7FF).contains(&u16::from_str_radix(s, 16).unwrap());
    if let Some(id) = rest.strip_prefix("SH") {
        return diag_id(id);
    }
    if let Some(id) = rest.strip_prefix("CRA") {
        return diag_id(id);
    }
    if let Some(t) = rest.strip_prefix("ST") {
        return t.len() == 2 && hex(t);
    }
    false
}

#[test]
fn every_at_command_up_to_four_characters_is_classified_exactly() {
    let chars: Vec<char> = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ@#".chars().collect();
    let mut rest = String::new();
    fn walk(chars: &[char], rest: &mut String, depth: usize) {
        let text = format!("AT{rest}");
        let accepted = AtCommand::parse(&text);
        assert_eq!(accepted.is_ok(), is_allowed_at(rest), "\"{text}\"");
        if let Ok(cmd) = accepted {
            assert_eq!(cmd.to_wire(), text, "accepted AT command must be sent as given");
        }
        if depth == 4 {
            return;
        }
        for &c in chars {
            rest.push(c);
            walk(chars, rest, depth + 1);
            rest.pop();
        }
    }
    walk(&chars, &mut rest, 0);
}

#[test]
fn header_and_filter_accept_only_diagnostic_ids() {
    for id in 0..=0xFFFu16 {
        let text = format!("{id:03X}");
        let allowed = (0x700..=0x7FF).contains(&id);
        assert_eq!(AtCommand::parse(&format!("ATSH{text}")).is_ok(), allowed, "ATSH{text}");
        assert_eq!(
            AtCommand::parse(&format!("ATCRA{text}")).is_ok(),
            allowed,
            "ATCRA{text}"
        );
    }
    // 29-bit headers and other lengths are never allowed.
    for text in [
        "ATSH18DA10F1",
        "ATSH0007E0",
        "ATSH7E00",
        "ATSH7E",
        "ATCRA18DAF110",
        "ATCRA7E80",
    ] {
        assert!(AtCommand::parse(text).is_err(), "{text}");
    }
    for id in 0..=0xFFFFu16 {
        assert_eq!(CanId::new(id).is_ok(), (0x700..=0x7FF).contains(&id));
    }
}

#[test]
fn dangerous_at_commands_are_rejected() {
    for text in [
        "ATCAF0",       // raw CAN frames
        "ATMA",         // monitor all
        "ATMR00",       // monitor for receiver
        "ATMT00",       // monitor for transmitter
        "ATBRD45",      // change RS232 baud rate
        "ATBRT00",      // baud rate timeout
        "ATCV1200",     // voltage calibration (stored in EEPROM)
        "ATPP2CSV81",   // programmable parameters (EEPROM)
        "ATPP2CON",     //
        "ATPPFFOFF",    //
        "ATSD00",       // save data byte (EEPROM)
        "ATWM8110F1",   // wakeup message
        "ATFCSH7E0",    // flow control header
        "ATFCSD300000", // flow control data
        "ATFCSM1",      // flow control mode
        "ATCM7FF",      // CAN mask
        "ATCF7E8",      // CAN filter
        "ATCSM0",       // CAN silent monitoring off
        "ATD",          // set all to defaults
        "ATWS",         // warm start
        "ATLP",         // low power mode
        "ATBD",         // buffer dump
        "ATBI",         // bypass initialisation
        "ATSP1",
        "ATSP2",
        "ATSP3",
        "ATSP4",
        "ATSP5",
        "ATSP7",
        "ATSP8",
        "ATSP9",
        "ATSPA",
        "ATSPC",
        "ATSPA6", // auto with fallback
        "ATTP6",  // try protocol
        "ATPB C1 04",
        "ATPB 81 08",
        "ATPB E0 04",
        "ATPB 81 01",
        "ATH2",
        "ATS2",
        "ATE1",
        "ATL1",
        "ATCAF2",
    ] {
        assert!(AtCommand::parse(text).is_err(), "\"{text}\" should be rejected");
        assert!(Command::parse(text).is_err(), "\"{text}\" should be rejected");
    }
}

// ---------------------------------------------------------------------------
// 3. Every constructible command has a safe wire format
// ---------------------------------------------------------------------------

/// Every command that can be built, with every parameter value.
fn all_commands() -> Vec<Command> {
    let mut v: Vec<Command> = vec![
        AtCommand::Reset.into(),
        AtCommand::EchoOff.into(),
        AtCommand::LinefeedsOff.into(),
        AtCommand::Spaces(false).into(),
        AtCommand::Spaces(true).into(),
        AtCommand::Headers(false).into(),
        AtCommand::Headers(true).into(),
        AtCommand::SetProtocol(Protocol::Auto).into(),
        AtCommand::SetProtocol(Protocol::HsCan500k).into(),
        AtCommand::SetProtocol(Protocol::UserB).into(),
        AtCommand::ProtocolB(ProtocolBConfig::MsCan125k).into(),
        AtCommand::ClearReceiveFilter.into(),
        AtCommand::CanAutoFormatOn.into(),
        AtCommand::ReadVoltage.into(),
        AtCommand::Identify.into(),
        AtCommand::DescribeProtocol.into(),
        DiagRequest::Obd(ObdRequest::StoredDtcs).into(),
        DiagRequest::Obd(ObdRequest::PendingDtcs).into(),
        DiagRequest::Obd(ObdRequest::PermanentDtcs).into(),
        DiagRequest::Uds(UdsRequest::ReadDtcInformation(DtcReport::Supported)).into(),
        DiagRequest::Uds(UdsRequest::TesterPresent).into(),
    ];
    for id in 0x700..=0x7FFu16 {
        let id = CanId::new(id).unwrap();
        v.push(AtCommand::SetHeader(id).into());
        v.push(AtCommand::ReceiveFilter(id).into());
    }
    for x in 0..=255u8 {
        v.push(AtCommand::Timeout(x).into());
        v.push(DiagRequest::Obd(ObdRequest::CurrentData { pid: x }).into());
        v.push(DiagRequest::Obd(ObdRequest::VehicleInfo { pid: x }).into());
        v.push(DiagRequest::Uds(UdsRequest::ReadDtcInformation(DtcReport::NumberByStatusMask(x))).into());
        v.push(DiagRequest::Uds(UdsRequest::ReadDtcInformation(DtcReport::ByStatusMask(x))).into());
    }
    for did in 0..=0xFFFFu16 {
        v.push(DiagRequest::Uds(UdsRequest::ReadDataByIdentifier { did }).into());
    }
    v
}

#[test]
fn every_constructible_command_has_a_safe_wire_format() {
    let all = all_commands();
    assert!(all.len() > 66_000);
    for cmd in &all {
        let wire = checked_wire(cmd).unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
        assert!(
            !wire.is_empty(),
            "{cmd:?}: empty wire would repeat the previous command"
        );
        assert!(
            wire.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
            "{cmd:?}: wire \"{wire}\" contains other characters"
        );
        assert_eq!(
            Command::parse(&wire).as_ref(),
            Ok(cmd),
            "{cmd:?}: does not parse back to itself"
        );
        if let Command::Diag(d) = cmd {
            let first = d.to_bytes()[0];
            assert!(READ_ONLY_SERVICES.contains(&first));
            assert!(!FORBIDDEN.contains(&first));
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Text tricks
// ---------------------------------------------------------------------------

#[test]
fn text_tricks_are_rejected() {
    for text in [
        // Control characters and line breaks: a second command must never ride along.
        "ATZ\r14",
        "ATZ\n14",
        "01 00\r14 FF FF FF",
        "01\r00",
        "ATRV\r",
        "\rATZ",
        "ATZ\0",
        "03\u{0}",
        "ATSH7E0\r2E",
        "AT\rZ",
        "19 02 FF\r\n04",
        // Separators and prefixes that the parser must not skip over.
        "ATZ;14",
        "01,00",
        "0x01 0x00",
        "0x14",
        "$14",
        "#14",
        "+14",
        "-14",
        "01-00",
        "01:00",
        "ATZ ATZ",
        // Look-alike characters: full-width, Arabic-Indic and superscript digits,
        // Cyrillic letters.
        "\u{FF10}\u{FF11} 00",
        "\u{0661}\u{0664}",
        "\u{00B9}\u{2074}",
        "AТZ",
        "АTZ",
        "ATЅH7E0",
        // Odd lengths and stray characters.
        "1",
        "014",
        "0 1 0",
        "01 0",
        "ZZ",
        "GG",
        "01 00 zz",
        "",
        "   ",
        "\t",
    ] {
        let r = Command::parse(text);
        assert!(r.is_err(), "{text:?} should be rejected, got {r:?}");
    }
    // Whitespace inside a valid command is allowed and does not change it.
    assert_eq!(Command::parse(" 19 02  FF "), Command::parse("1902FF"));
    assert_eq!(Command::parse("at sh 7e0"), Command::parse("ATSH7E0"));
}

// ---------------------------------------------------------------------------
// 5. Robustness against hostile or broken responses
// ---------------------------------------------------------------------------

/// Random text that looks a bit like ELM327 output.
fn random_response(rng: &mut Rng) -> String {
    const PIECES: [&str; 22] = [
        "7E8",
        "72E",
        "7D8",
        "728",
        "10",
        "21",
        "22",
        "2F",
        "03",
        "07",
        "0F",
        "FF",
        "59",
        "7F",
        "78",
        "43",
        "\r",
        " ",
        "NO DATA",
        "?",
        "SEARCHING...",
        "\u{0}",
    ];
    let n = rng.below(40);
    let mut s = String::new();
    for _ in 0..n {
        match rng.below(4) {
            0 => s.push(rng.byte() as char),
            1 => s.push_str(&format!("{:02X}", rng.byte())),
            _ => s.push_str(PIECES[rng.below(PIECES.len())]),
        }
        if rng.below(3) == 0 {
            s.push(' ');
        }
    }
    s
}

#[test]
fn parsers_never_panic_on_random_input() {
    let mut rng = Rng(0x5EED_0003);
    for _ in 0..50_000 {
        let text = random_response(&mut rng);
        let lines = elm::clean_response(&text, "1902FF");
        let _ = isotp::reassemble(&lines);
        let _ = elm::parse_voltage(&text);
        let _ = replay::parse_log(&text);
        let bytes: Vec<u8> = (0..rng.below(40)).map(|_| rng.byte()).collect();
        let _ = dtc::parse_obd_dtcs(&bytes);
        let _ = dtc::parse_uds_dtcs(&bytes);
        let _ = Command::parse(&text);
    }
}

#[test]
fn module_scan_never_panics_on_random_responses() {
    let mut rng = Rng(0x5EED_0004);
    let bcm = dtc::find_module("BCM").unwrap();
    let pcm = dtc::find_module("PCM").unwrap();
    for _ in 0..3_000 {
        let mut mock = MockTransport::elm_defaults();
        for wire in ["03", "07", "0A", "1902FF"] {
            for header in ["726", "7E0"] {
                mock = mock.on_target(header, wire, &random_response(&mut rng));
            }
        }
        let mut elm = Elm::new(Box::new(mock));
        elm.init(Bus::Hs).unwrap();
        // Errors are fine; panics are not.
        let _ = dtc::scan_module(&mut elm, bcm, false);
        let _ = dtc::scan_module(&mut elm, pcm, true);
    }
}

#[test]
fn isotp_handles_extreme_lengths() {
    // A first frame announcing 4095 bytes that never arrive must be an error,
    // not a hang or a huge allocation.
    let lines = vec!["72E 1F FF 59 02 FF 00 00 00".to_string()];
    assert!(isotp::reassemble(&lines).is_err());
    // Many consecutive frames with wrapping sequence numbers.
    let mut lines = vec!["72E 10 FF 59 02 FF 00 00 00".to_string()];
    let mut seq = 1u8;
    let mut got = 6usize;
    while got < 0xFF {
        lines.push(format!("72E {:02X} 00 00 00 00 00 00 00", 0x20 | seq));
        seq = (seq + 1) & 0x0F;
        got += 7;
    }
    let m = isotp::reassemble(&lines).unwrap();
    assert_eq!(m[0].data.len(), 0xFF);
}

// ---------------------------------------------------------------------------
// 6. Source code tripwires
// ---------------------------------------------------------------------------

const SOURCES: [(&str, &str); 9] = [
    ("main.rs", include_str!("main.rs")),
    ("command.rs", include_str!("command.rs")),
    ("transport.rs", include_str!("transport.rs")),
    ("elm.rs", include_str!("elm.rs")),
    ("dtc.rs", include_str!("dtc.rs")),
    ("isotp.rs", include_str!("isotp.rs")),
    ("battery.rs", include_str!("battery.rs")),
    ("report.rs", include_str!("report.rs")),
    ("replay.rs", include_str!("replay.rs")),
];

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

#[test]
fn only_one_place_opens_and_writes_the_serial_port() {
    for (name, src) in SOURCES {
        let opens = count(src, "serialport::new(");
        // Writes to a port or stream. `log.write(...)` (the raw log helper) is fine;
        // SerialTransport's `self.port.write_all(...)` is counted by both patterns.
        let writes = count(src, ".write_all(") + count(src, "port.write");
        if name == "transport.rs" {
            assert_eq!(opens, 1, "{name}: serialport::new");
            assert_eq!(
                writes, 2,
                "{name}: only SerialTransport::exchange may write to the port"
            );
        } else {
            assert_eq!(opens, 0, "{name} must not open the serial port");
            assert_eq!(writes, 0, "{name} must not write to a port or stream directly");
        }
    }
}

#[test]
fn every_transport_takes_a_command_and_checks_the_wire() {
    let mut impls = 0;
    for (name, src) in SOURCES {
        let n_impl = count(src, "impl Transport for");
        impls += n_impl;
        if n_impl == 0 {
            continue;
        }
        for line in src.lines().filter(|l| l.contains("fn exchange(")) {
            assert!(
                line.contains("cmd: &Command"),
                "{name}: exchange must take &Command: {line}"
            );
        }
        assert!(
            count(src, "checked_wire(cmd)") >= n_impl,
            "{name}: every transport must call checked_wire(cmd)"
        );
    }
    // SerialTransport, MockTransport, Logged and ReplayTransport.
    assert_eq!(
        impls, 4,
        "a new transport was added: make sure it calls checked_wire and update this test"
    );
}

#[test]
fn no_raw_send_api_exists() {
    // The Transport trait must only have `exchange(&Command)` and `describe()`.
    let src = include_str!("transport.rs");
    let start = src.find("pub trait Transport {").unwrap();
    let end = start + src[start..].find("\n}").unwrap();
    let trait_body = &src[start..end];
    let fns: Vec<&str> = trait_body
        .lines()
        .filter(|l| l.trim_start().starts_with("fn "))
        .collect();
    assert_eq!(fns.len(), 2, "unexpected methods in Transport: {fns:?}");
    assert!(fns[0].contains("fn exchange(&mut self, cmd: &Command"));
    assert!(fns[1].contains("fn describe(&self)"));
    // There is no command variant for raw bytes or text.
    let cmd_src = include_str!("command.rs");
    for bad in ["Raw(", "Custom(", "Bytes(Vec<u8>)", "Text(String)"] {
        assert!(!cmd_src.contains(bad), "command.rs contains {bad}");
    }
}
