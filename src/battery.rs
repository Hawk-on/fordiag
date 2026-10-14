//! Battery tests based on `ATRV` (the voltage on the OBD connector, pin 16).
//!
//! Limitations that apply to all tests:
//! * The ELM327 measures with 0.1 V resolution, and cheap clones can be off by
//!   0.1–0.2 V. Compare once with a multimeter on the battery terminals.
//! * Voltage says something about state of charge and whether the battery can
//!   handle cranking current, but cannot measure capacity (Ah). A real battery
//!   test requires a load or conductance tester.
//!
//! The analysis functions are pure functions of a list of [`Sample`]s, so they
//! can be tested without a car.

use crate::elm::Elm;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// One voltage measurement.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Sample {
    /// Milliseconds since the measurement started.
    pub t_ms: u64,
    pub volts: f32,
}

/// Format a voltage with two decimals: 12.456 → "12.46 V".
pub fn fmt_v(v: f32) -> String {
    format!("{v:.2} V")
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

/// Read `ATRV` repeatedly for `duration`.
///
/// * `interval = None`: as fast as possible (for the crank test).
/// * `on_sample` is called for each measurement (for live output).
///
/// Single errors (e.g. the ELM chip briefly losing power during cranking)
/// are tolerated; five errors in a row abort.
pub fn sample_voltage(
    elm: &mut Elm,
    duration: Duration,
    interval: Option<Duration>,
    mut on_sample: impl FnMut(&Sample),
) -> Result<Vec<Sample>> {
    let start = Instant::now();
    let mut samples = Vec::new();
    let mut errors_in_row = 0;
    while start.elapsed() < duration {
        let t0 = Instant::now();
        match elm.voltage() {
            Ok(v) => {
                errors_in_row = 0;
                let s = Sample {
                    t_ms: start.elapsed().as_millis() as u64,
                    volts: v,
                };
                on_sample(&s);
                samples.push(s);
            }
            Err(e) => {
                errors_in_row += 1;
                if errors_in_row >= 5 {
                    bail!("too many consecutive errors during measurement: {e:#}");
                }
            }
        }
        if let Some(iv) = interval {
            if let Some(rest) = iv.checked_sub(t0.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    }
    if samples.is_empty() {
        bail!("got no voltage measurements");
    }
    Ok(samples)
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

fn median(values: &[f32]) -> f32 {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n == 0 {
        return f32::NAN;
    }
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn mean(values: &[f32]) -> f32 {
    values.iter().sum::<f32>() / values.len() as f32
}

fn min_max(samples: &[Sample]) -> (f32, f32) {
    samples.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), s| {
        (lo.min(s.volts), hi.max(s.volts))
    })
}

/// Slope (V per minute) by least squares.
fn slope_v_per_min(samples: &[Sample]) -> f32 {
    if samples.len() < 2 {
        return 0.0;
    }
    let xs: Vec<f64> = samples.iter().map(|s| s.t_ms as f64 / 60_000.0).collect();
    let ys: Vec<f64> = samples.iter().map(|s| s.volts as f64).collect();
    let mx = xs.iter().sum::<f64>() / xs.len() as f64;
    let my = ys.iter().sum::<f64>() / ys.len() as f64;
    let num: f64 = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let den: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
    if den == 0.0 { 0.0 } else { (num / den) as f32 }
}

/// Longest gap between two measurements (ms).
fn max_gap_ms(samples: &[Sample]) -> u64 {
    samples.windows(2).map(|w| w[1].t_ms - w[0].t_ms).max().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Resting voltage
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestVerdict {
    /// ≥ 12.6 V
    Full,
    /// 12.4–12.5 V
    Ok,
    /// < 12.4 V
    Low,
    /// Above 13 V: engine running or charger connected – not a resting voltage.
    NotResting,
}

impl RestVerdict {
    pub fn text(self) -> &'static str {
        match self {
            RestVerdict::Full => "Fully charged and healthy (≥ 12.6 V)",
            RestVerdict::Ok => "OK, but not fully charged (12.4–12.5 V)",
            RestVerdict::Low => "Discharged or weakened battery (< 12.4 V)",
            RestVerdict::NotResting => {
                "Invalid measurement: above 13 V, so the engine is running or a charger is connected"
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestResult {
    pub median: f32,
    pub min: f32,
    pub max: f32,
    pub verdict: RestVerdict,
    pub warnings: Vec<String>,
}

pub fn analyse_rest(samples: &[Sample]) -> RestResult {
    let volts: Vec<f32> = samples.iter().map(|s| s.volts).collect();
    let med = median(&volts);
    let (min, max) = min_max(samples);
    // Compare with a little slack, because the ELM rounds to 0.1 V and the
    // median can land in between (e.g. 12.55).
    let verdict = if med > 13.0 {
        RestVerdict::NotResting
    } else if med >= 12.6 - 0.001 {
        RestVerdict::Full
    } else if med >= 12.4 - 0.001 {
        RestVerdict::Ok
    } else {
        RestVerdict::Low
    };
    let mut warnings = Vec::new();
    if max > 13.0 {
        warnings.push(
            "The voltage was above 13 V: the engine is running or a charger is connected. This is not a resting voltage.".into(),
        );
    }
    if max - min > 0.3 {
        warnings.push(format!(
            "The voltage varied by {} during the measurement – something in the car was drawing current. Wait until the car has gone to sleep (10–30 min with the doors closed) and measure again.",
            fmt_v(max - min)
        ));
    }
    RestResult {
        median: med,
        min,
        max,
        verdict,
        warnings,
    }
}

// ---------------------------------------------------------------------------
// Crank test
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrankVerdict {
    /// Lowest ≥ 10.0 V
    Healthy,
    /// 9.6–10.0 V
    Borderline,
    /// < 9.6 V
    Weak,
    /// No clear voltage drop found.
    NoCrankDetected,
}

impl CrankVerdict {
    pub fn text(self) -> &'static str {
        match self {
            CrankVerdict::Healthy => "Healthy: the voltage stayed at about 10 V or more while cranking",
            CrankVerdict::Borderline => "Borderline: the lowest voltage was between 9.6 and 10 V",
            CrankVerdict::Weak => "Weak: the voltage dropped clearly below 9.6 V while cranking",
            CrankVerdict::NoCrankDetected => {
                "No cranking dip found – was the engine started while the measurement was running?"
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrankResult {
    /// Voltage before cranking (median of the first 1.5 s).
    pub before: f32,
    /// Lowest measurement.
    pub min: f32,
    /// When the lowest measurement occurred (ms).
    pub min_t_ms: u64,
    /// Voltage at the end (median of the last 2 s).
    pub after: f32,
    pub engine_running_after: bool,
    /// Mean time between measurements (ms).
    pub mean_interval_ms: f32,
    pub verdict: CrankVerdict,
    pub warnings: Vec<String>,
}

pub fn analyse_crank(samples: &[Sample]) -> CrankResult {
    let first_t = samples.first().map(|s| s.t_ms).unwrap_or(0);
    let last_t = samples.last().map(|s| s.t_ms).unwrap_or(0);

    let early: Vec<f32> = samples
        .iter()
        .filter(|s| s.t_ms <= first_t + 1500)
        .map(|s| s.volts)
        .collect();
    let late: Vec<f32> = samples
        .iter()
        .filter(|s| s.t_ms + 2000 >= last_t)
        .map(|s| s.volts)
        .collect();
    let before = median(&early);
    let after = median(&late);

    let min_sample = samples
        .iter()
        .copied()
        .min_by(|a, b| a.volts.total_cmp(&b.volts))
        .unwrap_or(Sample {
            t_ms: 0,
            volts: f32::NAN,
        });
    let mean_interval_ms = if samples.len() > 1 {
        (last_t - first_t) as f32 / (samples.len() - 1) as f32
    } else {
        f32::NAN
    };

    // A cranking dip is at least 1 V below the voltage before cranking.
    let crank_detected = before - min_sample.volts >= 1.0;
    let verdict = if !crank_detected {
        CrankVerdict::NoCrankDetected
    } else if min_sample.volts >= 10.0 - 0.001 {
        CrankVerdict::Healthy
    } else if min_sample.volts >= 9.6 - 0.001 {
        CrankVerdict::Borderline
    } else {
        CrankVerdict::Weak
    };

    let mut warnings = Vec::new();
    if mean_interval_ms > 100.0 {
        warnings.push(format!(
            "Measurements only arrived every {:.0} ms; the true minimum may be slightly lower than measured.",
            mean_interval_ms
        ));
    }
    if before > 13.0 {
        warnings.push(
            "The voltage before cranking was above 13 V – was the engine already running, or a charger connected?"
                .into(),
        );
    }
    let gap = max_gap_ms(samples);
    if gap > 500 {
        warnings.push(format!(
            "Gap of {gap} ms in the measurements – the ELM chip may have lost power while cranking. The minimum may then be lower than measured."
        ));
    }
    if crank_detected && !(after >= 13.0) {
        warnings.push("The voltage after cranking is below 13 V – did the engine start, and is the alternator charging? (Ford smart charging can also keep the voltage low for a while.)".into());
    }

    CrankResult {
        before,
        min: min_sample.volts,
        min_t_ms: min_sample.t_ms,
        after,
        engine_running_after: after >= 13.0,
        mean_interval_ms,
        verdict,
        warnings,
    }
}

// ---------------------------------------------------------------------------
// Charging
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChargingVerdict {
    /// Mean 13.2–14.8 V and no peaks above 15 V.
    Normal,
    /// Never above 13.0 V.
    Low,
    /// Above 15.0 V.
    High,
    /// Other pattern, see numbers and trend.
    Variable,
}

impl ChargingVerdict {
    pub fn text(self) -> &'static str {
        match self {
            ChargingVerdict::Normal => "Within the normal charging range",
            ChargingVerdict::Low => {
                "Low: the voltage never exceeded 13 V. Either the alternator is not charging, or smart charging is holding back. Repeat with large loads on (lights, blower, rear window heater)."
            }
            ChargingVerdict::High => "Unusually high: above 15 V. Should be checked.",
            ChargingVerdict::Variable => {
                "Variable. Ford smart charging probably alternated between charging and resting; see min/max and trend."
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChargingResult {
    pub min: f32,
    pub max: f32,
    pub mean: f32,
    /// V per minute (positive = rising).
    pub slope_v_per_min: f32,
    /// Share of the time (0–1) with voltage ≥ 13.2 V.
    pub share_above_13_2: f32,
    pub verdict: ChargingVerdict,
    pub warnings: Vec<String>,
}

impl ChargingResult {
    pub fn trend_text(&self) -> &'static str {
        if self.slope_v_per_min > 0.05 {
            "rising"
        } else if self.slope_v_per_min < -0.05 {
            "falling"
        } else {
            "stable"
        }
    }
}

pub fn analyse_charging(samples: &[Sample]) -> ChargingResult {
    let volts: Vec<f32> = samples.iter().map(|s| s.volts).collect();
    let (min, max) = min_max(samples);
    let mean = mean(&volts);
    let slope = slope_v_per_min(samples);
    let share = volts.iter().filter(|&&v| v >= 13.2).count() as f32 / volts.len() as f32;

    let verdict = if max > 15.0 {
        ChargingVerdict::High
    } else if max < 13.0 {
        ChargingVerdict::Low
    } else if (13.2..=14.8).contains(&mean) && min >= 12.8 {
        ChargingVerdict::Normal
    } else {
        ChargingVerdict::Variable
    };

    let mut warnings = Vec::new();
    if samples.last().map(|s| s.t_ms).unwrap_or(0) < 60_000 {
        warnings.push("The measurement was shorter than one minute; the trend is uncertain.".into());
    }
    if min < 12.5 {
        warnings.push(format!(
            "The voltage dropped to {} with the engine running – the battery was supplying current instead of being charged.",
            fmt_v(min)
        ));
    }
    ChargingResult {
        min,
        max,
        mean,
        slope_v_per_min: slope,
        share_above_13_2: share,
        verdict,
        warnings,
    }
}

// ---------------------------------------------------------------------------
// Storing results (used by `battery --summary` and later `report`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Analysis {
    Rest(RestResult),
    Crank(CrankResult),
    Charging(ChargingResult),
}

impl Analysis {
    pub fn mode_name(&self) -> &'static str {
        match self {
            Analysis::Rest(_) => "rest",
            Analysis::Crank(_) => "crank",
            Analysis::Charging(_) => "charging",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatteryTest {
    pub started: chrono::DateTime<chrono::Local>,
    /// Which transport was used (e.g. "mock" or "/dev/ttyUSB0 @ 38400 baud").
    pub source: String,
    pub analysis: Analysis,
    pub samples: Vec<Sample>,
}

impl BatteryTest {
    /// Save as `<dir>/battery-<mode>-YYYYMMDD-HHMMSS.json`.
    pub fn save(&self, dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let name = format!(
            "battery-{}-{}.json",
            self.analysis.mode_name(),
            self.started.format("%Y%m%d-%H%M%S")
        );
        let path = dir.join(name);
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(path)
    }

    /// Find the latest saved test of each type in `dir`.
    pub fn load_latest(dir: &Path) -> Result<Latest> {
        let mut latest = Latest::default();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(latest);
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("battery-") && n.ends_with(".json"))
            })
            .collect();
        // File names contain a timestamp, so sorting gives chronological order.
        files.sort();
        for path in files {
            let text = std::fs::read_to_string(&path)?;
            let test: BatteryTest = serde_json::from_str(&text)?;
            match test.analysis {
                Analysis::Rest(_) => latest.rest = Some(test),
                Analysis::Crank(_) => latest.crank = Some(test),
                Analysis::Charging(_) => latest.charging = Some(test),
            }
        }
        Ok(latest)
    }
}

#[derive(Debug, Default)]
pub struct Latest {
    pub rest: Option<BatteryTest>,
    pub crank: Option<BatteryTest>,
    pub charging: Option<BatteryTest>,
}

/// Overall assessment based on the tests we have. Returns lines of text.
pub fn overall_assessment(
    rest: Option<&RestResult>,
    crank: Option<&CrankResult>,
    charging: Option<&ChargingResult>,
) -> Vec<String> {
    let mut out = Vec::new();
    let crank_v = crank.map(|c| c.verdict).filter(|v| *v != CrankVerdict::NoCrankDetected);

    match (rest.map(|r| r.verdict), crank_v) {
        (Some(RestVerdict::Low), Some(CrankVerdict::Weak | CrankVerdict::Borderline)) => out.push(
            "Both resting voltage and crank test are weak. Fully charge the battery with the smart charger, let it rest 12–24 hours and repeat. \
             If the results are the same, the battery is probably damaged (two deep discharges can permanently damage an EFB battery) – have it tested with a load tester."
                .into(),
        ),
        (Some(RestVerdict::Low), Some(CrankVerdict::Healthy)) => out.push(
            "The resting voltage is low, but cranking is fine. The battery is probably just discharged. Charge fully and measure the resting voltage again after 12–24 hours."
                .into(),
        ),
        (_, Some(CrankVerdict::Weak)) => out.push(
            "The crank test is weak even though the resting voltage looks fine. This suggests reduced capacity or high internal resistance. Have the battery load-tested."
                .into(),
        ),
        (Some(RestVerdict::Full | RestVerdict::Ok), Some(CrankVerdict::Healthy)) => out.push(
            "Resting voltage and crank test look normal. Judging by voltage, the battery appears healthy.".into(),
        ),
        _ => {}
    }

    if let Some(c) = charging {
        match c.verdict {
            ChargingVerdict::Normal => out.push("The charging voltage looks normal.".into()),
            ChargingVerdict::Low => out.push(
                "The charging voltage is low. It may be smart charging holding back because the battery monitoring system (BMS) thinks the battery is full/unknown – or a charging fault. See DTCs (phase 3).".into(),
            ),
            ChargingVerdict::High => out.push("The charging voltage is high (above 15 V) and should be checked.".into()),
            ChargingVerdict::Variable => out.push("The charging voltage varies, which is common with Ford smart charging.".into()),
        }
    }

    if out.is_empty() {
        out.push("Too few tests for an overall assessment. Run `--rest`, `--crank` and `--charging`.".into());
    }
    out.push(
        "Note: Voltage tests do not measure capacity. Nor do they alone explain the SYNC/12 V outlet symptoms; \
         DTCs from BCM/ACM/APIM (phase 3) will tell more about load shedding."
            .into(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build measurements with a fixed interval.
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

    #[test]
    fn rest_thresholds() {
        assert_eq!(
            analyse_rest(&series(500, &[12.6, 12.6, 12.7])).verdict,
            RestVerdict::Full
        );
        assert_eq!(
            analyse_rest(&series(500, &[12.5, 12.6, 12.5, 12.6])).verdict,
            RestVerdict::Ok
        ); // median 12.55
        assert_eq!(analyse_rest(&series(500, &[12.4, 12.4, 12.5])).verdict, RestVerdict::Ok);
        assert_eq!(
            analyse_rest(&series(500, &[12.3, 12.3, 12.4])).verdict,
            RestVerdict::Low
        );
    }

    #[test]
    fn rest_warns_about_charging() {
        let r = analyse_rest(&series(500, &[14.1, 14.2, 14.1]));
        assert_eq!(r.verdict, RestVerdict::NotResting);
        assert!(r.warnings.iter().any(|w| w.contains("not a resting voltage")));
    }

    fn crank_profile(min: f32) -> Vec<Sample> {
        let mut v = vec![12.4; 40]; // 2 s before cranking (50 ms interval)
        v.extend([11.0, min, min + 0.3, min + 0.5, 11.5, 12.5, 13.5]);
        v.extend(vec![14.3; 60]); // 3 s after
        series(50, &v)
    }

    #[test]
    fn crank_healthy() {
        let r = analyse_crank(&crank_profile(10.2));
        assert_eq!(r.verdict, CrankVerdict::Healthy);
        assert!((r.min - 10.2).abs() < 1e-4);
        assert!(r.engine_running_after);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn crank_borderline_and_weak() {
        assert_eq!(analyse_crank(&crank_profile(9.8)).verdict, CrankVerdict::Borderline);
        assert_eq!(analyse_crank(&crank_profile(9.6)).verdict, CrankVerdict::Borderline);
        assert_eq!(analyse_crank(&crank_profile(9.1)).verdict, CrankVerdict::Weak);
    }

    #[test]
    fn crank_not_detected() {
        let r = analyse_crank(&series(50, &[12.4; 100]));
        assert_eq!(r.verdict, CrankVerdict::NoCrankDetected);
    }

    #[test]
    fn crank_gap_in_measurements() {
        let mut s = crank_profile(10.5);
        for x in s.iter_mut().skip(42) {
            x.t_ms += 800;
        }
        assert!(analyse_crank(&s).warnings.iter().any(|w| w.contains("Gap")));
    }

    #[test]
    fn charging_normal_and_trend() {
        // 3 minutes, every second, from 14.4 down towards 13.9.
        let v: Vec<f32> = (0..180).map(|i| 14.4 - i as f32 * 0.5 / 180.0).collect();
        let r = analyse_charging(&series(1000, &v));
        assert_eq!(r.verdict, ChargingVerdict::Normal);
        assert_eq!(r.trend_text(), "falling");
        assert!(r.slope_v_per_min < -0.1 && r.slope_v_per_min > -0.25);
    }

    #[test]
    fn charging_low_and_high() {
        assert_eq!(
            analyse_charging(&series(1000, &[12.7; 90])).verdict,
            ChargingVerdict::Low
        );
        assert_eq!(
            analyse_charging(&series(1000, &[14.5, 15.3, 14.5])).verdict,
            ChargingVerdict::High
        );
    }

    #[test]
    fn overall_assessment_text() {
        let rest = analyse_rest(&series(500, &[12.2; 5]));
        let crank = analyse_crank(&crank_profile(10.4));
        let text = overall_assessment(Some(&rest), Some(&crank), None).join("\n");
        assert!(text.contains("probably just discharged"));
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("fordiag-bat-{}", std::process::id()));
        let s = series(500, &[12.5; 4]);
        let test = BatteryTest {
            started: chrono::Local::now(),
            source: "test".into(),
            analysis: Analysis::Rest(analyse_rest(&s)),
            samples: s,
        };
        test.save(&dir).unwrap();
        let latest = BatteryTest::load_latest(&dir).unwrap();
        assert!(latest.rest.is_some() && latest.crank.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn formats_voltage() {
        assert_eq!(fmt_v(12.456), "12.46 V");
    }
}
