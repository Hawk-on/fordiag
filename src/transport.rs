//! The transport layer: how commands reach the ELM327 and responses come back.
//!
//! The [`Transport`] trait only accepts `&Command` from the allowlist. There is
//! no method for sending raw text. Three implementations:
//!
//! * [`SerialTransport`] – a real cable (`/dev/ttyUSB0`) or a pseudo-terminal
//!   from an ELM327 emulator.
//! * [`MockTransport`] – recorded responses, for `cargo test` and `--mock`.
//! * [`Logged`] – wraps another transport and logs everything to a file.

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

/// Transport-layer errors the caller may want to tell apart.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The `>` prompt did not arrive in time. `partial` is what did arrive.
    #[error("timeout: no response from the ELM327 (received so far: {partial:?})")]
    Timeout { partial: String },
    /// An error replayed from a raw log, with exactly the text that was logged.
    /// `timeout` tells whether the original error was a timeout.
    #[error("{message}")]
    Recorded { message: String, timeout: bool },
}

impl TransportError {
    pub fn is_timeout(&self) -> bool {
        matches!(
            self,
            TransportError::Timeout { .. } | TransportError::Recorded { timeout: true, .. }
        )
    }
}

pub trait Transport {
    /// Send one allowed command and return the raw response up to (but not
    /// including) the `>` prompt.
    fn exchange(&mut self, cmd: &Command, timeout: Duration) -> Result<String>;

    /// Short description, e.g. "/dev/ttyUSB0 @ 38400" or "mock".
    fn describe(&self) -> String;
}

// ---------------------------------------------------------------------------
// Serial transport
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
            .with_context(|| format!("could not open {path} (is the cable plugged in, and is FORScan closed?)"))?;
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

        // Discard any stale bytes in the input buffer.
        let _ = self.port.clear(serialport::ClearBuffer::Input);

        // NB: We never send an empty line. A bare CR makes the ELM327 repeat
        // the *previous* command – which could have been something FORScan sent.
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
        // Some clones send NUL bytes; strip them.
        buf.retain(|&b| b != 0);
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    fn describe(&self) -> String {
        format!("{} @ {} baud", self.path, self.baud)
    }
}

// ---------------------------------------------------------------------------
// Mock transport
// ---------------------------------------------------------------------------

/// Function giving voltage (V) as a function of time since start.
pub type VoltageFn = Box<dyn FnMut(Duration) -> f32>;

/// Recorded ELM327. Responses are looked up by command text (e.g. `"ATRV"`).
/// If a command has several queued responses they are used in turn; the last
/// one stays and is repeated. Unknown commands return `?`, as a real ELM does.
pub struct MockTransport {
    responses: HashMap<String, VecDeque<String>>,
    voltage: Option<VoltageFn>,
    start: Instant,
    latency: Duration,
    sent: Rc<RefCell<Vec<String>>>,
    /// The CAN ID last set with `ATSH`, so responses can depend on the target.
    header: Option<String>,
    /// Response to diagnostic requests that have no recorded answer.
    diag_default: Option<String>,
}

impl Default for MockTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl MockTransport {
    /// Empty mock: everything returns `?`.
    pub fn new() -> Self {
        MockTransport {
            responses: HashMap::new(),
            voltage: None,
            start: Instant::now(),
            latency: Duration::ZERO,
            sent: Rc::new(RefCell::new(Vec::new())),
            header: None,
            diag_default: None,
        }
    }

    /// Mock with the usual responses to the init commands, as an ELM327 v1.5 replies.
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

    /// Add a response to the queue for `wire`.
    pub fn on(mut self, wire: &str, response: &str) -> Self {
        self.responses
            .entry(wire.to_string())
            .or_default()
            .push_back(response.to_string());
        self
    }

    /// Add a response for `wire` that only applies while `ATSH<header>` is active.
    pub fn on_target(self, header: &str, wire: &str, response: &str) -> Self {
        self.on(&format!("{header}|{wire}"), response)
    }

    /// Response to diagnostic requests without a recorded answer (e.g. `NO DATA`).
    pub fn with_diag_default(mut self, response: &str) -> Self {
        self.diag_default = Some(response.to_string());
        self
    }

    /// Replace all responses for `wire` with one.
    #[allow(dead_code)] // used in tests
    pub fn set(mut self, wire: &str, response: &str) -> Self {
        self.responses
            .insert(wire.to_string(), VecDeque::from([response.to_string()]));
        self
    }

    /// Let `ATRV` respond with a function of time (to simulate cranking etc.).
    pub fn with_voltage(mut self, f: VoltageFn) -> Self {
        self.voltage = Some(f);
        self
    }

    /// Simulate that each command takes some time, as on a real cable.
    pub fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    /// Handle to the list of everything sent (for tests).
    #[allow(dead_code)] // used in tests
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

        if wire == "ATRV"
            && let Some(f) = self.voltage.as_mut()
        {
            let v = f(self.start.elapsed());
            // A real ELM327 replies with one decimal.
            return Ok(format!("{v:.1}V\r\r"));
        }

        if let Some(h) = wire.strip_prefix("ATSH") {
            self.header = Some(h.to_string());
        }

        // Look for a target-specific response first, then a general one.
        let targeted = self.header.as_ref().map(|h| format!("{h}|{wire}"));
        let key = match targeted {
            Some(k) if self.responses.contains_key(&k) => k,
            _ => wire.clone(),
        };
        let Some(queue) = self.responses.get_mut(&key) else {
            // Setting header, filter or timeout always succeeds on a real ELM.
            if ["ATSH", "ATCRA", "ATST"].iter().any(|p| wire.starts_with(p)) {
                return Ok("OK\r\r".to_string());
            }
            if !wire.starts_with("AT")
                && let Some(d) = &self.diag_default
            {
                return Ok(d.clone());
            }
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
        "mock (no car)".to_string()
    }
}

// ---------------------------------------------------------------------------
// Raw log
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct LogEntry<'a> {
    /// Timestamp, local time with milliseconds.
    ts: String,
    /// "tx" (sent), "rx" (received), "err" (error) or "info".
    dir: &'a str,
    data: &'a str,
}

/// Log file in JSON Lines format: one line per event. Each line is written and
/// flushed immediately, so the log survives if the program crashes.
#[derive(Clone)]
pub struct RawLog {
    file: Rc<RefCell<File>>,
    path: PathBuf,
}

impl RawLog {
    /// Create a new log file `<dir>/raw-YYYYMMDD-HHMMSS.jsonl`.
    pub fn create(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
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
            // Logging errors must not stop a measurement; best effort.
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
}

/// Transport that logs everything passing through another transport.
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
        // Only log what can actually be sent; rejected commands are logged
        // as errors.
        match checked_wire(cmd) {
            Ok(wire) => self.log.write("tx", &wire),
            Err(e) => {
                self.log.write("err", &format!("rejected: {e}"));
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
    fn mock_queues_and_repeats_last_response() {
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
    fn log_writes_tx_and_rx() {
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
