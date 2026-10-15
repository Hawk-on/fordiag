//! High-level ELM327 API: init, version, protocol, voltage and diagnostic
//! requests. Interprets the raw responses from the transport layer.

use crate::command::{AtCommand, CanId, Command, DiagRequest, Protocol, ProtocolBConfig};
use crate::transport::{RawLog, SerialTransport, Transport, TransportError};
use anyhow::{Result, anyhow, bail};
use std::time::Duration;

/// Which CAN bus we talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Bus {
    /// HS-CAN, 500 kbit/s (engine control etc.)
    Hs,
    /// MS-CAN, 125 kbit/s (comfort modules; requires the cable switch set to MS)
    Ms,
}

/// Error messages the ELM327 may return instead of data.
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

/// Response to a diagnostic request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagReply {
    /// One or more response lines, e.g. `7E8 06 41 00 BE 3F A8 13`.
    Data(Vec<String>),
    /// `NO DATA` – no module responded.
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

    /// Send a command and return cleaned response lines.
    /// ELM error messages (e.g. `?`, `CAN ERROR`) are turned into `Err`.
    pub fn send(&mut self, cmd: Command) -> Result<Vec<String>> {
        let raw = self.t.exchange(&cmd, timeout_for(&cmd))?;
        let lines = clean_response(&raw, &cmd.to_wire());
        if let Some(err) = lines.iter().find(|l| ELM_ERRORS.contains(&l.as_str())) {
            bail!("ELM327 replied \"{err}\" to {}", cmd.to_wire());
        }
        Ok(lines)
    }

    pub fn at(&mut self, at: AtCommand) -> Result<Vec<String>> {
        self.send(Command::At(at))
    }

    /// Send an AT command that should reply "OK".
    fn at_ok(&mut self, at: AtCommand) -> Result<()> {
        let lines = self.at(at)?;
        if lines.iter().any(|l| l == "OK") {
            Ok(())
        } else {
            bail!("expected \"OK\" for {}, got {:?}", at.to_wire(), lines)
        }
    }

    /// Basic init: `ATZ`, `ATE0`, `ATL0`. Sends nothing on the CAN bus, so the
    /// car is not woken up. Returns the version string from ATZ.
    pub fn reset(&mut self) -> Result<String> {
        let lines = self.at(AtCommand::Reset)?;
        let version = lines.iter().find(|l| l.contains("ELM")).cloned().unwrap_or_default();
        self.at_ok(AtCommand::EchoOff)?;
        self.at_ok(AtCommand::LinefeedsOff)?;
        Ok(version)
    }

    /// Full init for diagnostics: reset + `ATS1`, `ATH1` and protocol.
    ///
    /// * HS: `ATSP6`
    /// * MS: `ATPB 81 04` + `ATSPB`
    ///
    /// Selecting the protocol does not send anything on the bus either; that
    /// only happens with the first diagnostic request.
    pub fn init(&mut self, bus: Bus) -> Result<String> {
        let version = self.reset()?;
        self.at_ok(AtCommand::Spaces(true))?;
        self.at_ok(AtCommand::Headers(true))?;
        match bus {
            Bus::Hs => self.at_ok(AtCommand::SetProtocol(Protocol::HsCan500k))?,
            Bus::Ms => {
                self.at_ok(AtCommand::ProtocolB(ProtocolBConfig::MsCan125k))
                    .map_err(|e| {
                        anyhow!("{e:#}\n(The cable may not support ATPB; it is then probably a basic clone.)")
                    })?;
                self.at_ok(AtCommand::SetProtocol(Protocol::UserB))?;
            }
        }
        Ok(version)
    }

    /// Prepare for DTC reading: automatic ISO-TP formatting on (`ATCAF1`, also
    /// the default) and a longer response timeout (`ATST64` = 400 ms) so slow
    /// modules are not reported as silent.
    pub fn prepare_for_dtc(&mut self) -> Result<()> {
        self.at_ok(AtCommand::CanAutoFormatOn)?;
        self.at_ok(AtCommand::Timeout(0x64))
    }

    /// Address one module: requests go to `request` (`ATSH`), and only
    /// responses from `request + 8` are received (`ATCRA`). Returns the
    /// response ID.
    pub fn target(&mut self, request: CanId) -> Result<CanId> {
        let response = request
            .response_id()
            .ok_or_else(|| anyhow!("{request} has no physical response address"))?;
        self.at_ok(AtCommand::SetHeader(request))?;
        self.at_ok(AtCommand::ReceiveFilter(response))?;
        Ok(response)
    }

    pub fn identify(&mut self) -> Result<String> {
        Ok(self.at(AtCommand::Identify)?.join(" "))
    }

    pub fn describe_protocol(&mut self) -> Result<String> {
        Ok(self.at(AtCommand::DescribeProtocol)?.join(" "))
    }

    /// Read the voltage on the OBD connector (V).
    pub fn voltage(&mut self) -> Result<f32> {
        let lines = self.at(AtCommand::ReadVoltage)?;
        let line = lines.first().ok_or_else(|| anyhow!("empty response to ATRV"))?;
        parse_voltage(line)
    }

    /// Send a diagnostic request.
    pub fn diag(&mut self, req: DiagRequest) -> Result<DiagReply> {
        let lines = self.send(Command::Diag(req))?;
        let lines: Vec<String> = lines.into_iter().filter(|l| !l.starts_with("SEARCHING")).collect();
        if lines.iter().any(|l| l == "NO DATA") {
            return Ok(DiagReply::NoData);
        }
        Ok(DiagReply::Data(lines))
    }
}

/// How long we wait for a response to different commands.
fn timeout_for(cmd: &Command) -> Duration {
    match cmd {
        Command::At(AtCommand::Reset) => Duration::from_secs(3),
        Command::At(_) => Duration::from_secs(2),
        // The first request after selecting a protocol can take a while.
        Command::Diag(_) => Duration::from_secs(6),
    }
}

/// Split a raw response into lines, removing empty lines and any echo of the
/// command we sent (echo is on until `ATE0`).
pub fn clean_response(raw: &str, sent_wire: &str) -> Vec<String> {
    raw.split(['\r', '\n'])
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter(|l| !l.replace(' ', "").eq_ignore_ascii_case(sent_wire))
        .map(|l| l.to_string())
        .collect()
}

/// Parse "12.6V" (or "12.6 V") into 12.6.
pub fn parse_voltage(line: &str) -> Result<f32> {
    let s = line.trim().trim_end_matches(['V', 'v']).trim();
    let v: f32 = s
        .parse()
        .map_err(|_| anyhow!("could not parse voltage from \"{line}\""))?;
    if !(0.0..=30.0).contains(&v) {
        bail!("implausible voltage {v} V from \"{line}\"");
    }
    Ok(v)
}

/// Open the serial port. Tries 38400 baud, then 115200, if `baud` is not given.
/// Probes with `ATI`, which only asks the chip for its version.
pub fn open_serial(path: &str, baud: Option<u32>, log: &RawLog) -> Result<Box<dyn Transport>> {
    let candidates = match baud {
        Some(b) => vec![b],
        None => vec![38400, 115200],
    };
    let probe = Command::At(AtCommand::Identify);
    let mut last_err = None;
    for b in candidates {
        log.write("info", &format!("trying {path} @ {b} baud"));
        let mut port = SerialTransport::open(path, b)?;
        // The first attempt may get "?" if there was junk in the ELM buffer; try twice.
        for _ in 0..2 {
            match port.exchange(&probe, Duration::from_millis(1500)) {
                Ok(resp) if resp.contains("ELM") => {
                    log.write("info", &format!("ELM327 responded @ {b} baud: {resp:?}"));
                    return Ok(Box::new(port));
                }
                Ok(resp) => {
                    log.write("info", &format!("unexpected response @ {b}: {resp:?}"));
                    last_err = Some(anyhow!("unexpected response: {resp:?}"));
                }
                Err(e) => {
                    let timeout = e
                        .downcast_ref::<TransportError>()
                        .is_some_and(TransportError::is_timeout);
                    log.write("info", &format!("no response @ {b}: {e:#}"));
                    last_err = Some(e);
                    if timeout {
                        break;
                    }
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("no response")).context(format!(
        "no ELM327 found on {path}. Check that the cable is connected to the car and the PC, and that FORScan is closed."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::ObdRequest;
    use crate::transport::MockTransport;

    #[test]
    fn clean_response_removes_echo_and_empty_lines() {
        assert_eq!(clean_response("ATZ\r\r\rELM327 v1.5\r\r", "ATZ"), vec!["ELM327 v1.5"]);
        assert_eq!(clean_response("12.4V\r\r", "ATRV"), vec!["12.4V"]);
        assert_eq!(
            clean_response("SEARCHING...\r7E8 06 41 00 BE 3F A8 13\r\r", "0100"),
            vec!["SEARCHING...", "7E8 06 41 00 BE 3F A8 13"]
        );
    }

    #[test]
    fn parse_voltage_values() {
        assert_eq!(parse_voltage("12.6V").unwrap(), 12.6);
        assert_eq!(parse_voltage(" 9.8 V ").unwrap(), 9.8);
        assert!(parse_voltage("?").is_err());
        assert!(parse_voltage("99V").is_err());
    }

    #[test]
    fn init_hs_sends_correct_sequence() {
        let mock = MockTransport::elm_defaults();
        let sent = mock.sent_handle();
        let mut elm = Elm::new(Box::new(mock));
        assert_eq!(elm.init(Bus::Hs).unwrap(), "ELM327 v1.5");
        assert_eq!(*sent.borrow(), vec!["ATZ", "ATE0", "ATL0", "ATS1", "ATH1", "ATSP6"]);
    }

    #[test]
    fn init_ms_configures_protocol_b() {
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
    fn elm_error_becomes_err() {
        let mock = MockTransport::elm_defaults().set("ATRV", "?\r\r");
        let mut elm = Elm::new(Box::new(mock));
        assert!(elm.voltage().is_err());
    }

    #[test]
    fn diag_no_data_and_data() {
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
