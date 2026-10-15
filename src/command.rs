//! Allowlist for everything that can be sent to the ELM327 chip and the car.
//!
//! The safety principle is that **forbidden commands cannot be constructed**:
//!
//! * Everything that goes out on the cable is a [`Command`]. It is either an
//!   [`AtCommand`] (to the ELM327 chip itself) or a [`DiagRequest`]
//!   (a diagnostic request to a module in the car).
//! * Both are `enum`s where every variant is a legitimate read operation.
//!   There is no "raw bytes" variant, so e.g. `04` (clear DTCs) or UDS `2E`
//!   (write) cannot be expressed at all.
//! * If we need to write a command as text (debugging), it goes through
//!   [`Command::parse`], which maps the text to one of the variants above –
//!   or returns [`Forbidden`] for anything else.
//! * CAN IDs are a newtype ([`CanId`]) with a private field, so they must go
//!   through a validating constructor.
//!
//! The transport layer (`transport.rs`) only accepts `&Command`, and also
//! checks that the text actually sent parses back to the same command.

use std::fmt;
use thiserror::Error;

/// The reason a command was rejected.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Forbidden {
    #[error("empty command")]
    Empty,
    #[error("invalid hex: \"{0}\"")]
    BadHex(String),
    #[error("service 0x{0:02X} is FORBIDDEN ({1}) – use FORScan if you really need this")]
    ForbiddenService(u8, &'static str),
    #[error("service 0x{0:02X} is not on the allowlist")]
    NotAllowed(u8),
    #[error("invalid format for service 0x{service:02X}: {reason}")]
    BadFormat { service: u8, reason: &'static str },
    #[error("AT command \"{0}\" is not on the allowlist")]
    AtNotAllowed(String),
    #[error("CAN ID 0x{0:X} is outside the diagnostic range 0x700–0x7FF")]
    CanIdOutOfRange(u16),
}

// ---------------------------------------------------------------------------
// CAN ID
// ---------------------------------------------------------------------------

/// An 11-bit CAN ID in the diagnostic range 0x700–0x7FF.
///
/// We restrict to this range so the tool cannot be used to send messages on
/// IDs that ordinary modules use for control while driving.
/// The field is private: the only way in is [`CanId::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanId(u16);

impl CanId {
    pub fn new(id: u16) -> Result<Self, Forbidden> {
        if (0x700..=0x7FF).contains(&id) {
            Ok(CanId(id))
        } else {
            Err(Forbidden::CanIdOutOfRange(id))
        }
    }

    pub fn value(self) -> u16 {
        self.0
    }

    /// Usual response address for physical addressing: request + 8.
    /// Does not apply to the functional address 0x7DF, and 0x7F8–0x7FF have no +8.
    pub fn response_id(self) -> Option<CanId> {
        if self.0 == 0x7DF {
            return None;
        }
        CanId::new(self.0 + 8).ok()
    }

    fn parse(s: &str) -> Result<Self, Forbidden> {
        let id = u16::from_str_radix(s, 16).map_err(|_| Forbidden::BadHex(s.to_string()))?;
        if s.len() != 3 {
            return Err(Forbidden::BadHex(s.to_string()));
        }
        CanId::new(id)
    }
}

impl fmt::Display for CanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:03X}", self.0)
    }
}

// ---------------------------------------------------------------------------
// AT commands (to the ELM327 chip)
// ---------------------------------------------------------------------------

/// Protocols we allow with `ATSP`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// `ATSP0` – automatic search.
    Auto,
    /// `ATSP6` – ISO 15765-4 CAN, 11-bit ID, 500 kbit/s (Ford HS-CAN).
    HsCan500k,
    /// `ATSPB` – the "USER1" protocol, configured with `ATPB` (used for MS-CAN).
    UserB,
}

impl Protocol {
    fn code(self) -> char {
        match self {
            Protocol::Auto => '0',
            Protocol::HsCan500k => '6',
            Protocol::UserB => 'B',
        }
    }
}

/// Allowed configurations for protocol B (`ATPB xx yy`).
///
/// Only fixed, known configurations are allowed, because the wrong bit rate
/// on a CAN bus can disturb the traffic on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolBConfig {
    /// `ATPB 81 04` – Ford MS-CAN, 125 kbit/s. According to the ELM327 data
    /// sheet (PP 2C/2D): 0x81 = 11-bit ID, fixed 8-byte length, ISO 15765-4
    /// format; 0x04 = 500/4 = 125 kbit/s.
    MsCan125k,
}

impl ProtocolBConfig {
    fn bytes(self) -> (u8, u8) {
        match self {
            ProtocolBConfig::MsCan125k => (0x81, 0x04),
        }
    }
}

/// All AT commands the tool can send to the ELM327 chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtCommand {
    /// `ATZ` – reset the chip.
    Reset,
    /// `ATE0` – echo off.
    EchoOff,
    /// `ATL0` – linefeeds after CR off.
    LinefeedsOff,
    /// `ATS0`/`ATS1` – spaces in responses off/on.
    Spaces(bool),
    /// `ATH0`/`ATH1` – show CAN ID (header) in responses off/on.
    Headers(bool),
    /// `ATSPx` – select protocol.
    SetProtocol(Protocol),
    /// `ATPB xx yy` – configure protocol B.
    ProtocolB(ProtocolBConfig),
    /// `ATSH xxx` – CAN ID that requests are sent to.
    SetHeader(CanId),
    /// `ATCRA xxx` – only receive responses from this CAN ID.
    ReceiveFilter(CanId),
    /// `ATCRA` without argument – clear the receive filter.
    ClearReceiveFilter,
    /// `ATCAF1` – automatic ISO 15765 formatting on (ELM adds the PCI byte).
    /// `ATCAF0` is deliberately left out: it would allow arbitrary CAN frames.
    CanAutoFormatOn,
    /// `ATST xx` – response timeout, xx × 4 ms.
    Timeout(u8),
    /// `ATRV` – read the voltage on the OBD connector (pin 16).
    ReadVoltage,
    /// `ATI` – version string.
    Identify,
    /// `ATDP` – description of the active protocol.
    DescribeProtocol,
}

impl AtCommand {
    /// The text sent to the ELM327 (without CR).
    pub fn to_wire(&self) -> String {
        match self {
            AtCommand::Reset => "ATZ".into(),
            AtCommand::EchoOff => "ATE0".into(),
            AtCommand::LinefeedsOff => "ATL0".into(),
            AtCommand::Spaces(on) => format!("ATS{}", *on as u8),
            AtCommand::Headers(on) => format!("ATH{}", *on as u8),
            AtCommand::SetProtocol(p) => format!("ATSP{}", p.code()),
            AtCommand::ProtocolB(cfg) => {
                let (a, b) = cfg.bytes();
                format!("ATPB{a:02X}{b:02X}")
            }
            AtCommand::SetHeader(id) => format!("ATSH{id}"),
            AtCommand::ReceiveFilter(id) => format!("ATCRA{id}"),
            AtCommand::ClearReceiveFilter => "ATCRA".into(),
            AtCommand::CanAutoFormatOn => "ATCAF1".into(),
            AtCommand::Timeout(t) => format!("ATST{t:02X}"),
            AtCommand::ReadVoltage => "ATRV".into(),
            AtCommand::Identify => "ATI".into(),
            AtCommand::DescribeProtocol => "ATDP".into(),
        }
    }

    /// Parse an AT command written as text. Whitespace and letter case do not
    /// matter. Anything not on the list is rejected.
    pub fn parse(text: &str) -> Result<Self, Forbidden> {
        let s: String = text
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_uppercase();
        let reject = || Forbidden::AtNotAllowed(text.trim().to_string());
        let rest = s.strip_prefix("AT").ok_or_else(reject)?;

        let cmd = match rest {
            "Z" => AtCommand::Reset,
            "E0" => AtCommand::EchoOff,
            "L0" => AtCommand::LinefeedsOff,
            "S0" => AtCommand::Spaces(false),
            "S1" => AtCommand::Spaces(true),
            "H0" => AtCommand::Headers(false),
            "H1" => AtCommand::Headers(true),
            "SP0" => AtCommand::SetProtocol(Protocol::Auto),
            "SP6" => AtCommand::SetProtocol(Protocol::HsCan500k),
            "SPB" => AtCommand::SetProtocol(Protocol::UserB),
            "PB8104" => AtCommand::ProtocolB(ProtocolBConfig::MsCan125k),
            "CAF1" => AtCommand::CanAutoFormatOn,
            "CRA" => AtCommand::ClearReceiveFilter,
            "RV" => AtCommand::ReadVoltage,
            "I" => AtCommand::Identify,
            "DP" => AtCommand::DescribeProtocol,
            _ => {
                // Commands with arguments.
                if let Some(id) = rest.strip_prefix("SH") {
                    AtCommand::SetHeader(CanId::parse(id)?)
                } else if let Some(id) = rest.strip_prefix("CRA") {
                    AtCommand::ReceiveFilter(CanId::parse(id)?)
                } else if let Some(t) = rest.strip_prefix("ST") {
                    if t.len() != 2 {
                        return Err(reject());
                    }
                    AtCommand::Timeout(u8::from_str_radix(t, 16).map_err(|_| reject())?)
                } else {
                    return Err(reject());
                }
            }
        };
        Ok(cmd)
    }
}

// ---------------------------------------------------------------------------
// Diagnostic requests (to modules in the car)
// ---------------------------------------------------------------------------

/// Standard OBD-II read modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObdRequest {
    /// Mode `01 pp` – live data for one PID.
    CurrentData { pid: u8 },
    /// Mode `03` – stored DTCs.
    StoredDtcs,
    /// Mode `07` – pending DTCs.
    PendingDtcs,
    /// Mode `09 pp` – vehicle information (VIN etc.).
    VehicleInfo { pid: u8 },
    /// Mode `0A` – permanent DTCs.
    PermanentDtcs,
}

/// Allowed sub-functions of UDS `19` (ReadDTCInformation). All are reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtcReport {
    /// `19 01 mm` – number of DTCs matching the status mask.
    NumberByStatusMask(u8),
    /// `19 02 mm` – DTCs matching the status mask.
    ByStatusMask(u8),
    /// `19 0A` – all DTCs the module supports.
    Supported,
}

/// Allowed UDS read services.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdsRequest {
    /// `19 …` – ReadDTCInformation.
    ReadDtcInformation(DtcReport),
    /// `22 hh ll` – ReadDataByIdentifier, one DID.
    ReadDataByIdentifier { did: u16 },
    /// `3E 00` – TesterPresent (with positive response).
    TesterPresent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagRequest {
    Obd(ObdRequest),
    Uds(UdsRequest),
}

/// Services that are explicitly forbidden, with a reason. Anything not on the
/// allowlist is rejected anyway; this list only gives better error messages.
const FORBIDDEN_SERVICES: &[(u8, &str)] = &[
    (0x04, "OBD: clear DTCs"),
    (0x08, "OBD: control on-board system"),
    (0x10, "UDS: DiagnosticSessionControl"),
    (0x11, "UDS: ECUReset"),
    (0x14, "UDS: ClearDiagnosticInformation"),
    (0x27, "UDS: SecurityAccess"),
    (0x28, "UDS: CommunicationControl"),
    (0x2E, "UDS: WriteDataByIdentifier"),
    (0x2F, "UDS: InputOutputControlByIdentifier"),
    (0x31, "UDS: RoutineControl"),
    (0x34, "UDS: RequestDownload"),
    (0x35, "UDS: RequestUpload"),
    (0x36, "UDS: TransferData"),
    (0x37, "UDS: RequestTransferExit"),
    (0x3D, "UDS: WriteMemoryByAddress"),
    (0x85, "UDS: ControlDTCSetting"),
];

impl DiagRequest {
    /// The bytes of the request.
    pub fn to_bytes(&self) -> Vec<u8> {
        match *self {
            DiagRequest::Obd(o) => match o {
                ObdRequest::CurrentData { pid } => vec![0x01, pid],
                ObdRequest::StoredDtcs => vec![0x03],
                ObdRequest::PendingDtcs => vec![0x07],
                ObdRequest::VehicleInfo { pid } => vec![0x09, pid],
                ObdRequest::PermanentDtcs => vec![0x0A],
            },
            DiagRequest::Uds(u) => match u {
                UdsRequest::ReadDtcInformation(r) => match r {
                    DtcReport::NumberByStatusMask(m) => vec![0x19, 0x01, m],
                    DtcReport::ByStatusMask(m) => vec![0x19, 0x02, m],
                    DtcReport::Supported => vec![0x19, 0x0A],
                },
                UdsRequest::ReadDataByIdentifier { did } => vec![0x22, (did >> 8) as u8, did as u8],
                UdsRequest::TesterPresent => vec![0x3E, 0x00],
            },
        }
    }

    /// The text sent to the ELM327, e.g. `1902FF`.
    pub fn to_wire(&self) -> String {
        self.to_bytes().iter().map(|b| format!("{b:02X}")).collect()
    }

    /// Validating constructor for raw hex, e.g. `"19 02 FF"`.
    ///
    /// The bytes are mapped to one of the variants above. Anything else –
    /// forbidden services, unknown services, wrong length – returns `Err`.
    pub fn from_hex(text: &str) -> Result<Self, Forbidden> {
        let bytes = parse_hex_bytes(text)?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Forbidden> {
        let (&service, args) = bytes.split_first().ok_or(Forbidden::Empty)?;

        if let Some(&(_, why)) = FORBIDDEN_SERVICES.iter().find(|(s, _)| *s == service) {
            return Err(Forbidden::ForbiddenService(service, why));
        }

        let bad = |reason| Forbidden::BadFormat { service, reason };
        use DiagRequest::{Obd, Uds};
        let req = match (service, args) {
            (0x01, [pid]) => Obd(ObdRequest::CurrentData { pid: *pid }),
            (0x01, _) => return Err(bad("expected exactly one PID")),
            (0x03, []) => Obd(ObdRequest::StoredDtcs),
            (0x07, []) => Obd(ObdRequest::PendingDtcs),
            (0x0A, []) => Obd(ObdRequest::PermanentDtcs),
            (0x03 | 0x07 | 0x0A, _) => return Err(bad("this mode takes no arguments")),
            (0x09, [pid]) => Obd(ObdRequest::VehicleInfo { pid: *pid }),
            (0x09, _) => return Err(bad("expected exactly one PID")),
            (0x19, [0x01, mask]) => Uds(UdsRequest::ReadDtcInformation(DtcReport::NumberByStatusMask(*mask))),
            (0x19, [0x02, mask]) => Uds(UdsRequest::ReadDtcInformation(DtcReport::ByStatusMask(*mask))),
            (0x19, [0x0A]) => Uds(UdsRequest::ReadDtcInformation(DtcReport::Supported)),
            (0x19, _) => return Err(bad("only sub-functions 01 mm, 02 mm and 0A are allowed")),
            (0x22, [hi, lo]) => Uds(UdsRequest::ReadDataByIdentifier {
                did: u16::from_be_bytes([*hi, *lo]),
            }),
            (0x22, _) => return Err(bad("expected exactly one DID (2 bytes)")),
            (0x3E, [0x00]) => Uds(UdsRequest::TesterPresent),
            (0x3E, _) => return Err(bad("only \"3E 00\" is allowed")),
            _ => return Err(Forbidden::NotAllowed(service)),
        };
        Ok(req)
    }
}

/// Parse hex text such as "19 02 FF" or "1902ff" into bytes.
fn parse_hex_bytes(text: &str) -> Result<Vec<u8>, Forbidden> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() {
        return Err(Forbidden::Empty);
    }
    if compact.len() % 2 != 0 || !compact.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Forbidden::BadHex(text.trim().to_string()));
    }
    (0..compact.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&compact[i..i + 2], 16).map_err(|_| Forbidden::BadHex(text.to_string())))
        .collect()
}

// ---------------------------------------------------------------------------
// Combined command type
// ---------------------------------------------------------------------------

/// Everything that can be sent on the cable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    At(AtCommand),
    Diag(DiagRequest),
}

impl Command {
    pub fn to_wire(&self) -> String {
        match self {
            Command::At(a) => a.to_wire(),
            Command::Diag(d) => d.to_wire(),
        }
    }

    /// Parse text into a command. Text starting with "AT" is parsed as an AT
    /// command, anything else as a hex diagnostic request.
    pub fn parse(text: &str) -> Result<Self, Forbidden> {
        let t = text.trim();
        if t.len() >= 2 && t[..2].eq_ignore_ascii_case("AT") {
            AtCommand::parse(t).map(Command::At)
        } else {
            DiagRequest::from_hex(t).map(Command::Diag)
        }
    }
}

impl From<AtCommand> for Command {
    fn from(a: AtCommand) -> Self {
        Command::At(a)
    }
}

impl From<DiagRequest> for Command {
    fn from(d: DiagRequest) -> Self {
        Command::Diag(d)
    }
}

/// Last safety check before anything is written to the cable: the text must
/// parse back to exactly the same command. Called by every transport.
pub fn checked_wire(cmd: &Command) -> Result<String, Forbidden> {
    let wire = cmd.to_wire();
    match Command::parse(&wire) {
        Ok(back) if back == *cmd => Ok(wire),
        Ok(_) => Err(Forbidden::AtNotAllowed(wire)),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn is_forbidden(text: &str) -> bool {
        matches!(DiagRequest::from_hex(text), Err(Forbidden::ForbiddenService(..)))
    }

    #[test]
    fn forbidden_services_are_rejected() {
        // Every forbidden service, both alone and with typical arguments.
        for text in [
            "04",
            "14",
            "14 FF FF FF",
            "2E F1 90 00",
            "2F 01 02 03",
            "31 01 FF 00",
            "11 01",
            "10 01",
            "10 02",
            "10 03",
            "27 01",
            "27 02 00 00",
            "28 00 01",
            "85 02",
            "34 00 44",
            "35 00",
            "36 01",
            "37",
            "08 01",
            "3D 14 00",
        ] {
            assert!(is_forbidden(text), "\"{text}\" should be forbidden");
            assert!(Command::parse(text).is_err());
        }
    }

    #[test]
    fn all_services_outside_the_list_are_rejected() {
        let allowed = [0x01, 0x03, 0x07, 0x09, 0x0A, 0x19, 0x22, 0x3E];
        for service in 0u8..=255 {
            if allowed.contains(&service) {
                continue;
            }
            for len in 0..4 {
                let mut bytes = vec![service];
                bytes.extend(std::iter::repeat_n(0x00, len));
                assert!(
                    DiagRequest::from_bytes(&bytes).is_err(),
                    "0x{service:02X} should be rejected"
                );
            }
        }
    }

    #[test]
    fn allowed_read_requests_are_accepted() {
        assert_eq!(
            DiagRequest::from_hex("01 00"),
            Ok(DiagRequest::Obd(ObdRequest::CurrentData { pid: 0 }))
        );
        assert_eq!(
            DiagRequest::from_hex("03"),
            Ok(DiagRequest::Obd(ObdRequest::StoredDtcs))
        );
        assert_eq!(
            DiagRequest::from_hex("07"),
            Ok(DiagRequest::Obd(ObdRequest::PendingDtcs))
        );
        assert_eq!(
            DiagRequest::from_hex("0a"),
            Ok(DiagRequest::Obd(ObdRequest::PermanentDtcs))
        );
        assert_eq!(
            DiagRequest::from_hex("0902"),
            Ok(DiagRequest::Obd(ObdRequest::VehicleInfo { pid: 2 }))
        );
        assert_eq!(
            DiagRequest::from_hex("19 02 FF"),
            Ok(DiagRequest::Uds(UdsRequest::ReadDtcInformation(
                DtcReport::ByStatusMask(0xFF)
            )))
        );
        assert_eq!(
            DiagRequest::from_hex("22 F1 90"),
            Ok(DiagRequest::Uds(UdsRequest::ReadDataByIdentifier { did: 0xF190 }))
        );
        assert_eq!(
            DiagRequest::from_hex("3E 00"),
            Ok(DiagRequest::Uds(UdsRequest::TesterPresent))
        );
    }

    #[test]
    fn malformed_requests_are_rejected() {
        for text in [
            "",
            "1",
            "0",
            "zz",
            "01",
            "01 00 01",
            "03 00",
            "19",
            "19 04 FF",
            "19 02",
            "22 F1",
            "22 F1 90 F1 91",
            "3E 80",
            "3E",
        ] {
            assert!(DiagRequest::from_hex(text).is_err(), "\"{text}\" should be rejected");
        }
    }

    #[test]
    fn at_command_allowlist() {
        assert_eq!(AtCommand::parse("ATZ"), Ok(AtCommand::Reset));
        assert_eq!(
            AtCommand::parse("at sp 6"),
            Ok(AtCommand::SetProtocol(Protocol::HsCan500k))
        );
        assert_eq!(
            AtCommand::parse("AT PB 81 04"),
            Ok(AtCommand::ProtocolB(ProtocolBConfig::MsCan125k))
        );
        assert_eq!(
            AtCommand::parse("ATSH 7E0"),
            Ok(AtCommand::SetHeader(CanId::new(0x7E0).unwrap()))
        );
        assert_eq!(
            AtCommand::parse("ATCRA7E8"),
            Ok(AtCommand::ReceiveFilter(CanId::new(0x7E8).unwrap()))
        );
        assert_eq!(AtCommand::parse("ATST 32"), Ok(AtCommand::Timeout(0x32)));

        // Not on the list: monitoring, raw CAN, baud rate change, calibration,
        // programmable parameters, other PB configurations, IDs outside 7xx.
        for text in [
            "ATMA",
            "ATCAF0",
            "ATBRD45",
            "ATCV1250",
            "ATPP2CSV81",
            "ATPB C0 0F",
            "ATSH 123",
            "ATSH 7E",
            "ATSH 18DA10F1",
            "ATCRA 0F0",
            "ATWM8110",
            "ATSP1",
            "AT",
            "ATZ 04",
            "ATST1",
            "ZAT",
        ] {
            assert!(AtCommand::parse(text).is_err(), "\"{text}\" should be rejected");
        }
    }

    #[test]
    fn can_id_validation() {
        assert!(CanId::new(0x6FF).is_err());
        assert!(CanId::new(0x800).is_err());
        assert_eq!(
            CanId::new(0x726).unwrap().response_id(),
            Some(CanId::new(0x72E).unwrap())
        );
        assert_eq!(CanId::new(0x7DF).unwrap().response_id(), None);
        assert_eq!(CanId::new(0x7FF).unwrap().response_id(), None);
    }

    #[test]
    fn all_commands_survive_round_trip() {
        // Every command we can build must produce text that parses back to itself.
        let id = CanId::new(0x7E0).unwrap();
        let cmds: Vec<Command> = vec![
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
            AtCommand::SetHeader(id).into(),
            AtCommand::ReceiveFilter(id.response_id().unwrap()).into(),
            AtCommand::ClearReceiveFilter.into(),
            AtCommand::CanAutoFormatOn.into(),
            AtCommand::Timeout(0xFF).into(),
            AtCommand::ReadVoltage.into(),
            AtCommand::Identify.into(),
            AtCommand::DescribeProtocol.into(),
            DiagRequest::Obd(ObdRequest::CurrentData { pid: 0x0C }).into(),
            DiagRequest::Obd(ObdRequest::StoredDtcs).into(),
            DiagRequest::Obd(ObdRequest::PendingDtcs).into(),
            DiagRequest::Obd(ObdRequest::VehicleInfo { pid: 0x02 }).into(),
            DiagRequest::Obd(ObdRequest::PermanentDtcs).into(),
            DiagRequest::Uds(UdsRequest::ReadDtcInformation(DtcReport::NumberByStatusMask(0xFF))).into(),
            DiagRequest::Uds(UdsRequest::ReadDtcInformation(DtcReport::ByStatusMask(0xFF))).into(),
            DiagRequest::Uds(UdsRequest::ReadDtcInformation(DtcReport::Supported)).into(),
            DiagRequest::Uds(UdsRequest::ReadDataByIdentifier { did: 0xDD01 }).into(),
            DiagRequest::Uds(UdsRequest::TesterPresent).into(),
        ];
        for c in cmds {
            let wire = checked_wire(&c).expect("should be accepted");
            assert!(!wire.contains('\r') && !wire.contains('\n'));
        }
    }
}
