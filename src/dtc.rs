//! Phase 3: reading diagnostic trouble codes (DTCs).
//!
//! * Standard OBD-II modes `03`, `07` and `0A` (2-byte DTCs).
//! * UDS `19 02 FF` (ReadDTCInformation, report by status mask) per module:
//!   3-byte DTCs plus a status byte.
//!
//! Module addresses are **unverified** candidates. A module counts as verified on
//! this car only once it has actually responded. Code meanings are not included:
//! look them up in FORScan or Ford documentation. Failure type descriptions are
//! generic SAE J2012 **suggestions** and must be checked against Ford documentation.

use crate::command::{CanId, DiagRequest, DtcReport, ObdRequest, UdsRequest};
use crate::elm::{DiagReply, Elm};
use crate::isotp;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Candidate modules
// ---------------------------------------------------------------------------

/// A candidate module. **All addresses are unverified for this car**; they are
/// common Ford addresses seen on other models. The bus is only a guess.
pub struct Module {
    pub name: &'static str,
    pub description: &'static str,
    /// Request CAN ID. The response is expected on request + 8.
    pub request: u16,
    /// Guessed bus: "HS", "MS" or "?".
    pub likely_bus: &'static str,
}

pub const CANDIDATES: &[Module] = &[
    Module {
        name: "PCM",
        description: "Powertrain control module (engine)",
        request: 0x7E0,
        likely_bus: "HS",
    },
    Module {
        name: "TCM",
        description: "Transmission control module (PowerShift)",
        request: 0x7E1,
        likely_bus: "HS",
    },
    Module {
        name: "ABS",
        description: "Anti-lock brakes / stability control",
        request: 0x760,
        likely_bus: "HS",
    },
    Module {
        name: "RCM",
        description: "Restraints control module (airbags)",
        request: 0x737,
        likely_bus: "HS",
    },
    Module {
        name: "PSCM",
        description: "Power steering control module",
        request: 0x730,
        likely_bus: "HS",
    },
    Module {
        name: "IPC",
        description: "Instrument panel cluster",
        request: 0x720,
        likely_bus: "?",
    },
    Module {
        name: "BCM",
        description: "Body control module (incl. battery monitoring)",
        request: 0x726,
        likely_bus: "?",
    },
    Module {
        name: "ACM",
        description: "Audio control module (radio)",
        request: 0x727,
        likely_bus: "MS",
    },
    Module {
        name: "APIM",
        description: "SYNC (accessory protocol interface module)",
        request: 0x7D0,
        likely_bus: "MS",
    },
    Module {
        name: "HVAC",
        description: "Climate control",
        request: 0x733,
        likely_bus: "?",
    },
    Module {
        name: "GWM",
        description: "Gateway module (may not exist on this car)",
        request: 0x716,
        likely_bus: "?",
    },
    Module {
        name: "DDM",
        description: "Driver door module",
        request: 0x740,
        likely_bus: "MS",
    },
    Module {
        name: "PDM",
        description: "Passenger door module",
        request: 0x741,
        likely_bus: "MS",
    },
    Module {
        name: "PAM",
        description: "Parking aid module",
        request: 0x736,
        likely_bus: "?",
    },
];

/// Find a candidate by name (case-insensitive).
pub fn find_module(name: &str) -> Option<&'static Module> {
    CANDIDATES.iter().find(|m| m.name.eq_ignore_ascii_case(name))
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// One DTC. For UDS DTCs, `ftb` is the failure type byte and `status` the
/// status byte; standard OBD-II DTCs have neither.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dtc {
    pub code: String,
    pub ftb: Option<u8>,
    pub status: Option<u8>,
}

impl Dtc {
    /// E.g. "P0563" or "U3003:16".
    pub fn display(&self) -> String {
        match self.ftb {
            Some(f) => format!("{}:{f:02X}", self.code),
            None => self.code.clone(),
        }
    }
}

/// Decode the 2-byte DTC base: the top two bits give the letter (P/C/B/U),
/// the remaining 14 bits the four digits (SAE J2012).
pub fn decode_code(hi: u8, lo: u8) -> String {
    let letter = ['P', 'C', 'B', 'U'][(hi >> 6) as usize];
    format!("{letter}{}{:X}{:X}{:X}", (hi >> 4) & 0x3, hi & 0xF, lo >> 4, lo & 0xF)
}

/// Rough category of a code from its first two characters (SAE J2012).
/// A suggestion, not a definition.
pub fn code_category(code: &str) -> &'static str {
    match code.get(0..2) {
        Some("P0" | "P2" | "C0" | "B0" | "U0" | "C3" | "B3" | "U3") => "SAE-defined",
        Some("P1" | "C1" | "C2" | "B1" | "B2" | "U1" | "U2") => "manufacturer-specific",
        _ => "mixed / see documentation",
    }
}

/// Parse a positive response to OBD mode 03, 07 or 0A on CAN:
/// `[service + 0x40, count, hi, lo, hi, lo, ...]`.
pub fn parse_obd_dtcs(data: &[u8]) -> Result<Vec<Dtc>, String> {
    if data.len() < 2 {
        return Err("response too short".into());
    }
    let count = data[1] as usize;
    let pairs = &data[2..];
    if pairs.len() < count * 2 {
        return Err(format!("expected {count} DTCs, got {} bytes", pairs.len()));
    }
    Ok(pairs[..count * 2]
        .chunks(2)
        .map(|c| Dtc {
            code: decode_code(c[0], c[1]),
            ftb: None,
            status: None,
        })
        .collect())
}

/// Parse a positive response to UDS `19 02 mm`:
/// `[0x59, 0x02, availability mask, (hi, mid, ftb, status) * n]`.
pub fn parse_uds_dtcs(data: &[u8]) -> Result<Vec<Dtc>, String> {
    if data.len() < 3 || data[0] != 0x59 || data[1] != 0x02 {
        return Err("not a 59 02 response".into());
    }
    let records = &data[3..];
    if records.len() % 4 != 0 {
        return Err(format!("{} record bytes is not a multiple of 4", records.len()));
    }
    Ok(records
        .chunks(4)
        .map(|r| Dtc {
            code: decode_code(r[0], r[1]),
            ftb: Some(r[2]),
            status: Some(r[3]),
        })
        .collect())
}

/// The UDS DTC status bits (ISO 14229-1), from bit 0 to bit 7.
const STATUS_BITS: [&str; 8] = [
    "test failed now",
    "failed this operation cycle",
    "pending",
    "confirmed",
    "not completed since clear",
    "failed since clear",
    "not completed this cycle",
    "warning light requested",
];

/// The names of the bits that are set in `status`.
pub fn status_text(status: u8) -> Vec<&'static str> {
    (0..8)
        .filter(|b| status & (1 << b) != 0)
        .map(|b| STATUS_BITS[b])
        .collect()
}

/// Names of UDS negative response codes (ISO 14229-1).
pub fn nrc_name(nrc: u8) -> &'static str {
    match nrc {
        0x10 => "general reject",
        0x11 => "service not supported",
        0x12 => "sub-function not supported",
        0x13 => "incorrect message length or format",
        0x14 => "response too long",
        0x21 => "busy, repeat request",
        0x22 => "conditions not correct",
        0x31 => "request out of range",
        0x33 => "security access denied",
        0x78 => "response pending",
        0x7E => "sub-function not supported in active session",
        0x7F => "service not supported in active session",
        _ => "unknown negative response code",
    }
}

/// Generic SAE J2012 failure type byte descriptions. **Suggestions only** – the
/// exact meaning for a given Ford DTC must be checked against Ford documentation.
pub fn ftb_suggestion(ftb: u8) -> Option<&'static str> {
    Some(match ftb {
        0x00 => "no sub-type information",
        0x11 => "circuit short to ground",
        0x12 => "circuit short to battery",
        0x13 => "circuit open",
        0x16 => "circuit voltage below threshold",
        0x17 => "circuit voltage above threshold",
        0x1C => "circuit voltage out of range",
        0x29 => "signal invalid",
        0x49 => "internal electronic failure",
        0x4B => "over temperature",
        0x55 => "not configured",
        0x62 => "signal compare failure",
        0x64 => "signal plausibility failure",
        0x68 => "event information",
        0x87 => "missing message",
        0x88 => "bus off",
        0x96 => "component internal failure",
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

/// What came back from one read request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// `NO DATA`: nothing answered on this address.
    NoResponse,
    /// Positive response (possibly with zero DTCs).
    Dtcs { dtcs: Vec<Dtc> },
    /// The module answered with a negative response code.
    Negative { nrc: u8, meaning: String },
    /// Anything else (ELM error, malformed response …).
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Read {
    pub label: String,
    /// The request as sent, e.g. "1902FF".
    pub request: String,
    pub outcome: Outcome,
    /// Raw response lines, for the log and the report.
    pub raw: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModuleResult {
    pub name: String,
    pub description: String,
    pub request_id: String,
    pub response_id: String,
    pub reads: Vec<Read>,
}

impl ModuleResult {
    /// True if the module answered anything at all (positive or negative). This
    /// is what makes an address "verified" on the bus that was scanned.
    pub fn responded(&self) -> bool {
        self.reads
            .iter()
            .any(|r| matches!(r.outcome, Outcome::Dtcs { .. } | Outcome::Negative { .. }))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DtcScan {
    pub started: chrono::DateTime<chrono::Local>,
    pub source: String,
    /// "hs" or "ms".
    pub bus: String,
    pub modules: Vec<ModuleResult>,
    /// File name of the raw log for this run (in `<data-dir>/logs/`).
    #[serde(default)]
    pub raw_log: Option<String>,
}

impl DtcScan {
    /// Save as `<dir>/dtc-<bus>-YYYYMMDD-HHMMSS.json`.
    pub fn save(&self, dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let name = format!("dtc-{}-{}.json", self.bus, self.started.format("%Y%m%d-%H%M%S"));
        let path = dir.join(name);
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(path)
    }

    /// Find the latest saved scan for each bus in `dir` (HS first, then MS).
    pub fn load_latest(dir: &Path) -> Result<Vec<DtcScan>> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(Vec::new());
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("dtc-") && n.ends_with(".json"))
            })
            .collect();
        // File names contain bus and timestamp, so the last one per bus is the latest.
        files.sort();
        let mut out = Vec::new();
        for bus in ["hs", "ms"] {
            let prefix = format!("dtc-{bus}-");
            let latest = files.iter().rfind(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&prefix))
            });
            if let Some(path) = latest {
                out.push(serde_json::from_str(&std::fs::read_to_string(path)?)?);
            }
        }
        Ok(out)
    }
}

/// The kind of response we expect, used to parse a positive answer.
#[derive(Clone, Copy)]
enum Expect {
    Obd,
    Uds,
}

/// Send one read request to the currently targeted module and interpret the answer.
fn read_one(elm: &mut Elm, label: &str, req: DiagRequest, response_id: CanId, expect: Expect) -> Read {
    let wire = req.to_wire();
    let service = req.to_bytes()[0];
    let make = |outcome, raw| Read {
        label: label.to_string(),
        request: wire.clone(),
        outcome,
        raw,
    };

    let lines = match elm.diag(req) {
        Ok(DiagReply::NoData) => return make(Outcome::NoResponse, vec![]),
        Ok(DiagReply::Data(lines)) => lines,
        Err(e) => {
            return make(
                Outcome::Error {
                    message: format!("{e:#}"),
                },
                vec![],
            );
        }
    };

    let messages = match isotp::reassemble(&lines) {
        Ok(m) => m,
        Err(e) => return make(Outcome::Error { message: e.to_string() }, lines),
    };

    // Only messages from the module we asked (the receive filter should already
    // ensure this), and skip "response pending" (7F xx 78) if a real answer follows.
    let ours: Vec<&isotp::Message> = messages.iter().filter(|m| m.id == response_id.value()).collect();
    let is_pending = |m: &&isotp::Message| m.data.len() >= 3 && m.data[0] == 0x7F && m.data[2] == 0x78;
    let Some(msg) = ours.iter().rev().find(|m| !is_pending(m)).or(ours.last()) else {
        return make(Outcome::NoResponse, lines);
    };
    let d = &msg.data;

    let outcome = if d.len() >= 3 && d[0] == 0x7F {
        if d[2] == 0x78 {
            Outcome::Error {
                message: "module said \"response pending\" but no final answer arrived; try again".into(),
            }
        } else {
            Outcome::Negative {
                nrc: d[2],
                meaning: nrc_name(d[2]).to_string(),
            }
        }
    } else if d.first() == Some(&(service + 0x40)) {
        let parsed = match expect {
            Expect::Obd => parse_obd_dtcs(d),
            Expect::Uds => parse_uds_dtcs(d),
        };
        match parsed {
            Ok(dtcs) => Outcome::Dtcs { dtcs },
            Err(message) => Outcome::Error { message },
        }
    } else {
        Outcome::Error {
            message: format!("unexpected response {:02X?}", d),
        }
    };
    make(outcome, lines)
}

/// Read DTCs from one module. With `include_obd`, also run OBD modes 03/07/0A
/// (only meaningful for the PCM).
pub fn scan_module(elm: &mut Elm, module: &Module, include_obd: bool) -> Result<ModuleResult> {
    let request = CanId::new(module.request)?;
    let response = request
        .response_id()
        .ok_or_else(|| anyhow!("{} has no physical response address", module.name))?;

    let mut result = ModuleResult {
        name: module.name.to_string(),
        description: module.description.to_string(),
        request_id: request.to_string(),
        response_id: response.to_string(),
        reads: Vec::new(),
    };

    if let Err(e) = elm.target(request) {
        result.reads.push(Read {
            label: "set target".into(),
            request: format!("ATSH{request} / ATCRA{response}"),
            outcome: Outcome::Error {
                message: format!("{e:#}"),
            },
            raw: vec![],
        });
        return Ok(result);
    }

    if include_obd {
        for (label, r) in [
            ("OBD stored (03)", ObdRequest::StoredDtcs),
            ("OBD pending (07)", ObdRequest::PendingDtcs),
            ("OBD permanent (0A)", ObdRequest::PermanentDtcs),
        ] {
            let read = read_one(elm, label, DiagRequest::Obd(r), response, Expect::Obd);
            result.reads.push(read);
        }
    }

    let uds = DiagRequest::Uds(UdsRequest::ReadDtcInformation(DtcReport::ByStatusMask(0xFF)));
    let read = read_one(elm, "UDS 19 02 FF", uds, response, Expect::Uds);
    result.reads.push(read);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elm::Bus;
    use crate::transport::MockTransport;

    #[test]
    fn decode_codes() {
        assert_eq!(decode_code(0x01, 0x33), "P0133");
        assert_eq!(decode_code(0x05, 0x63), "P0563");
        assert_eq!(decode_code(0x41, 0x23), "C0123");
        assert_eq!(decode_code(0x92, 0x00), "B1200");
        assert_eq!(decode_code(0xF0, 0x03), "U3003");
        assert_eq!(decode_code(0xC1, 0x00), "U0100");
    }

    #[test]
    fn categories() {
        assert_eq!(code_category("P0563"), "SAE-defined");
        assert_eq!(code_category("B1317"), "manufacturer-specific");
    }

    #[test]
    fn obd_response() {
        assert_eq!(parse_obd_dtcs(&[0x43, 0x00]).unwrap(), vec![]);
        let d = parse_obd_dtcs(&[0x47, 0x02, 0x05, 0x63, 0xC1, 0x00]).unwrap();
        assert_eq!(d.iter().map(|x| x.display()).collect::<Vec<_>>(), ["P0563", "U0100"]);
        assert!(parse_obd_dtcs(&[0x43, 0x02, 0x05]).is_err());
    }

    #[test]
    fn uds_response() {
        let data = [0x59, 0x02, 0xFF, 0xF0, 0x03, 0x16, 0x08, 0x92, 0x00, 0x00, 0x28];
        let d = parse_uds_dtcs(&data).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].display(), "U3003:16");
        assert_eq!(d[0].status, Some(0x08));
        assert_eq!(d[1].display(), "B1200:00");
        assert!(parse_uds_dtcs(&[0x59, 0x02, 0xFF, 0x01]).is_err());
        assert!(parse_uds_dtcs(&[0x59, 0x02, 0xFF]).unwrap().is_empty());
    }

    #[test]
    fn status_bits() {
        assert_eq!(status_text(0x08), vec!["confirmed"]);
        assert_eq!(status_text(0x2C), vec!["pending", "confirmed", "failed since clear"]);
        assert!(status_text(0x00).is_empty());
    }

    #[test]
    fn all_candidates_are_valid_ids() {
        for m in CANDIDATES {
            let id = CanId::new(m.request).expect(m.name);
            assert!(id.response_id().is_some(), "{}", m.name);
        }
    }

    fn mock_elm(mock: MockTransport) -> Elm {
        let mut elm = Elm::new(Box::new(mock));
        elm.init(Bus::Ms).unwrap();
        elm
    }

    #[test]
    fn scan_multi_frame_uds() {
        let mock = MockTransport::elm_defaults().on_target(
            "726",
            "1902FF",
            "72E 10 0F 59 02 FF F0 03 16\r72E 21 08 92 00 00 28 91 17\r72E 22 62 28 AA AA AA AA AA\r\r",
        );
        let mut elm = mock_elm(mock);
        let r = scan_module(&mut elm, find_module("BCM").unwrap(), false).unwrap();
        assert!(r.responded());
        let Outcome::Dtcs { dtcs } = &r.reads[0].outcome else {
            panic!("{:?}", r.reads[0].outcome)
        };
        let shown: Vec<String> = dtcs.iter().map(|d| d.display()).collect();
        assert_eq!(shown, ["U3003:16", "B1200:00", "B1117:62"]);
    }

    #[test]
    fn scan_no_response_and_negative() {
        let mock = MockTransport::elm_defaults()
            .with_diag_default("NO DATA\r\r")
            .on_target("727", "1902FF", "72F 03 7F 19 31\r\r");
        let mut elm = mock_elm(mock);
        let none = scan_module(&mut elm, find_module("APIM").unwrap(), false).unwrap();
        assert!(!none.responded());
        assert_eq!(none.reads[0].outcome, Outcome::NoResponse);
        let neg = scan_module(&mut elm, find_module("ACM").unwrap(), false).unwrap();
        assert!(neg.responded());
        assert!(matches!(neg.reads[0].outcome, Outcome::Negative { nrc: 0x31, .. }));
    }

    #[test]
    fn scan_pending_then_answer() {
        let mock = MockTransport::elm_defaults().on_target("726", "1902FF", "72E 03 7F 19 78\r72E 03 59 02 FF\r\r");
        let mut elm = mock_elm(mock);
        let r = scan_module(&mut elm, find_module("BCM").unwrap(), false).unwrap();
        assert_eq!(r.reads[0].outcome, Outcome::Dtcs { dtcs: vec![] });
    }

    #[test]
    fn scan_pcm_with_obd() {
        let mock = MockTransport::elm_defaults()
            .on_target("7E0", "03", "7E8 02 43 00\r\r")
            .on_target("7E0", "07", "7E8 04 47 01 05 63\r\r")
            .on_target("7E0", "0A", "7E8 02 4A 00\r\r")
            .on_target("7E0", "1902FF", "7E8 03 59 02 FF\r\r");
        let sent = mock.sent_handle();
        let mut elm = mock_elm(mock);
        let r = scan_module(&mut elm, find_module("PCM").unwrap(), true).unwrap();
        assert_eq!(r.reads.len(), 4);
        assert_eq!(
            r.reads[1].outcome,
            Outcome::Dtcs {
                dtcs: vec![Dtc {
                    code: "P0563".into(),
                    ftb: None,
                    status: None
                }]
            }
        );
        // Header and receive filter are set before the requests.
        let sent = sent.borrow();
        let i = sent.iter().position(|w| w == "ATSH7E0").unwrap();
        assert_eq!(sent[i + 1], "ATCRA7E8");
    }
}
