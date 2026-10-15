//! Replay of a recorded raw log (`raw-*.jsonl`) as a transport.
//!
//! Every `tx` in the log is paired with the `rx` (or `err`) that followed it. When
//! fordiag sends a command, the next recorded pair must have exactly the same `tx`;
//! the recorded response is then returned. Anything else is an error, so a replay
//! shows immediately when the code would now send something different from what
//! it sent to the car.
//!
//! Uses:
//! * `fordiag --replay <log> dtc …` re-runs a scan against a recording, for example
//!   to check that a parser fix interprets an old answer correctly.
//! * Recordings in `fixtures/replay/` become regression tests in `cargo test`.
//!
//! Battery tests are time-based, so replaying them is not meaningful. Their saved
//! JSON files already contain every sample and can be re-analysed directly.

use crate::command::{Command, checked_wire};
use crate::transport::{Transport, TransportError};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

/// One recorded command and what came back.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub tx: String,
    /// `Ok(raw response)` or `Err(error message)` as logged.
    pub rx: Result<String, String>,
}

#[derive(Deserialize)]
struct Entry {
    dir: String,
    data: String,
}

/// Turn the lines of a raw log into tx/rx steps. `info` lines are ignored, and so
/// are errors that did not follow a `tx` (e.g. a command that was rejected
/// before it was sent).
pub fn parse_log(text: &str) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    let mut pending: Option<String> = None;
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let e: Entry = serde_json::from_str(line).with_context(|| format!("line {} is not a log entry", n + 1))?;
        match e.dir.as_str() {
            "tx" => {
                if let Some(tx) = pending.take() {
                    bail!("line {}: \"{tx}\" was sent but no response was logged", n + 1);
                }
                pending = Some(e.data);
            }
            "rx" | "err" => {
                if let Some(tx) = pending.take() {
                    let rx = if e.dir == "rx" { Ok(e.data) } else { Err(e.data) };
                    steps.push(Step { tx, rx });
                }
            }
            _ => {}
        }
    }
    // A final tx without a response (e.g. the program was interrupted) is dropped.
    Ok(steps)
}

pub struct ReplayTransport {
    steps: Vec<Step>,
    /// How many steps have been used. Shared so tests can check that a replay
    /// consumed the whole recording.
    pos: Rc<Cell<usize>>,
    name: String,
}

impl ReplayTransport {
    pub fn new(steps: Vec<Step>, name: &str) -> Self {
        ReplayTransport {
            steps,
            pos: Rc::new(Cell::new(0)),
            name: name.to_string(),
        }
    }

    pub fn open(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("log");
        Ok(Self::new(parse_log(&text)?, name))
    }

    #[allow(dead_code)] // used in tests
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    #[allow(dead_code)] // used in tests
    pub fn progress_handle(&self) -> Rc<Cell<usize>> {
        Rc::clone(&self.pos)
    }
}

impl Transport for ReplayTransport {
    fn exchange(&mut self, cmd: &Command, _timeout: Duration) -> Result<String> {
        let wire = checked_wire(cmd)?;
        let i = self.pos.get();
        let Some(step) = self.steps.get(i) else {
            bail!(
                "replay of {} ended: there is no recorded response for \"{wire}\" (step {})",
                self.name,
                i + 1
            );
        };
        if step.tx != wire {
            bail!(
                "replay of {} diverged at step {}: the recording sent \"{}\", now \"{wire}\". \
                 Run the same command with the same options as when it was recorded.",
                self.name,
                i + 1,
                step.tx
            );
        }
        self.pos.set(i + 1);
        match &step.rx {
            Ok(rx) => Ok(rx.clone()),
            Err(e) if e.starts_with("timeout") => Err(TransportError::Timeout { partial: String::new() }.into()),
            Err(e) => bail!("recorded error: {e}"),
        }
    }

    fn describe(&self) -> String {
        format!("replay of {}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::AtCommand;
    use crate::dtc::{self, DtcScan, ModuleResult};
    use crate::elm::{Bus, Elm};

    const LOG: &str = r#"{"ts":"t","dir":"info","data":"transport: test"}
{"ts":"t","dir":"tx","data":"ATZ"}
{"ts":"t","dir":"rx","data":"\r\rELM327 v1.5\r\r"}
{"ts":"t","dir":"tx","data":"ATI"}
{"ts":"t","dir":"err","data":"timeout: no response from the ELM327 (received so far: \"\")"}
"#;

    #[test]
    fn parses_tx_rx_and_errors() {
        let steps = parse_log(LOG).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].tx, "ATZ");
        assert_eq!(steps[0].rx, Ok("\r\rELM327 v1.5\r\r".to_string()));
        assert!(steps[1].rx.as_ref().unwrap_err().starts_with("timeout"));
        assert!(parse_log("not json").is_err());
    }

    #[test]
    fn replays_in_order_and_detects_divergence() {
        let mut t = ReplayTransport::new(parse_log(LOG).unwrap(), "test");
        let d = Duration::from_secs(1);
        assert_eq!(
            t.exchange(&Command::At(AtCommand::Reset), d).unwrap(),
            "\r\rELM327 v1.5\r\r"
        );
        let err = t.exchange(&Command::At(AtCommand::Identify), d).unwrap_err();
        assert!(
            err.downcast_ref::<TransportError>().is_some(),
            "timeout should replay as timeout"
        );
        assert!(
            t.exchange(&Command::At(AtCommand::Identify), d).is_err(),
            "recording has ended"
        );

        let mut t = ReplayTransport::new(parse_log(LOG).unwrap(), "test");
        let err = t.exchange(&Command::At(AtCommand::EchoOff), d).unwrap_err();
        assert!(err.to_string().contains("diverged"));
    }

    /// Re-run a recorded `fordiag dtc` scan: the bus and the modules are taken
    /// from the recording itself (`ATSPB` means MS-CAN; the `ATSH` commands give
    /// the modules in order).
    fn rerun_dtc(path: &Path) -> Vec<ModuleResult> {
        let replay = ReplayTransport::open(path).unwrap();
        let bus = if replay.steps().iter().any(|s| s.tx == "ATSPB") {
            Bus::Ms
        } else {
            Bus::Hs
        };
        let modules: Vec<&dtc::Module> = replay
            .steps()
            .iter()
            .filter_map(|s| s.tx.strip_prefix("ATSH"))
            .map(|h| {
                let id = u16::from_str_radix(h, 16).unwrap();
                dtc::CANDIDATES
                    .iter()
                    .find(|m| m.request == id)
                    .unwrap_or_else(|| panic!("{}: unknown module {h}", path.display()))
            })
            .collect();
        let total = replay.steps().len();
        let progress = replay.progress_handle();

        let mut elm = Elm::new(Box::new(replay));
        elm.init(bus).unwrap();
        elm.prepare_for_dtc().unwrap();
        let results: Vec<ModuleResult> = modules
            .iter()
            .map(|m| dtc::scan_module(&mut elm, m, m.name == "PCM").unwrap())
            .collect();
        assert_eq!(
            progress.get(),
            total,
            "{}: not all recorded steps were replayed",
            path.display()
        );
        results
    }

    /// Every `fixtures/replay/dtc-*.jsonl` is replayed. If a matching
    /// `.expected.json` (a saved `DtcScan`) exists, the module results must match it.
    #[test]
    fn recorded_fixtures_replay_unchanged() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/replay");
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        let mut n = 0;
        for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            if !(name.starts_with("dtc-") && name.ends_with(".jsonl")) {
                continue;
            }
            n += 1;
            let results = rerun_dtc(&path);
            let expected = path.with_file_name(name.replace(".jsonl", ".expected.json"));
            if let Ok(text) = std::fs::read_to_string(&expected) {
                let scan: DtcScan = serde_json::from_str(&text).unwrap();
                assert_eq!(
                    results,
                    scan.modules,
                    "{name}: results differ from {}",
                    expected.display()
                );
            }
        }
        assert!(n > 0, "no fixtures found in {}", dir.display());
    }
}
