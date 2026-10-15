//! Phase 4: a Markdown report that can be shared.
//!
//! The report collects the latest saved results from the data directory:
//! one battery test of each kind, the latest DTC scan for each bus, and the raw
//! logs of those runs as an appendix. Rendering is a pure function of
//! [`ReportInput`], so it can be tested without files or a car.

use crate::battery::{self, BatteryTest, Latest, fmt_v};
use crate::dtc::{self, DtcScan, ModuleResult, Outcome, Read};
use anyhow::Result;
use std::fmt::Write as _;
use std::path::Path;

/// Everything the report is built from.
pub struct ReportInput {
    pub generated: chrono::DateTime<chrono::Local>,
    /// Free text describing the vehicle, e.g. "Ford S-Max Mk2 2.0 TDCi". Optional,
    /// so the report never contains the VIN unless the user puts it there.
    pub vehicle: Option<String>,
    pub battery: Latest,
    pub dtc: Vec<DtcScan>,
    /// Raw logs to include as an appendix (empty with `--no-raw`).
    pub raw_logs: Vec<RawLogFile>,
}

/// One raw log, already formatted as readable lines.
pub struct RawLogFile {
    pub name: String,
    /// What the log belongs to, e.g. "battery: rest".
    pub belongs_to: String,
    /// The formatted lines, or why they are not included.
    pub lines: Result<Vec<String>, &'static str>,
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Load the latest results (and optionally their raw logs) from `dir`.
pub fn load(dir: &Path, vehicle: Option<String>, include_raw: bool) -> Result<ReportInput> {
    let battery = BatteryTest::load_latest(dir)?;
    let dtc = DtcScan::load_latest(dir)?;

    let mut raw_logs = Vec::new();
    if include_raw {
        // Which raw log belongs to which result, in report order.
        let mut wanted: Vec<(Option<&String>, String)> = Vec::new();
        for t in [&battery.rest, &battery.crank, &battery.charging].into_iter().flatten() {
            wanted.push((t.raw_log.as_ref(), format!("battery: {}", t.analysis.mode_name())));
        }
        for s in &dtc {
            wanted.push((s.raw_log.as_ref(), format!("dtc: {}-can", s.bus)));
        }
        for (name, belongs_to) in wanted {
            let Some(name) = name else { continue };
            let lines = if !is_plain_log_name(name) {
                // A result file is just JSON and may come from elsewhere: never let
                // it point the report at another file on this machine.
                Err("file name rejected: not a plain raw-*.jsonl name in the logs directory")
            } else {
                std::fs::read_to_string(dir.join("logs").join(name))
                    .map(|text| text.lines().map(format_log_line).collect())
                    .map_err(|_| "log file not found")
            };
            raw_logs.push(RawLogFile {
                name: name.clone(),
                belongs_to,
                lines,
            });
        }
    }

    Ok(ReportInput {
        generated: chrono::Local::now(),
        vehicle,
        battery,
        dtc,
        raw_logs,
    })
}

/// True for a plain file name such as `raw-20261018-101500.jsonl`: no directory
/// parts, no `..`, no absolute path, on any platform.
fn is_plain_log_name(name: &str) -> bool {
    name.starts_with("raw-")
        && name.ends_with(".jsonl")
        && !name.contains(['/', '\\', ':'])
        && !name.contains("..")
        && Path::new(name).file_name().and_then(|n| n.to_str()) == Some(name)
}

/// Turn one JSON line from the raw log into `timestamp dir data`, with control
/// characters such as `\r` made visible.
fn format_log_line(line: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return line.to_string();
    };
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
    let data: String = get("data").escape_debug().collect();
    format!("{} {:<4} {}", get("ts"), get("dir"), data)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Make text safe inside a Markdown table cell.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\r', '\n'], " ")
}

fn when(started: &chrono::DateTime<chrono::Local>, source: &str) -> String {
    format!("{} ({})", started.format("%Y-%m-%d %H:%M"), cell(source))
}

/// True if any of the results came from the simulator or from a replayed log,
/// i.e. not from a live connection to the car.
fn contains_mock(input: &ReportInput) -> bool {
    let not_live = |source: &str| source.contains("mock") || source.starts_with("replay");
    let b = &input.battery;
    [&b.rest, &b.crank, &b.charging]
        .into_iter()
        .flatten()
        .any(|t| not_live(&t.source))
        || input.dtc.iter().any(|s| not_live(&s.source))
}

/// Render the whole report as Markdown.
pub fn render(input: &ReportInput) -> String {
    let mut out = String::new();
    // Writing to a String cannot fail, so the results of writeln! are ignored.
    let w = &mut out;

    let _ = writeln!(w, "# fordiag report\n");
    let _ = writeln!(
        w,
        "Generated {} with fordiag {}.",
        input.generated.format("%Y-%m-%d %H:%M"),
        env!("CARGO_PKG_VERSION")
    );
    if let Some(v) = &input.vehicle {
        let _ = writeln!(w, "\nVehicle: {v}");
    }
    let _ = writeln!(
        w,
        "\n> [!WARNING]\n\
         > Produced by fordiag, a read-only hobby tool. Provided \"as is\", without warranty; use at your own risk.\n\
         > Voltages are measured at the OBD connector by an ELM327 (0.1 V resolution, clones may be off by 0.1–0.2 V).\n\
         > DTC meanings are not included, and failure-type texts are generic SAE suggestions:\n\
         > check everything against Ford documentation before acting on it."
    );
    if contains_mock(input) {
        let _ = writeln!(
            w,
            "\n> [!CAUTION]\n> This report contains **simulated data** from `--mock` or results re-run with `--replay`. \
             Check the source of each result; simulated data does not describe a real car."
        );
    }

    render_summary(w, input);
    render_battery(w, &input.battery);
    render_dtcs(w, &input.dtc);
    render_raw_logs(w, &input.raw_logs);
    out
}

fn render_summary(w: &mut String, input: &ReportInput) {
    let _ = writeln!(w, "\n## Summary\n");
    let b = &input.battery;
    let rest = b.rest.as_ref().and_then(|t| match &t.analysis {
        battery::Analysis::Rest(r) => Some(r),
        _ => None,
    });
    let crank = b.crank.as_ref().and_then(|t| match &t.analysis {
        battery::Analysis::Crank(r) => Some(r),
        _ => None,
    });
    let charging = b.charging.as_ref().and_then(|t| match &t.analysis {
        battery::Analysis::Charging(r) => Some(r),
        _ => None,
    });

    let _ = writeln!(w, "**Battery**\n");
    for line in battery::overall_assessment(rest, crank, charging) {
        let _ = writeln!(w, "- {line}");
    }

    let _ = writeln!(w, "\n**DTCs**\n");
    if input.dtc.is_empty() {
        let _ = writeln!(w, "- No DTC scan saved yet. Run `fordiag dtc --bus hs` and `--bus ms`.");
    }
    for scan in &input.dtc {
        let responded: Vec<&str> = scan
            .modules
            .iter()
            .filter(|m| m.responded())
            .map(|m| m.name.as_str())
            .collect();
        let total: usize = scan.modules.iter().map(module_dtc_count).sum();
        let _ = writeln!(
            w,
            "- {}-CAN ({}): {} DTC(s) in total; modules that responded: {}.",
            scan.bus.to_uppercase(),
            scan.started.format("%Y-%m-%d %H:%M"),
            total,
            if responded.is_empty() {
                "none".to_string()
            } else {
                responded.join(", ")
            }
        );
    }
}

fn render_battery(w: &mut String, b: &Latest) {
    let _ = writeln!(w, "\n## Battery tests");

    let _ = writeln!(w, "\n### Resting voltage\n");
    match &b.rest {
        Some(
            t @ BatteryTest {
                analysis: battery::Analysis::Rest(r),
                ..
            },
        ) => {
            table(
                w,
                &[
                    ("Measured", when(&t.started, &t.source)),
                    ("Median", fmt_v(r.median)),
                    ("Min / max", format!("{} / {}", fmt_v(r.min), fmt_v(r.max))),
                    ("Measurements", t.samples.len().to_string()),
                    ("Verdict", r.verdict.text().to_string()),
                ],
            );
            warnings(w, &r.warnings);
        }
        _ => {
            let _ = writeln!(w, "Not measured. Run `fordiag battery --rest`.");
        }
    }

    let _ = writeln!(w, "\n### Crank test\n");
    match &b.crank {
        Some(
            t @ BatteryTest {
                analysis: battery::Analysis::Crank(r),
                ..
            },
        ) => {
            table(
                w,
                &[
                    ("Measured", when(&t.started, &t.source)),
                    ("Before cranking", fmt_v(r.before)),
                    (
                        "Lowest",
                        format!("{} (after {:.1} s)", fmt_v(r.min), r.min_t_ms as f32 / 1000.0),
                    ),
                    ("After cranking", fmt_v(r.after)),
                    (
                        "Measurements",
                        format!("{} (mean {:.0} ms apart)", t.samples.len(), r.mean_interval_ms),
                    ),
                    ("Verdict", r.verdict.text().to_string()),
                ],
            );
            warnings(w, &r.warnings);
        }
        _ => {
            let _ = writeln!(w, "Not measured. Run `fordiag battery --crank`.");
        }
    }

    let _ = writeln!(w, "\n### Charging test\n");
    match &b.charging {
        Some(
            t @ BatteryTest {
                analysis: battery::Analysis::Charging(r),
                ..
            },
        ) => {
            table(
                w,
                &[
                    ("Measured", when(&t.started, &t.source)),
                    (
                        "Min / mean / max",
                        format!("{} / {} / {}", fmt_v(r.min), fmt_v(r.mean), fmt_v(r.max)),
                    ),
                    ("Trend", format!("{} ({:+.2} V/min)", r.trend_text(), r.slope_v_per_min)),
                    ("Time ≥ 13.2 V", format!("{:.0} %", r.share_above_13_2 * 100.0)),
                    ("Measurements", t.samples.len().to_string()),
                    ("Verdict", r.verdict.text().to_string()),
                ],
            );
            warnings(w, &r.warnings);
        }
        _ => {
            let _ = writeln!(w, "Not measured. Run `fordiag battery --charging`.");
        }
    }
}

/// A two-column table without a header row.
fn table(w: &mut String, rows: &[(&str, String)]) {
    let _ = writeln!(w, "| | |\n|---|---|");
    for (k, v) in rows {
        let _ = writeln!(w, "| {} | {} |", k, cell(v));
    }
}

fn warnings(w: &mut String, list: &[String]) {
    if list.is_empty() {
        return;
    }
    let _ = writeln!(w, "\nWarnings:\n");
    for x in list {
        let _ = writeln!(w, "- {x}");
    }
}

fn module_dtc_count(m: &ModuleResult) -> usize {
    m.reads
        .iter()
        .map(|r| match &r.outcome {
            Outcome::Dtcs { dtcs } => dtcs.len(),
            _ => 0,
        })
        .sum()
}

/// One-line summary of a read, e.g. "no DTCs" or "negative response 0x31 (…)".
fn read_summary(r: &Read) -> String {
    match &r.outcome {
        Outcome::NoResponse => "no response".into(),
        Outcome::Dtcs { dtcs } if dtcs.is_empty() => "no DTCs".into(),
        Outcome::Dtcs { dtcs } => format!("{} DTC(s)", dtcs.len()),
        Outcome::Negative { nrc, meaning } => format!("negative response 0x{nrc:02X} ({meaning})"),
        Outcome::Error { message } => format!("error: {message}"),
    }
}

fn render_dtcs(w: &mut String, scans: &[DtcScan]) {
    let _ = writeln!(w, "\n## DTCs");
    if scans.is_empty() {
        let _ = writeln!(w, "\nNo DTC scan saved yet.");
        return;
    }
    let _ = writeln!(
        w,
        "\nModule addresses are **unverified** candidates; \"responded\" means a module answered on that address \
         and bus. DTC meanings are not included: look them up in FORScan or Ford documentation. \
         \"Type\" is a generic SAE J2012 **suggestion** for the failure type byte."
    );

    for scan in scans {
        let _ = writeln!(
            w,
            "\n### {}-CAN – {}\n",
            scan.bus.to_uppercase(),
            when(&scan.started, &scan.source)
        );
        let _ = writeln!(w, "| Module | Address | Responded | Result |\n|---|---|---|---|");
        for m in &scan.modules {
            let result: Vec<String> = m
                .reads
                .iter()
                .map(|r| format!("{}: {}", r.label, read_summary(r)))
                .collect();
            let _ = writeln!(
                w,
                "| {} | {} → {} | {} | {} |",
                m.name,
                m.request_id,
                m.response_id,
                if m.responded() { "yes" } else { "no" },
                cell(&result.join("; "))
            );
        }

        for m in scan.modules.iter().filter(|m| module_dtc_count(m) > 0) {
            let _ = writeln!(
                w,
                "\n#### {} ({} → {}) – {}\n",
                m.name,
                m.request_id,
                m.response_id,
                cell(&m.description)
            );
            let _ = writeln!(
                w,
                "| Read | Code | Category | Type (suggestion) | Status |\n|---|---|---|---|---|"
            );
            for r in &m.reads {
                let Outcome::Dtcs { dtcs } = &r.outcome else { continue };
                for d in dtcs {
                    let ftb = d.ftb.and_then(dtc::ftb_suggestion).unwrap_or("–");
                    let status = match d.status {
                        Some(st) => format!("0x{st:02X}: {}", dtc::status_text(st).join(", ")),
                        None => "–".into(),
                    };
                    let _ = writeln!(
                        w,
                        "| {} | {} | {} | {} | {} |",
                        cell(&r.label),
                        d.display(),
                        dtc::code_category(&d.code),
                        ftb,
                        cell(&status)
                    );
                }
            }
        }
    }
}

fn render_raw_logs(w: &mut String, logs: &[RawLogFile]) {
    if logs.is_empty() {
        return;
    }
    let _ = writeln!(w, "\n## Appendix: raw logs\n");
    let _ = writeln!(
        w,
        "All communication with the cable, one line per event: timestamp, direction \
         (`tx` sent, `rx` received, `err` error, `info`) and data. `\\r` is a carriage return."
    );
    for log in logs {
        let _ = writeln!(w, "\n### {} ({})\n", log.name, log.belongs_to);
        match &log.lines {
            Ok(lines) => {
                // Four tildes, so nothing inside the log can end the block early.
                let _ = writeln!(w, "~~~~text");
                for l in lines {
                    let _ = writeln!(w, "{l}");
                }
                let _ = writeln!(w, "~~~~");
            }
            Err(why) => {
                let _ = writeln!(w, "({why})");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery::{Analysis, Sample, analyse_crank, analyse_rest};
    use crate::dtc::Dtc;

    fn series(interval_ms: u64, volts: &[f32]) -> Vec<Sample> {
        volts
            .iter()
            .enumerate()
            .map(|(i, &v)| Sample {
                t_ms: i as u64 * interval_ms,
                volts: v,
            })
            .collect()
    }

    fn rest_test(source: &str) -> BatteryTest {
        let s = series(500, &[12.5; 6]);
        BatteryTest {
            started: chrono::Local::now(),
            source: source.into(),
            analysis: Analysis::Rest(analyse_rest(&s)),
            samples: s,
            raw_log: Some("raw-20261018-080000.jsonl".into()),
        }
    }

    fn scan(source: &str) -> DtcScan {
        DtcScan {
            started: chrono::Local::now(),
            source: source.into(),
            bus: "ms".into(),
            modules: vec![
                ModuleResult {
                    name: "BCM".into(),
                    description: "Body control module".into(),
                    request_id: "726".into(),
                    response_id: "72E".into(),
                    reads: vec![Read {
                        label: "UDS 19 02 FF".into(),
                        request: "1902FF".into(),
                        outcome: Outcome::Dtcs {
                            dtcs: vec![Dtc {
                                code: "U3003".into(),
                                ftb: Some(0x16),
                                status: Some(0x08),
                            }],
                        },
                        raw: vec![],
                    }],
                },
                ModuleResult {
                    name: "APIM".into(),
                    description: "SYNC".into(),
                    request_id: "7D0".into(),
                    response_id: "7D8".into(),
                    reads: vec![Read {
                        label: "UDS 19 02 FF".into(),
                        request: "1902FF".into(),
                        outcome: Outcome::NoResponse,
                        raw: vec![],
                    }],
                },
            ],
            raw_log: None,
        }
    }

    fn input(battery: Latest, dtc: Vec<DtcScan>) -> ReportInput {
        ReportInput {
            generated: chrono::Local::now(),
            vehicle: Some("Ford S-Max Mk2".into()),
            battery,
            dtc,
            raw_logs: vec![],
        }
    }

    #[test]
    fn empty_report_says_what_is_missing() {
        let md = render(&input(Latest::default(), vec![]));
        assert!(md.starts_with("# fordiag report"));
        assert!(md.contains("Vehicle: Ford S-Max Mk2"));
        assert!(md.contains("Not measured. Run `fordiag battery --rest`."));
        assert!(md.contains("No DTC scan saved yet"));
        assert!(!md.contains("simulated data"));
        assert!(!md.contains("Appendix"));
    }

    #[test]
    fn report_with_results() {
        let battery = Latest {
            rest: Some(rest_test("/dev/ttyUSB0 @ 38400 baud")),
            crank: Some(BatteryTest {
                analysis: Analysis::Crank(analyse_crank(&series(50, &[12.4, 12.4, 10.3, 11.0, 14.2, 14.2]))),
                ..rest_test("/dev/ttyUSB0 @ 38400 baud")
            }),
            charging: None,
        };
        let md = render(&input(battery, vec![scan("/dev/ttyUSB0 @ 38400 baud")]));
        assert!(md.contains("| Median | 12.50 V |"));
        assert!(md.contains("### Crank test"));
        assert!(md.contains("Not measured. Run `fordiag battery --charging`."));
        assert!(md.contains("- MS-CAN ("));
        assert!(md.contains("1 DTC(s) in total; modules that responded: BCM."));
        assert!(md.contains("| BCM | 726 → 72E | yes | UDS 19 02 FF: 1 DTC(s) |"));
        assert!(md.contains("| APIM | 7D0 → 7D8 | no | UDS 19 02 FF: no response |"));
        assert!(
            md.contains(
                "| UDS 19 02 FF | U3003:16 | SAE-defined | circuit voltage below threshold | 0x08: confirmed |"
            )
        );
    }

    #[test]
    fn mock_data_is_flagged() {
        let battery = Latest {
            rest: Some(rest_test("mock (no car)")),
            ..Latest::default()
        };
        assert!(render(&input(battery, vec![])).contains("simulated data"));
        assert!(render(&input(Latest::default(), vec![scan("mock (no car)")])).contains("simulated data"));
        assert!(render(&input(Latest::default(), vec![scan("replay of raw-x.jsonl")])).contains("simulated data"));
    }

    #[test]
    fn table_cells_are_escaped() {
        assert_eq!(cell("a|b\r\nc"), "a\\|b  c");
    }

    #[test]
    fn raw_log_lines_are_readable() {
        let line = r#"{"ts":"2026-10-18T08:00:00.000+02:00","dir":"rx","data":"OK\r\r"}"#;
        assert_eq!(format_log_line(line), "2026-10-18T08:00:00.000+02:00 rx   OK\\r\\r");
        assert_eq!(format_log_line("not json"), "not json");
    }

    #[test]
    fn only_plain_log_names_are_accepted() {
        assert!(is_plain_log_name("raw-20261018-080000.jsonl"));
        for bad in [
            "../raw-x.jsonl",
            "logs/raw-x.jsonl",
            "/etc/passwd",
            "/tmp/raw-x.jsonl",
            "raw-..jsonl",
            "..\\raw-x.jsonl",
            "C:raw-x.jsonl",
            "secret.txt",
            "raw-x.json",
            "",
            "..",
        ] {
            assert!(!is_plain_log_name(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn report_never_includes_files_outside_the_logs_directory() {
        let dir = std::env::temp_dir().join(format!("fordiag-report-evil-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("logs")).unwrap();
        // A file outside logs/ that a tampered result points at.
        std::fs::write(dir.join("secret.txt"), "TOP-SECRET-CONTENT\n").unwrap();
        let mut test = rest_test("test");
        test.raw_log = Some("../secret.txt".into());
        test.save(&dir).unwrap();

        let input = load(&dir, None, true).unwrap();
        assert!(input.raw_logs[0].lines.is_err());
        let md = render(&input);
        assert!(!md.contains("TOP-SECRET-CONTENT"));
        assert!(md.contains("file name rejected"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_from_directory_includes_raw_logs() {
        let dir = std::env::temp_dir().join(format!("fordiag-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        rest_test("test").save(&dir).unwrap();
        std::fs::create_dir_all(dir.join("logs")).unwrap();
        std::fs::write(
            dir.join("logs/raw-20261018-080000.jsonl"),
            "{\"ts\":\"t\",\"dir\":\"tx\",\"data\":\"ATRV\"}\n",
        )
        .unwrap();

        let with_raw = load(&dir, None, true).unwrap();
        assert!(with_raw.battery.rest.is_some());
        assert_eq!(with_raw.raw_logs.len(), 1);
        let md = render(&with_raw);
        assert!(md.contains("## Appendix: raw logs"));
        assert!(md.contains("### raw-20261018-080000.jsonl (battery: rest)"));
        assert!(md.contains("t tx   ATRV"));

        let without = load(&dir, None, false).unwrap();
        assert!(without.raw_logs.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
