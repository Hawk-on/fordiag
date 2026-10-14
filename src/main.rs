//! fordiag – les-berre diagnoseverktøy for Ford S-Max via ELM327.
//!
//! Sjå README.md for bruk. Tryggleik: alt som blir sendt til bilen går
//! gjennom tillatslista i `command.rs`.

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
#[command(name = "fordiag", version, about = "Les-berre diagnose for Ford S-Max via ELM327")]
struct Cli {
    /// Seriellport (eller pseudoterminal frå ein ELM327-emulator)
    #[arg(long, global = true, default_value = "/dev/ttyUSB0")]
    port: String,

    /// Tving baudrate (elles: prøv 38400, så 115200)
    #[arg(long, global = true)]
    baud: Option<u32>,

    /// Bruk innebygd simulator i staden for kabel og bil
    #[arg(long, global = true)]
    mock: bool,

    /// Mappe for loggar og resultat
    #[arg(long, global = true, default_value = "fordiag-data")]
    data_dir: PathBuf,

    /// Ikkje vent på Enter før målingar
    #[arg(long, short = 'y', global = true)]
    yes: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Kople til ELM327, køyr init og vis versjon, protokoll og spenning
    Connect {
        /// Buss: hs (500 kbit/s) eller ms (125 kbit/s)
        #[arg(long, value_enum, default_value = "hs")]
        bus: Bus,
        /// Send òg OBD «01 00» for å sjå om motorstyringa svarar (krev tenning på)
        #[arg(long)]
        probe: bool,
    },
    /// Batteritestar basert på spenninga på OBD-kontakten
    Battery(BatteryArgs),
    /// Sjekk ein kommando mot tillatslista utan å sende han
    Check {
        /// T.d. «19 02 FF» eller «ATSH 7E0»
        text: String,
    },
}

#[derive(Args)]
struct BatteryArgs {
    #[command(flatten)]
    mode: BatteryMode,
    /// Kor lenge å måle i sekund (standard: rest 10, crank 15)
    #[arg(long)]
    seconds: Option<u64>,
    /// Kor lenge å måle lading i minutt
    #[arg(long, default_value_t = 3)]
    minutes: u64,
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct BatteryMode {
    /// Kvilespenning (bilen har stått i fleire timar)
    #[arg(long)]
    rest: bool,
    /// Lågaste spenning under start
    #[arg(long)]
    crank: bool,
    /// Ladespenning med motoren i gang
    #[arg(long)]
    charging: bool,
    /// Samla vurdering av siste lagra testar
    #[arg(long)]
    summary: bool,
}

/// Kva slags simulert bil `--mock` skal late som.
#[derive(Clone, Copy)]
enum MockScenario {
    Rest,
    Crank,
    Charging,
    Generic,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Feil: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Check { text } => {
            match Command::parse(text) {
                Ok(c) => println!("TILLATEN: {:?} → sendast som «{}»", c, c.to_wire()),
                Err(e) => println!("AVVIST: {e}"),
            }
            Ok(())
        }
        Cmd::Connect { bus, probe } => cmd_connect(&cli, *bus, *probe),
        Cmd::Battery(args) => cmd_battery(&cli, args),
    }
}

/// Opne kabelen (eller simulatoren) med rålogg. Returnerer òg loggen.
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

/// Spør brukaren og vent på Enter (hoppast over med `--yes` eller `--mock`).
fn wait_enter(cli: &Cli, prompt: &str) -> Result<()> {
    println!("{prompt}");
    if cli.yes || cli.mock {
        return Ok(());
    }
    print!("Trykk Enter når du er klar … ");
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
        wait_enter(cli, "MS-CAN: slå brytaren på kabelen over til MS (125 kbit/s) først.")?;
    }
    let (mut elm, log) = open(cli, MockScenario::Generic)?;
    println!("Tilkopla: {}", elm.describe());

    elm.init(bus)?;
    println!("ELM-versjon: {}", elm.identify()?);
    println!("Protokoll:   {}", elm.describe_protocol()?);
    println!("Spenning:    {}", fmt_v(elm.voltage()?));

    if probe {
        println!("\nSender OBD «01 00» (støtta PID-ar) …");
        match elm.diag(DiagRequest::Obd(ObdRequest::CurrentData { pid: 0x00 }))? {
            DiagReply::Data(lines) => {
                println!("Svar:");
                for l in lines {
                    println!("  {l}");
                }
            }
            DiagReply::NoData => println!("NO DATA – ingen modul svarte. Er tenninga på, og står brytaren rett?"),
        }
    }
    println!("\nRålogg: {}", log.path().display());
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

    // Instruksjonar til brukaren før vi opnar kabelen.
    match scenario {
        MockScenario::Rest => wait_enter(
            cli,
            "KVILESPENNING: Bilen skal ha stått i fleire timar utan ladar og utan bruk.\n\
             Ikkje slå på tenninga. Har du nett opna døra for å kople til kabelen, vent\n\
             gjerne 10–30 min med dørene lukka så bilen sovnar att.",
        )?,
        MockScenario::Crank => wait_enter(
            cli,
            &format!(
                "STARTTEST: Set deg i bilen. Målinga varer i {} s.\n\
                 Start motoren 2–3 sekund etter at det står «START NO».",
                args.seconds.unwrap_or(15)
            ),
        )?,
        _ => wait_enter(
            cli,
            "LADETEST: Start motoren og la han gå på tomgang heile målinga.\n\
             Ikkje slå av motoren før målinga er ferdig.",
        )?,
    }

    let (mut elm, log) = open(cli, scenario)?;
    // Berre grunnleggjande init: ATRV sender ingenting på CAN-bussen.
    elm.reset()?;
    let started = chrono::Local::now();

    let (samples, analysis) = match scenario {
        MockScenario::Rest => {
            let secs = args.seconds.unwrap_or(10);
            println!("Måler i {secs} s …");
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
            println!(">>> START NO <<<  (måler i {secs} s)");
            let s = battery::sample_voltage(&mut elm, Duration::from_secs(secs), None, |_| {})?;
            let r = battery::analyse_crank(&s);
            print_crank(&r, s.len());
            (s, Analysis::Crank(r))
        }
        _ => {
            let secs = args.seconds.unwrap_or(args.minutes * 60);
            println!("Måler i {secs} s, éi måling per sekund …");
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
    println!("\nResultat lagra: {}", path.display());
    println!("Rålogg:         {}", log.path().display());
    Ok(())
}

fn print_warnings(w: &[String]) {
    for x in w {
        println!("  ! {x}");
    }
}

fn print_rest(r: &battery::RestResult, n: usize) {
    println!(
        "\nKvilespenning ({n} målingar): median {} (min {}, maks {})",
        fmt_v(r.median),
        fmt_v(r.min),
        fmt_v(r.max)
    );
    println!("Vurdering: {}", r.verdict.text());
    print_warnings(&r.warnings);
}

fn print_crank(r: &battery::CrankResult, n: usize) {
    println!("\nStarttest ({n} målingar, snitt {:.0} ms mellom):", r.mean_interval_ms);
    println!("  Før start:  {}", fmt_v(r.before));
    println!(
        "  Lågaste:    {} (etter {} s)",
        fmt_v(r.min),
        format!("{:.1}", r.min_t_ms as f32 / 1000.0).replace('.', ",")
    );
    println!("  Etter:      {}", fmt_v(r.after));
    println!("Vurdering: {}", r.verdict.text());
    print_warnings(&r.warnings);
}

fn print_charging(r: &battery::ChargingResult, n: usize) {
    println!("\nLadetest ({n} målingar):");
    println!(
        "  Min / snitt / maks: {} / {} / {}",
        fmt_v(r.min),
        fmt_v(r.mean),
        fmt_v(r.max)
    );
    println!(
        "  Trend: {} ({} V/min), {:.0} % av tida ≥ 13,2 V",
        r.trend_text(),
        format!("{:+.2}", r.slope_v_per_min).replace('.', ","),
        r.share_above_13_2 * 100.0
    );
    println!("Vurdering: {}", r.verdict.text());
    print_warnings(&r.warnings);
}

fn battery_summary(cli: &Cli) -> Result<()> {
    let latest = BatteryTest::load_latest(&cli.data_dir)?;
    println!("Siste lagra batteritestar i {}:", cli.data_dir.display());
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
    let when = |t: &BatteryTest| format!("{} [{}]", t.started.format("%d.%m.%Y %H:%M"), t.source);
    match rest {
        Some((t, r)) => println!("  Kvile:  {} – {}  ({})", fmt_v(r.median), r.verdict.text(), when(t)),
        None => println!("  Kvile:  ikkje målt"),
    }
    match crank {
        Some((t, r)) => println!(
            "  Start:  lågaste {} – {}  ({})",
            fmt_v(r.min),
            r.verdict.text(),
            when(t)
        ),
        None => println!("  Start:  ikkje målt"),
    }
    match charging {
        Some((t, r)) => println!(
            "  Lading: snitt {} – {}  ({})",
            fmt_v(r.mean),
            r.verdict.text(),
            when(t)
        ),
        None => println!("  Lading: ikkje målt"),
    }
    println!("\nSamla vurdering:");
    for line in battery::overall_assessment(rest.map(|x| x.1), crank.map(|x| x.1), charging.map(|x| x.1)) {
        println!("  • {line}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Simulator for --mock
// ---------------------------------------------------------------------------

/// Enkel deterministisk «støy» på ±0,05 V, så simulerte kurver ser naturlege ut.
fn wobble(t: f32) -> f32 {
    ((t * 7.3).sin() + (t * 2.1).cos()) * 0.025
}

fn mock_for(scenario: MockScenario) -> MockTransport {
    // Litt forseinking per kommando, som på ein ekte kabel.
    let m = MockTransport::elm_defaults().with_latency(Duration::from_millis(25));
    match scenario {
        MockScenario::Rest => m.with_voltage(Box::new(|t| 12.45 + wobble(t.as_secs_f32()))),
        MockScenario::Crank => m.with_voltage(Box::new(|t| {
            let s = t.as_secs_f32();
            match s {
                s if s < 3.0 => 12.3 + wobble(s),                         // tenning på
                s if s < 3.2 => 12.3 - (s - 3.0) / 0.2 * 2.2,             // startmotor slår inn
                s if s < 4.0 => 10.3 + (s - 3.2) * 0.4 + wobble(s) * 2.0, // sveiv
                s if s < 5.0 => 10.6 + (s - 4.0) * 3.7,                   // motoren tek
                s => 14.3 + wobble(s),                                    // lading
            }
        })),
        MockScenario::Charging => m.with_voltage(Box::new(|t| {
            let s = t.as_secs_f32();
            // Smartlading: høg i starten, fell sakte, med eit kvileparti.
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
