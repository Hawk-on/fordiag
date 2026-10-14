//! Tillatsliste for alt som kan sendast til ELM327-brikka og til bilen.
//!
//! Tryggleiksprinsippet er at **forbodne kommandoar ikkje kan konstruerast**:
//!
//! * Alt som går ut på kabelen er ein [`Command`]. Han er anten ein
//!   [`AtCommand`] (til sjølve ELM327-brikka) eller ein [`DiagRequest`]
//!   (diagnoseførespurnad til ein modul i bilen).
//! * Begge er `enum`-ar der kvar variant er ei lovleg leseoperasjon. Det finst
//!   ingen variant for «rå bytes», så t.d. `04` (slett feilkodar) eller UDS
//!   `2E` (skriv) kan ikkje uttrykkjast i det heile.
//! * Treng vi å skrive ein kommando som tekst (feilsøking), går han gjennom
//!   [`Command::parse`], som tolkar teksten til ein av variantane over –
//!   eller returnerer [`Forbidden`] for alt anna.
//! * CAN-ID-ar er ein newtype ([`CanId`]) med privat felt, så dei må gå
//!   gjennom ein validerande konstruktør.
//!
//! Transportlaget (`transport.rs`) tek berre imot `&Command`, og sjekkar i
//! tillegg at teksten som faktisk blir sendt tolkar tilbake til same kommando.

use std::fmt;
use thiserror::Error;

/// Grunnen til at ein kommando vart avvist.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Forbidden {
    #[error("tom kommando")]
    Empty,
    #[error("ugyldig hex: «{0}»")]
    BadHex(String),
    #[error("teneste 0x{0:02X} er FORBODEN ({1}) – bruk FORScan om dette verkeleg trengst")]
    ForbiddenService(u8, &'static str),
    #[error("teneste 0x{0:02X} står ikkje på tillatslista")]
    NotAllowed(u8),
    #[error("feil format for teneste 0x{service:02X}: {reason}")]
    BadFormat { service: u8, reason: &'static str },
    #[error("AT-kommandoen «{0}» står ikkje på tillatslista")]
    AtNotAllowed(String),
    #[error("CAN-ID 0x{0:X} er utanfor diagnoseområdet 0x700–0x7FF")]
    CanIdOutOfRange(u16),
}

// ---------------------------------------------------------------------------
// CAN-ID
// ---------------------------------------------------------------------------

/// Ein 11-bits CAN-ID i diagnoseområdet 0x700–0x7FF.
///
/// Vi avgrensar til dette området så verktøyet ikkje kan brukast til å sende
/// meldingar på ID-ar som vanlege modular brukar til styring under køyring.
/// Feltet er privat: einaste vegen inn er [`CanId::new`].
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

    #[allow(dead_code)] // blir brukt i fase 3 (feilkodar)
    pub fn value(self) -> u16 {
        self.0
    }

    /// Vanleg svaradresse for fysisk adressering: førespurnad + 8.
    /// Gjeld ikkje funksjonell adresse 0x7DF, og 0x7F8–0x7FF har inga +8.
    #[allow(dead_code)] // blir brukt i fase 3 (feilkodar)
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
// AT-kommandoar (til ELM327-brikka)
// ---------------------------------------------------------------------------

/// Protokollar vi tillet med `ATSP`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// `ATSP0` – automatisk søk.
    Auto,
    /// `ATSP6` – ISO 15765-4 CAN, 11-bit ID, 500 kbit/s (Ford HS-CAN).
    HsCan500k,
    /// `ATSPB` – «USER1»-protokollen, sett opp med `ATPB` (brukt for MS-CAN).
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

/// Lovlege oppsett for protokoll B (`ATPB xx yy`).
///
/// Berre faste, kjende oppsett er tillatne, fordi feil bitrate på ein CAN-buss
/// kan forstyrre trafikken der.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolBConfig {
    /// `ATPB 81 04` – Ford MS-CAN, 125 kbit/s. Ifølgje ELM327-databladet
    /// (PP 2C/2D): 0x81 = 11-bit ID, fast 8-byte lengd, ISO 15765-4-format;
    /// 0x04 = 500/4 = 125 kbit/s.
    MsCan125k,
}

impl ProtocolBConfig {
    fn bytes(self) -> (u8, u8) {
        match self {
            ProtocolBConfig::MsCan125k => (0x81, 0x04),
        }
    }
}

/// Alle AT-kommandoar verktøyet kan sende til ELM327-brikka.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtCommand {
    /// `ATZ` – nullstill brikka.
    Reset,
    /// `ATE0` – slå av ekko.
    EchoOff,
    /// `ATL0` – slå av linjeskift etter CR.
    LinefeedsOff,
    /// `ATS0`/`ATS1` – mellomrom i svaret av/på.
    Spaces(bool),
    /// `ATH0`/`ATH1` – vis CAN-ID (header) i svaret av/på.
    Headers(bool),
    /// `ATSPx` – vel protokoll.
    SetProtocol(Protocol),
    /// `ATPB xx yy` – set opp protokoll B.
    ProtocolB(ProtocolBConfig),
    /// `ATSH xxx` – CAN-ID som førespurnader blir sende til.
    SetHeader(CanId),
    /// `ATCRA xxx` – berre ta imot svar frå denne CAN-ID-en.
    ReceiveFilter(CanId),
    /// `ATCRA` utan argument – fjern mottaksfilteret.
    ClearReceiveFilter,
    /// `ATCAF1` – automatisk ISO 15765-formatering på (ELM legg til PCI-byte).
    /// `ATCAF0` er med vilje ikkje med: då kunne ein sende vilkårlege CAN-rammer.
    CanAutoFormatOn,
    /// `ATST xx` – svartidsavbrot, xx × 4 ms.
    Timeout(u8),
    /// `ATRV` – les spenninga på OBD-kontakten (pinne 16).
    ReadVoltage,
    /// `ATI` – versjonsstreng.
    Identify,
    /// `ATDP` – skildring av aktiv protokoll.
    DescribeProtocol,
}

impl AtCommand {
    /// Teksten som blir send til ELM327 (utan CR).
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

    /// Tolk ein AT-kommando skriven som tekst. Mellomrom og små/store bokstavar
    /// spelar inga rolle. Alt som ikkje står på lista blir avvist.
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
                // Kommandoar med argument. Merk rekkjefølgja: «CRA» før «CR…» o.l.
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
// Diagnoseførespurnader (til modular i bilen)
// ---------------------------------------------------------------------------

/// Standard OBD-II lesemodusar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObdRequest {
    /// Modus `01 pp` – live-data for éin PID.
    CurrentData { pid: u8 },
    /// Modus `03` – lagra feilkodar.
    StoredDtcs,
    /// Modus `07` – ventande feilkodar.
    PendingDtcs,
    /// Modus `09 pp` – køyretøyinfo (VIN o.l.).
    VehicleInfo { pid: u8 },
    /// Modus `0A` – permanente feilkodar.
    PermanentDtcs,
}

/// Lovlege underfunksjonar av UDS `19` (ReadDTCInformation). Alle er lesing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtcReport {
    /// `19 01 mm` – tal på feilkodar som matchar statusmaska.
    NumberByStatusMask(u8),
    /// `19 02 mm` – feilkodar som matchar statusmaska.
    ByStatusMask(u8),
    /// `19 0A` – alle feilkodar modulen støttar.
    Supported,
}

/// Lovlege UDS-lesetenester.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdsRequest {
    /// `19 …` – ReadDTCInformation.
    ReadDtcInformation(DtcReport),
    /// `22 hh ll` – ReadDataByIdentifier, éin DID.
    ReadDataByIdentifier { did: u16 },
    /// `3E 00` – TesterPresent (med positivt svar).
    TesterPresent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagRequest {
    Obd(ObdRequest),
    Uds(UdsRequest),
}

/// Tenester som er eksplisitt forbodne, med grunngjeving. Alt som ikkje er på
/// tillatslista blir avvist uansett; denne lista gjev berre betre feilmeldingar.
const FORBIDDEN_SERVICES: &[(u8, &str)] = &[
    (0x04, "OBD: slett feilkodar"),
    (0x08, "OBD: styr system om bord"),
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
    /// Bytane i førespurnaden.
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

    /// Teksten som blir send til ELM327, t.d. `1902FF`.
    pub fn to_wire(&self) -> String {
        self.to_bytes().iter().map(|b| format!("{b:02X}")).collect()
    }

    /// Validerande konstruktør for rå hex, t.d. `"19 02 FF"`.
    ///
    /// Bytane blir tolka til ein av variantane over. Alt anna – forbodne
    /// tenester, ukjende tenester, feil lengd – gjev `Err`.
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
            (0x01, _) => return Err(bad("venta nøyaktig éin PID")),
            (0x03, []) => Obd(ObdRequest::StoredDtcs),
            (0x07, []) => Obd(ObdRequest::PendingDtcs),
            (0x0A, []) => Obd(ObdRequest::PermanentDtcs),
            (0x03 | 0x07 | 0x0A, _) => return Err(bad("denne modusen tek ingen argument")),
            (0x09, [pid]) => Obd(ObdRequest::VehicleInfo { pid: *pid }),
            (0x09, _) => return Err(bad("venta nøyaktig éin PID")),
            (0x19, [0x01, mask]) => Uds(UdsRequest::ReadDtcInformation(DtcReport::NumberByStatusMask(*mask))),
            (0x19, [0x02, mask]) => Uds(UdsRequest::ReadDtcInformation(DtcReport::ByStatusMask(*mask))),
            (0x19, [0x0A]) => Uds(UdsRequest::ReadDtcInformation(DtcReport::Supported)),
            (0x19, _) => return Err(bad("berre underfunksjon 01 mm, 02 mm og 0A er tillatne")),
            (0x22, [hi, lo]) => Uds(UdsRequest::ReadDataByIdentifier {
                did: u16::from_be_bytes([*hi, *lo]),
            }),
            (0x22, _) => return Err(bad("venta nøyaktig éin DID (2 byte)")),
            (0x3E, [0x00]) => Uds(UdsRequest::TesterPresent),
            (0x3E, _) => return Err(bad("berre «3E 00» er tillaten")),
            _ => return Err(Forbidden::NotAllowed(service)),
        };
        Ok(req)
    }
}

/// Tolk hex-tekst som «19 02 FF» eller «1902ff» til bytes.
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
// Samla kommando-type
// ---------------------------------------------------------------------------

/// Alt som kan sendast på kabelen.
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

    /// Tolk tekst til ein kommando. Tekst som byrjar med «AT» blir tolka som
    /// AT-kommando, alt anna som hex-diagnoseførespurnad.
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

/// Siste tryggleikssjekk før noko blir skrive til kabelen: teksten må tolke
/// tilbake til nøyaktig same kommando. Blir kalla av alle transportar.
pub fn checked_wire(cmd: &Command) -> Result<String, Forbidden> {
    let wire = cmd.to_wire();
    match Command::parse(&wire) {
        Ok(back) if back == *cmd => Ok(wire),
        Ok(_) => Err(Forbidden::AtNotAllowed(wire)),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Testar
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn is_forbidden(text: &str) -> bool {
        matches!(DiagRequest::from_hex(text), Err(Forbidden::ForbiddenService(..)))
    }

    #[test]
    fn forbodne_tenester_blir_avviste() {
        // Kvar forboden teneste, både åleine og med typiske argument.
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
            assert!(is_forbidden(text), "«{text}» skulle vore forboden");
            assert!(Command::parse(text).is_err());
        }
    }

    #[test]
    fn alle_tenester_utanfor_lista_blir_avviste() {
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
                    "0x{service:02X} skulle vore avvist"
                );
            }
        }
    }

    #[test]
    fn tillatne_leseforespurnader_blir_godtekne() {
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
    fn feil_format_blir_avvist() {
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
            assert!(DiagRequest::from_hex(text).is_err(), "«{text}» skulle vore avvist");
        }
    }

    #[test]
    fn at_kommandoar_tillatsliste() {
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

        // Ikkje på lista: monitorering, rå CAN, endra baudrate, kalibrering,
        // programmerbare parametrar, andre PB-oppsett, ID-ar utanfor 7xx.
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
            assert!(AtCommand::parse(text).is_err(), "«{text}» skulle vore avvist");
        }
    }

    #[test]
    fn can_id_validering() {
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
    fn alle_kommandoar_overlever_tur_retur() {
        // Kvar kommando vi kan lage skal gje tekst som tolkar tilbake til seg sjølv.
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
            let wire = checked_wire(&c).expect("skulle vore godkjend");
            assert!(!wire.contains('\r') && !wire.contains('\n'));
        }
    }
}
