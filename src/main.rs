//! fordiag – read-only diagnostic tool for the Ford S-Max via ELM327.
//!
//! See README.md for usage. Safety: everything sent to the car goes through
//! the allowlist in `command.rs`.

mod battery;
mod command;
mod elm;
mod transport;

use anyhow::Result;
use battery::{Analysis, BatteryTest, fmt_v};
use clap::{Args, Parser, Subcommand};
use command::{Command, DiagRequest, ObdRequest};
use elm::{Bus, DiagReply, Elm};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;
use transport::{Logged, MockTransport, RawLog};

#[derive(Parser)]
#[command(
    name = "fordiag",
    version,
    about = "Read-only diagnostics for the Ford S-Max via ELM327",
    after_help = DISCLAIMER
)]
struct Cli {
    /// Serial port (or pseudo-terminal from an ELM327 emulator)
    #[arg(long, global = true, default_value = "/dev/ttyUSB0")]
    port: String,

    /// Force baud rate (otherwise: try 38400, then 115200)
    #[arg(long, global = true)]
    baud: Option<u32>,

    /// Use the built-in simulator instead of cable and car
    #[arg(long, global = true)]
    mock: bool,

    /// Directory for logs and results
    #[arg(long, global = true, default_value = "fordiag-data")]
    data_dir: PathBuf,

    /// Do not wait for Enter before measurements
    #[arg(long, short = 'y', global = true)]
    yes: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Connect to the ELM327, run init and show version, protocol and voltage
    Connect {
        /// Bus: hs (500 kbit/s) or ms (125 kbit/s)
        #[arg(long, value_enum, default_value = "hs")]
        bus: Bus,
        /// Also send OBD "01 00" to see if the engine control module responds (requires ignition on)
        #[arg(long)]
        probe: bool,
    },
    /// Battery tests based on the voltage at the OBD connector
    Battery(BatteryArgs),
    /// Check a command against the allowlist without sending it
    Check {
        /// E.g. "19 02 FF" or "ATSH 7E0"
        text: String,
    },
}

#[derive(Args)]
struct BatteryArgs {
    #[command(flatten)]
    mode: BatteryMode,
    /// How long to measure, in seconds (default: rest 10, crank 15)
    #[arg(long)]
    seconds: Option<u64>,
    /// How long to measure charging, in minutes
    #[arg(long, default_value_t = 3)]
    minutes: u64,
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct BatteryMode {
    /// Resting voltage (the car has been parked for several hours)
    #[arg(long)]
    rest: bool,
    /// Lowest voltage while cranking
    #[arg(long)]
    crank: bool,
    /// Charging voltage with the engine running
    #[arg(long)]
    charging: bool,
    /// Overall assessment of the latest saved tests
    #[arg(long)]
    summary: bool,
}

/// What kind of simulated car `--mock` should pretend to be.
#[derive(Clone, Copy)]
enum MockScenario {
    Rest,
    Crank,
    Charging,
    Generic,
}

/// Shown at the bottom of `--help`.
const DISCLAIMER: &str = "DISCLAIMER: Use at your own risk. fordiag is provided \"as is\", without warranty of any kind \
(see LICENSE). It is not affiliated with or endorsed by Ford. Do not operate it while driving.";

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Check { text } => {
            match Command::parse(text) {
                Ok(c) => println!("ALLOWED: {:?} → sent as \"{}\"", c, c.to_wire()),
                Err(e) => println!("REJECTED: {e}"),
            }
            Ok(())
        }
        Cmd::Connect { bus, probe } => cmd_connect(&cli, *bus, *probe),
        Cmd::Battery(args) => cmd_battery(&cli, args),
    }
}

/// Open the cable (or the simulator) with a raw log. Also returns the log.
fn open(cli: &Cli, scenario: MockScenario) -> Result<(Elm, RawLog)> {
    let log = RawLog::create(&cli.data_dir.join("logs"))?;
    let inner: Box<dyn transport::Transport> = if cli.mock {
        Box::new(mock_for(scenario))
    } else {
        elm::open_serial(&cli.port, cli.baud, &log)?
    };
    let elm = Elm::new(Box::new(Logged::new(inner, log.clone())));
    Ok((elm, log))
}

/// Prompt the user and wait for Enter (skipped with `--yes` or `--mock`).
fn wait_enter(cli: &Cli, prompt: &str) -> Result<()> {
    println!("{prompt}");
    if cli.yes || cli.mock {
        return Ok(());
    }
    print!("Press Enter when ready … ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// connect
// ---------------------------------------------------------------------------

fn cmd_connect(cli: &Cli, bus: Bus, probe: bool) -> Result<()> {
    if bus == Bus::Ms {
        wait_enter(cli, "MS-CAN: set the switch on the cable to MS (125 kbit/s) first.")?;
    }
    let (mut elm, log) = open(cli, MockScenario::Generic)?;
    println!("Connected: {}", elm.describe());

    elm.init(bus)?;
    println!("ELM version: {}", elm.identify()?);
    println!("Protocol:    {}", elm.describe_protocol()?);
    println!("Voltage:     {}", fmt_v(elm.voltage()?));

    if probe {
        println!("\nSending OBD \"01 00\" (supported PIDs) …");
        match elm.diag(DiagRequest::Obd(ObdRequest::CurrentData { pid: 0x00 }))? {
            DiagReply::Data(lines) => {
                println!("Response:");
                for l in lines {
                    println!("  {l}");
                }
            }
            DiagReply::NoData => {
                println!("NO DATA – no module responded. Is the ignition on, and is the switch in the right position?")
            }
        }
    }
    println!("\nRaw log: {}", log.path().display());
    Ok(())
}

// ---------------------------------------------------------------------------
// battery
// ---------------------------------------------------------------------------

fn cmd_battery(cli: &Cli, args: &BatteryArgs) -> Result<()> {
    let m = &args.mode;
    if m.summary {
        return battery_summary(cli);
    }

    let scenario = if m.rest {
        MockScenario::Rest
    } else if m.crank {
        MockScenario::Crank
    } else {
        MockScenario::Charging
    };

    // Instructions to the user before we open the cable.
    match scenario {
        MockScenario::Rest => wait_enter(
            cli,
            "RESTING VOLTAGE: The car should have been parked for several hours without a charger and unused.\n\
             Do not switch on the ignition. If you just opened the door to plug in the cable, wait\n\
             10–30 min with the doors closed so the car goes back to sleep.",
        )?,
        MockScenario::Crank => wait_enter(
            cli,
            &format!(
                "CRANK TEST: Sit in the car. The measurement lasts {} s.\n\
                 Start the engine 2–3 seconds after \"START NOW\" appears.",
                args.seconds.unwrap_or(15)
            ),
        )?,
        _ => wait_enter(
            cli,
            "CHARGING TEST: Start the engine and let it idle for the whole measurement.\n\
             Do not switch off the engine until the measurement is finished.",
        )?,
    }

    let (mut elm, log) = open(cli, scenario)?;
    // Basic init only: ATRV sends nothing on the CAN bus.
    elm.reset()?;
    let started = chrono::Local::now();

    let (samples, analysis) = match scenario {
        MockScenario::Rest => {
            let secs = args.seconds.unwrap_or(10);
            println!("Measuring for {secs} s …");
            let s = battery::sample_voltage(
                &mut elm,
                Duration::from_secs(secs),
                Some(Duration::from_millis(500)),
                |_| {},
            )?;
            let r = battery::analyse_rest(&s);
            print_rest(&r, s.len());
            (s, Analysis::Rest(r))
        }
        MockScenario::Crank => {
            let secs = args.seconds.unwrap_or(15);
            println!(">>> START NOW <<<  (measuring for {secs} s)");
            let s = battery::sample_voltage(&mut elm, Duration::from_secs(secs), None, |_| {})?;
            let r = battery::analyse_crank(&s);
            print_crank(&r, s.len());
            (s, Analysis::Crank(r))
        }
        _ => {
            let secs = args.seconds.unwrap_or(args.minutes * 60);
            println!("Measuring for {secs} s, one measurement per second …");
            let mut last_print = 0;
            let s = battery::sample_voltage(&mut elm, Duration::from_secs(secs), Some(Duration::from_secs(1)), |s| {
                if s.t_ms >= last_print {
                    println!("  {:>4} s: {}", s.t_ms / 1000, fmt_v(s.volts));
                    last_print = s.t_ms + 10_000;
                }
            })?;
            let r = battery::analyse_charging(&s);
            print_charging(&r, s.len());
            (s, Analysis::Charging(r))
        }
    };

    let test = BatteryTest {
        started,
        source: elm.describe(),
        analysis,
        samples,
    };
    let path = test.save(&cli.data_dir)?;
    println!("\nResult saved: {}", path.display());
    println!("Raw log:      {}", log.path().display());
    Ok(())
}

fn print_warnings(w: &[String]) {
    for x in w {
        println!("  ! {x}");
    }
}

fn print_rest(r: &battery::RestResult, n: usize) {
    println!(
        "\nResting voltage ({n} measurements): median {} (min {}, max {})",
        fmt_v(r.median),
        fmt_v(r.min),
        fmt_v(r.max)
    );
    println!("Verdict: {}", r.verdict.text());
    print_warnings(&r.warnings);
}

fn print_crank(r: &battery::CrankResult, n: usize) {
    println!(
        "\nCrank test ({n} measurements, mean {:.0} ms apart):",
        r.mean_interval_ms
    );
    println!("  Before:  {}", fmt_v(r.before));
    println!(
        "  Lowest:  {} (after {:.1} s)",
        fmt_v(r.min),
        r.min_t_ms as f32 / 1000.0
    );
    println!("  After:   {}", fmt_v(r.after));
    println!("Verdict: {}", r.verdict.text());
    print_warnings(&r.warnings);
}

fn print_charging(r: &battery::ChargingResult, n: usize) {
    println!("\nCharging test ({n} measurements):");
    println!(
        "  Min / mean / max: {} / {} / {}",
        fmt_v(r.min),
        fmt_v(r.mean),
        fmt_v(r.max)
    );
    println!(
        "  Trend: {} ({:+.2} V/min), {:.0} % of the time ≥ 13.2 V",
        r.trend_text(),
        r.slope_v_per_min,
        r.share_above_13_2 * 100.0
    );
    println!("Verdict: {}", r.verdict.text());
    print_warnings(&r.warnings);
}

fn battery_summary(cli: &Cli) -> Result<()> {
    let latest = BatteryTest::load_latest(&cli.data_dir)?;
    println!("Latest saved battery tests in {}:", cli.data_dir.display());
    let rest = latest.rest.as_ref().and_then(|t| match &t.analysis {
        Analysis::Rest(r) => Some((t, r)),
        _ => None,
    });
    let crank = latest.crank.as_ref().and_then(|t| match &t.analysis {
        Analysis::Crank(r) => Some((t, r)),
        _ => None,
    });
    let charging = latest.charging.as_ref().and_then(|t| match &t.analysis {
        Analysis::Charging(r) => Some((t, r)),
        _ => None,
    });
    let when = |t: &BatteryTest| format!("{} [{}]", t.started.format("%Y-%m-%d %H:%M"), t.source);
    match rest {
        Some((t, r)) => println!("  Rest:     {} – {}  ({})", fmt_v(r.median), r.verdict.text(), when(t)),
        None => println!("  Rest:     not measured"),
    }
    match crank {
        Some((t, r)) => println!(
            "  Crank:    lowest {} – {}  ({})",
            fmt_v(r.min),
            r.verdict.text(),
            when(t)
        ),
        None => println!("  Crank:    not measured"),
    }
    match charging {
        Some((t, r)) => println!(
            "  Charging: mean {} – {}  ({})",
            fmt_v(r.mean),
            r.verdict.text(),
            when(t)
        ),
        None => println!("  Charging: not measured"),
    }
    println!("\nOverall assessment:");
    for line in battery::overall_assessment(rest.map(|x| x.1), crank.map(|x| x.1), charging.map(|x| x.1)) {
        println!("  • {line}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Simulator for --mock
// ---------------------------------------------------------------------------

/// Simple deterministic "noise" of ±0.05 V, so simulated curves look natural.
fn wobble(t: f32) -> f32 {
    ((t * 7.3).sin() + (t * 2.1).cos()) * 0.025
}

fn mock_for(scenario: MockScenario) -> MockTransport {
    // A little delay per command, as on a real cable.
    let m = MockTransport::elm_defaults().with_latency(Duration::from_millis(25));
    match scenario {
        MockScenario::Rest => m.with_voltage(Box::new(|t| 12.45 + wobble(t.as_secs_f32()))),
        MockScenario::Crank => m.with_voltage(Box::new(|t| {
            let s = t.as_secs_f32();
            match s {
                s if s < 3.0 => 12.3 + wobble(s),                         // ignition on
                s if s < 3.2 => 12.3 - (s - 3.0) / 0.2 * 2.2,             // starter engages
                s if s < 4.0 => 10.3 + (s - 3.2) * 0.4 + wobble(s) * 2.0, // cranking
                s if s < 5.0 => 10.6 + (s - 4.0) * 3.7,                   // engine catches
                s => 14.3 + wobble(s),                                    // charging
            }
        })),
        MockScenario::Charging => m.with_voltage(Box::new(|t| {
            let s = t.as_secs_f32();
            // Smart charging: high at first, slowly falling, with a resting phase.
            let base = 14.5 - s / 180.0 * 0.6;
            if (100.0..130.0).contains(&s) {
                12.9 + wobble(s)
            } else {
                base + wobble(s)
            }
        })),
        MockScenario::Generic => m.on("0100", "SEARCHING...\r7E8 06 41 00 BE 3F A8 13\r\r"),
    }
}
