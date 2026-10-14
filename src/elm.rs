//! Høgnivå-API mot ELM327: init, versjon, protokoll, spenning og
//! diagnoseførespurnader. Tolkar dei rå svara frå transportlaget.

use crate::command::{AtCommand, Command, DiagRequest, Protocol, ProtocolBConfig};
use crate::transport::{RawLog, SerialTransport, Transport, TransportError};
use anyhow::{Result, anyhow, bail};
use std::time::Duration;

/// Kva CAN-buss vi snakkar med.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Bus {
    /// HS-CAN, 500 kbit/s (motorstyring o.a.)
    Hs,
    /// MS-CAN, 125 kbit/s (komfortmodular; krev at brytaren på kabelen står på MS)
    Ms,
}

/// Feilmeldingar ELM327 kan svare med i staden for data.
const ELM_ERRORS: &[&str] = &[
    "?",
    "CAN ERROR",
    "BUS ERROR",
    "BUS BUSY",
    "BUS INIT",
    "UNABLE TO CONNECT",
    "STOPPED",
    "BUFFER FULL",
    "DATA ERROR",
    "FB ERROR",
    "<RX ERROR",
    "LV RESET",
    "ACT ALERT",
    "ERROR",
];

/// Svar på ein diagnoseførespurnad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagReply {
    /// Ei eller fleire svarlinjer, t.d. `7E8 06 41 00 BE 3F A8 13`.
    Data(Vec<String>),
    /// `NO DATA` – ingen modul svarte.
    NoData,
}

pub struct Elm {
    t: Box<dyn Transport>,
}

impl Elm {
    pub fn new(t: Box<dyn Transport>) -> Self {
        Elm { t }
    }

    pub fn describe(&self) -> String {
        self.t.describe()
    }

    /// Send ein kommando og returner reinska svarlinjer.
    /// ELM-feilmeldingar (t.d. `?`, `CAN ERROR`) blir gjort om til `Err`.
    pub fn send(&mut self, cmd: Command) -> Result<Vec<String>> {
        let raw = self.t.exchange(&cmd, timeout_for(&cmd))?;
        let lines = clean_response(&raw, &cmd.to_wire());
        if let Some(err) = lines.iter().find(|l| ELM_ERRORS.contains(&l.as_str())) {
            bail!("ELM327 svarte «{err}» på {}", cmd.to_wire());
        }
        Ok(lines)
    }

    pub fn at(&mut self, at: AtCommand) -> Result<Vec<String>> {
        self.send(Command::At(at))
    }

    /// Send ein AT-kommando som skal svare «OK».
    fn at_ok(&mut self, at: AtCommand) -> Result<()> {
        let lines = self.at(at)?;
        if lines.iter().any(|l| l == "OK") {
            Ok(())
        } else {
            bail!("venta «OK» på {}, fekk {:?}", at.to_wire(), lines)
        }
    }

    /// Grunnleggjande init: `ATZ`, `ATE0`, `ATL0`. Sender ingenting på
    /// CAN-bussen, så bilen blir ikkje vekt. Returnerer versjonsstrengen frå ATZ.
    pub fn reset(&mut self) -> Result<String> {
        let lines = self.at(AtCommand::Reset)?;
        let version = lines.iter().find(|l| l.contains("ELM")).cloned().unwrap_or_default();
        self.at_ok(AtCommand::EchoOff)?;
        self.at_ok(AtCommand::LinefeedsOff)?;
        Ok(version)
    }

    /// Full init for diagnose: reset + `ATS1`, `ATH1` og protokoll.
    ///
    /// * HS: `ATSP6`
    /// * MS: `ATPB 81 04` + `ATSPB`
    ///
    /// Sjølve protokollvalet sender heller ikkje noko på bussen; det skjer
    /// først ved første diagnoseførespurnad.
    pub fn init(&mut self, bus: Bus) -> Result<String> {
        let version = self.reset()?;
        self.at_ok(AtCommand::Spaces(true))?;
        self.at_ok(AtCommand::Headers(true))?;
        match bus {
            Bus::Hs => self.at_ok(AtCommand::SetProtocol(Protocol::HsCan500k))?,
            Bus::Ms => {
                self.at_ok(AtCommand::ProtocolB(ProtocolBConfig::MsCan125k))
                    .map_err(|e| {
                        anyhow!("{e:#}\n(Kabelen støttar kanskje ikkje ATPB; då er det truleg ein enkel klon.)")
                    })?;
                self.at_ok(AtCommand::SetProtocol(Protocol::UserB))?;
            }
        }
        Ok(version)
    }

    pub fn identify(&mut self) -> Result<String> {
        Ok(self.at(AtCommand::Identify)?.join(" "))
    }

    pub fn describe_protocol(&mut self) -> Result<String> {
        Ok(self.at(AtCommand::DescribeProtocol)?.join(" "))
    }

    /// Les spenning på OBD-kontakten (V).
    pub fn voltage(&mut self) -> Result<f32> {
        let lines = self.at(AtCommand::ReadVoltage)?;
        let line = lines.first().ok_or_else(|| anyhow!("tomt svar på ATRV"))?;
        parse_voltage(line)
    }

    /// Send ein diagnoseførespurnad.
    pub fn diag(&mut self, req: DiagRequest) -> Result<DiagReply> {
        let lines = self.send(Command::Diag(req))?;
        let lines: Vec<String> = lines.into_iter().filter(|l| !l.starts_with("SEARCHING")).collect();
        if lines.iter().any(|l| l == "NO DATA") {
            return Ok(DiagReply::NoData);
        }
        Ok(DiagReply::Data(lines))
    }
}

/// Kor lenge vi ventar på svar for ulike kommandoar.
fn timeout_for(cmd: &Command) -> Duration {
    match cmd {
        Command::At(AtCommand::Reset) => Duration::from_secs(3),
        Command::At(_) => Duration::from_secs(2),
        // Første førespurnad etter protokollval kan ta tid.
        Command::Diag(_) => Duration::from_secs(6),
    }
}

/// Del opp eit rått svar i linjer, fjern tomme linjer og eventuelt ekko av
/// kommandoen vi sende (ekko er på fram til `ATE0`).
pub fn clean_response(raw: &str, sent_wire: &str) -> Vec<String> {
    raw.split(['\r', '\n'])
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter(|l| !l.replace(' ', "").eq_ignore_ascii_case(sent_wire))
        .map(|l| l.to_string())
        .collect()
}

/// Tolk «12.6V» (eller «12.6 V») til 12.6.
pub fn parse_voltage(line: &str) -> Result<f32> {
    let s = line.trim().trim_end_matches(['V', 'v']).trim();
    let v: f32 = s
        .parse()
        .map_err(|_| anyhow!("klarte ikkje tolke spenning frå «{line}»"))?;
    if !(0.0..=30.0).contains(&v) {
        bail!("urimeleg spenning {v} V frå «{line}»");
    }
    Ok(v)
}

/// Opne seriellporten. Prøver 38400 baud, så 115200, om `baud` ikkje er gitt.
/// Testar med `ATI`, som berre spør brikka om versjon.
pub fn open_serial(path: &str, baud: Option<u32>, log: &RawLog) -> Result<Box<dyn Transport>> {
    let candidates = match baud {
        Some(b) => vec![b],
        None => vec![38400, 115200],
    };
    let probe = Command::At(AtCommand::Identify);
    let mut last_err = None;
    for b in candidates {
        log.write("info", &format!("prøver {path} @ {b} baud"));
        let mut port = SerialTransport::open(path, b)?;
        // Første forsøk kan få «?» om det låg rusk i ELM-bufferen; prøv to gonger.
        for _ in 0..2 {
            match port.exchange(&probe, Duration::from_millis(1500)) {
                Ok(resp) if resp.contains("ELM") => {
                    log.write("info", &format!("ELM327 svarte @ {b} baud: {resp:?}"));
                    return Ok(Box::new(port));
                }
                Ok(resp) => {
                    log.write("info", &format!("uventa svar @ {b}: {resp:?}"));
                    last_err = Some(anyhow!("uventa svar: {resp:?}"));
                }
                Err(e) => {
                    let timeout = e.downcast_ref::<TransportError>().is_some();
                    log.write("info", &format!("ingen svar @ {b}: {e:#}"));
                    last_err = Some(e);
                    if timeout {
                        break;
                    }
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("ingen svar")).context(format!(
        "fann ingen ELM327 på {path}. Sjekk at kabelen er kopla til bilen og PC-en, og at FORScan er lukka."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::ObdRequest;
    use crate::transport::MockTransport;

    #[test]
    fn reinsk_svar_fjernar_ekko_og_tomme_linjer() {
        assert_eq!(clean_response("ATZ\r\r\rELM327 v1.5\r\r", "ATZ"), vec!["ELM327 v1.5"]);
        assert_eq!(clean_response("12.4V\r\r", "ATRV"), vec!["12.4V"]);
        assert_eq!(
            clean_response("SEARCHING...\r7E8 06 41 00 BE 3F A8 13\r\r", "0100"),
            vec!["SEARCHING...", "7E8 06 41 00 BE 3F A8 13"]
        );
    }

    #[test]
    fn tolk_spenning() {
        assert_eq!(parse_voltage("12.6V").unwrap(), 12.6);
        assert_eq!(parse_voltage(" 9.8 V ").unwrap(), 9.8);
        assert!(parse_voltage("?").is_err());
        assert!(parse_voltage("99V").is_err());
    }

    #[test]
    fn init_hs_sender_rett_sekvens() {
        let mock = MockTransport::elm_defaults();
        let sent = mock.sent_handle();
        let mut elm = Elm::new(Box::new(mock));
        assert_eq!(elm.init(Bus::Hs).unwrap(), "ELM327 v1.5");
        assert_eq!(*sent.borrow(), vec!["ATZ", "ATE0", "ATL0", "ATS1", "ATH1", "ATSP6"]);
    }

    #[test]
    fn init_ms_set_opp_protokoll_b() {
        let mock = MockTransport::elm_defaults();
        let sent = mock.sent_handle();
        let mut elm = Elm::new(Box::new(mock));
        elm.init(Bus::Ms).unwrap();
        assert_eq!(
            *sent.borrow(),
            vec!["ATZ", "ATE0", "ATL0", "ATS1", "ATH1", "ATPB8104", "ATSPB"]
        );
    }

    #[test]
    fn elm_feil_blir_err() {
        let mock = MockTransport::elm_defaults().set("ATRV", "?\r\r");
        let mut elm = Elm::new(Box::new(mock));
        assert!(elm.voltage().is_err());
    }

    #[test]
    fn diag_no_data_og_data() {
        let mock = MockTransport::elm_defaults()
            .on("0100", "SEARCHING...\rNO DATA\r\r")
            .on("0100", "7E8 06 41 00 BE 3F A8 13\r\r");
        let mut elm = Elm::new(Box::new(mock));
        let req = DiagRequest::Obd(ObdRequest::CurrentData { pid: 0 });
        assert_eq!(elm.diag(req).unwrap(), DiagReply::NoData);
        assert_eq!(
            elm.diag(req).unwrap(),
            DiagReply::Data(vec!["7E8 06 41 00 BE 3F A8 13".into()])
        );
    }
}
