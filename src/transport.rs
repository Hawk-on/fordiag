//! Transportlaget: korleis kommandoar kjem fram til ELM327 og svar kjem tilbake.
//!
//! [`Transport`]-traiten tek berre imot `&Command` frå tillatslista. Det finst
//! ingen metode for å sende rå tekst. Tre implementasjonar:
//!
//! * [`SerialTransport`] – ekte kabel (`/dev/ttyUSB0`) eller ein pseudoterminal
//!   frå ein ELM327-emulator.
//! * [`MockTransport`] – innspelte svar, for `cargo test` og `--mock`.
//! * [`Logged`] – pakkar inn ein annan transport og loggar alt til fil.

use crate::command::{Command, checked_wire};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Feil frå transportlaget som kallaren kan ville skilje ut.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Fekk ikkje `>`-prompten innan tida. `partial` er det som kom.
    #[error("tidsavbrot: fekk ikkje svar frå ELM327 (motteke så langt: {partial:?})")]
    Timeout { partial: String },
}

pub trait Transport {
    /// Send éin tillaten kommando og returner rått svar fram til (men utan)
    /// `>`-prompten.
    fn exchange(&mut self, cmd: &Command, timeout: Duration) -> Result<String>;

    /// Kort skildring, t.d. «/dev/ttyUSB0 @ 38400» eller «mock».
    fn describe(&self) -> String;
}

// ---------------------------------------------------------------------------
// Seriell transport
// ---------------------------------------------------------------------------

pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
    path: String,
    baud: u32,
}

impl SerialTransport {
    pub fn open(path: &str, baud: u32) -> Result<Self> {
        let port = serialport::new(path, baud)
            .timeout(Duration::from_millis(50))
            .open()
            .with_context(|| format!("klarte ikkje opne {path} (er kabelen i, og er FORScan lukka?)"))?;
        Ok(SerialTransport {
            port,
            path: path.to_string(),
            baud,
        })
    }
}

impl Transport for SerialTransport {
    fn exchange(&mut self, cmd: &Command, timeout: Duration) -> Result<String> {
        let wire = checked_wire(cmd)?;

        // Kast eventuelle gamle bytes som ligg i innbufferen.
        let _ = self.port.clear(serialport::ClearBuffer::Input);

        // NB: Vi sender aldri ein tom linje. Ein tom CR får ELM327 til å
        // gjenta *førre* kommando – som kunne vore noko FORScan sende.
        self.port.write_all(format!("{wire}\r").as_bytes())?;
        self.port.flush()?;

        let deadline = Instant::now() + timeout;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 256];
        loop {
            match self.port.read(&mut chunk) {
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(e.into()),
            }
            if let Some(pos) = buf.iter().position(|&b| b == b'>') {
                buf.truncate(pos);
                break;
            }
            if Instant::now() >= deadline {
                let partial = String::from_utf8_lossy(&buf).into_owned();
                return Err(TransportError::Timeout { partial }.into());
            }
        }
        // Somme klonar sender NUL-bytes; fjern dei.
        buf.retain(|&b| b != 0);
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    fn describe(&self) -> String {
        format!("{} @ {} baud", self.path, self.baud)
    }
}

// ---------------------------------------------------------------------------
// Mock-transport
// ---------------------------------------------------------------------------

/// Funksjon som gjev spenning (V) som funksjon av tid sidan start.
pub type VoltageFn = Box<dyn FnMut(Duration) -> f32>;

/// Innspelt ELM327. Svar blir slått opp på kommandoteksten (t.d. `"ATRV"`).
/// Har ein kommando fleire svar i kø, blir dei brukte i tur; det siste blir
/// verande og gjentatt. Ukjende kommandoar gjev `?`, slik ein ekte ELM gjer.
pub struct MockTransport {
    responses: HashMap<String, VecDeque<String>>,
    voltage: Option<VoltageFn>,
    start: Instant,
    latency: Duration,
    sent: Rc<RefCell<Vec<String>>>,
}

impl Default for MockTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl MockTransport {
    /// Tom mock: alt gjev `?`.
    pub fn new() -> Self {
        MockTransport {
            responses: HashMap::new(),
            voltage: None,
            start: Instant::now(),
            latency: Duration::ZERO,
            sent: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Mock med vanlege svar på init-kommandoane, slik ein ELM327 v1.5 svarar.
    pub fn elm_defaults() -> Self {
        let mut m = Self::new()
            .on("ATZ", "\r\rELM327 v1.5\r\r")
            .on("ATI", "ELM327 v1.5\r\r")
            .on("ATRV", "12.6V\r\r");
        for ok in [
            "ATE0", "ATL0", "ATS0", "ATS1", "ATH0", "ATH1", "ATSP0", "ATSP6", "ATSPB", "ATPB8104", "ATCAF1",
        ] {
            m = m.on(ok, "OK\r\r");
        }
        m.on("ATDP", "ISO 15765-4 (CAN 11/500)\r\r")
    }

    /// Legg til eit svar i køen for `wire`.
    pub fn on(mut self, wire: &str, response: &str) -> Self {
        self.responses
            .entry(wire.to_string())
            .or_default()
            .push_back(response.to_string());
        self
    }

    /// Erstatt alle svar for `wire` med eitt.
    #[allow(dead_code)] // brukt i testar
    pub fn set(mut self, wire: &str, response: &str) -> Self {
        self.responses
            .insert(wire.to_string(), VecDeque::from([response.to_string()]));
        self
    }

    /// La `ATRV` svare med ein funksjon av tida (for å simulere start o.l.).
    pub fn with_voltage(mut self, f: VoltageFn) -> Self {
        self.voltage = Some(f);
        self
    }

    /// Simuler at kvar kommando tek litt tid, som på ein ekte kabel.
    pub fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    /// Handtak til lista over alt som er sendt (for testar).
    #[allow(dead_code)] // brukt i testar
    pub fn sent_handle(&self) -> Rc<RefCell<Vec<String>>> {
        Rc::clone(&self.sent)
    }
}

impl Transport for MockTransport {
    fn exchange(&mut self, cmd: &Command, _timeout: Duration) -> Result<String> {
        let wire = checked_wire(cmd)?;
        self.sent.borrow_mut().push(wire.clone());
        if !self.latency.is_zero() {
            std::thread::sleep(self.latency);
        }

        if wire == "ATRV" {
            if let Some(f) = self.voltage.as_mut() {
                let v = f(self.start.elapsed());
                // Ekte ELM327 svarar med éin desimal.
                return Ok(format!("{v:.1}V\r\r"));
            }
        }

        let Some(queue) = self.responses.get_mut(&wire) else {
            return Ok("?\r\r".to_string());
        };
        let resp = if queue.len() > 1 {
            queue.pop_front().unwrap()
        } else {
            queue.front().cloned().unwrap_or_default()
        };
        Ok(resp)
    }

    fn describe(&self) -> String {
        "mock (ingen bil)".to_string()
    }
}

// ---------------------------------------------------------------------------
// Rålogg
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct LogEntry<'a> {
    /// Tidsstempel, lokal tid med millisekund.
    ts: String,
    /// "tx" (sendt), "rx" (motteke), "err" (feil) eller "info".
    dir: &'a str,
    data: &'a str,
}

/// Loggfil i JSON Lines-format: éi linje per hending. Kvar linje blir skriven
/// og flusha med ein gong, så loggen overlever om programmet krasjar.
#[derive(Clone)]
pub struct RawLog {
    file: Rc<RefCell<File>>,
    path: PathBuf,
}

impl RawLog {
    /// Lag ny loggfil `<dir>/raw-ÅÅÅÅMMDD-TTMMSS.jsonl`.
    pub fn create(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("klarte ikkje lage {}", dir.display()))?;
        let name = format!("raw-{}.jsonl", chrono::Local::now().format("%Y%m%d-%H%M%S"));
        let path = dir.join(name);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(RawLog {
            file: Rc::new(RefCell::new(file)),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, dir: &str, data: &str) {
        let entry = LogEntry {
            ts: chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string(),
            dir,
            data,
        };
        if let Ok(line) = serde_json::to_string(&entry) {
            let mut f = self.file.borrow_mut();
            // Loggfeil skal ikkje stoppe målinga; vi prøver vårt beste.
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
}

/// Transport som loggar alt som går gjennom ein annan transport.
pub struct Logged {
    inner: Box<dyn Transport>,
    log: RawLog,
}

impl Logged {
    pub fn new(inner: Box<dyn Transport>, log: RawLog) -> Self {
        log.write("info", &format!("transport: {}", inner.describe()));
        Logged { inner, log }
    }
}

impl Transport for Logged {
    fn exchange(&mut self, cmd: &Command, timeout: Duration) -> Result<String> {
        // Logg berre det som faktisk kan sendast; avviste kommandoar blir
        // logga som feil.
        match checked_wire(cmd) {
            Ok(wire) => self.log.write("tx", &wire),
            Err(e) => {
                self.log.write("err", &format!("avvist: {e}"));
                bail!(e);
            }
        }
        match self.inner.exchange(cmd, timeout) {
            Ok(resp) => {
                self.log.write("rx", &resp);
                Ok(resp)
            }
            Err(e) => {
                self.log.write("err", &format!("{e:#}"));
                Err(e)
            }
        }
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::AtCommand;

    #[test]
    fn mock_koar_og_gjentek_siste_svar() {
        let mut m = MockTransport::new().on("ATI", "A").on("ATI", "B");
        let cmd = Command::At(AtCommand::Identify);
        let t = Duration::from_secs(1);
        assert_eq!(m.exchange(&cmd, t).unwrap(), "A");
        assert_eq!(m.exchange(&cmd, t).unwrap(), "B");
        assert_eq!(m.exchange(&cmd, t).unwrap(), "B");
        assert_eq!(m.exchange(&Command::At(AtCommand::Reset), t).unwrap(), "?\r\r");
        assert_eq!(*m.sent_handle().borrow(), vec!["ATI", "ATI", "ATI", "ATZ"]);
    }

    #[test]
    fn logg_skriv_tx_og_rx() {
        let dir = std::env::temp_dir().join(format!("fordiag-test-{}", std::process::id()));
        let log = RawLog::create(&dir).unwrap();
        let mut t = Logged::new(Box::new(MockTransport::elm_defaults()), log.clone());
        t.exchange(&Command::At(AtCommand::Identify), Duration::from_secs(1))
            .unwrap();
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(text.contains("\"dir\":\"tx\",\"data\":\"ATI\""));
        assert!(text.contains("\"dir\":\"rx\""));
        let _ = std::fs::remove_dir_all(dir);
    }
}
