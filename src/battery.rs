//! Batteritestar basert på `ATRV` (spenninga på OBD-kontakten, pinne 16).
//!
//! Avgrensingar som gjeld alle testane:
//! * ELM327 måler med 0,1 V oppløysing, og billege klonar kan bomme med
//!   0,1–0,2 V. Samanlikn gjerne éin gong med multimeter på batteripolane.
//! * Spenning seier noko om ladenivå og om batteriet toler startstraum, men
//!   kan ikkje måle kapasitet (Ah). Ein ekte batteritest krev ein
//!   belastnings- eller konduktanstestar.
//!
//! Analysefunksjonane er reine funksjonar av ei liste [`Sample`], så dei kan
//! testast utan bil.

use crate::elm::Elm;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Éi spenningsmåling.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Sample {
    /// Millisekund sidan målinga starta.
    pub t_ms: u64,
    pub volts: f32,
}

/// Formater spenning med desimalkomma: 12.456 → «12,46 V».
pub fn fmt_v(v: f32) -> String {
    format!("{v:.2} V").replace('.', ",")
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

/// Les `ATRV` gjentekne gonger i `duration`.
///
/// * `interval = None`: så raskt som mogleg (for starttest).
/// * `on_sample` blir kalla for kvar måling (for live-utskrift).
///
/// Enkeltfeil (t.d. at ELM-brikka mistar straum eit augneblink under start)
/// blir tolererte; fem feil på rad avbryt.
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
                    bail!("for mange feil på rad under måling: {e:#}");
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
        bail!("fekk ingen spenningsmålingar");
    }
    Ok(samples)
}

// ---------------------------------------------------------------------------
// Statistikk
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

/// Stigningstal (V per minutt) med minste kvadrats metode.
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

/// Lengste pause mellom to målingar (ms).
fn max_gap_ms(samples: &[Sample]) -> u64 {
    samples.windows(2).map(|w| w[1].t_ms - w[0].t_ms).max().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Kvilespenning
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestVerdict {
    /// ≥ 12,6 V
    Full,
    /// 12,4–12,5 V
    Ok,
    /// < 12,4 V
    Low,
    /// Over 13 V: motoren går eller ladar er tilkopla – ikkje kvilespenning.
    NotResting,
}

impl RestVerdict {
    pub fn text(self) -> &'static str {
        match self {
            RestVerdict::Full => "Fullt og friskt (≥ 12,6 V)",
            RestVerdict::Ok => "Greitt, men ikkje heilt fullt (12,4–12,5 V)",
            RestVerdict::Low => "Utlada eller svekka batteri (< 12,4 V)",
            RestVerdict::NotResting => "Ugyldig måling: over 13 V, så motoren går eller ein ladar er kopla til",
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
    // Samanlikn med litt slakk, fordi ELM-en rundar til 0,1 V og medianen
    // kan hamne midt mellom (t.d. 12,55).
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
            "Spenninga var over 13 V: motoren går eller ein ladar er kopla til. Dette er ikkje kvilespenning.".into(),
        );
    }
    if max - min > 0.3 {
        warnings.push(format!(
            "Spenninga varierte {} under målinga – noko i bilen trekte straum. Vent til bilen har sovna (10–30 min med dørene lukka) og mål på nytt.",
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
// Starttest
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrankVerdict {
    /// Lågaste ≥ 10,0 V
    Healthy,
    /// 9,6–10,0 V
    Borderline,
    /// < 9,6 V
    Weak,
    /// Fann ikkje noko tydeleg spenningsfall.
    NoCrankDetected,
}

impl CrankVerdict {
    pub fn text(self) -> &'static str {
        match self {
            CrankVerdict::Healthy => "Friskt: spenninga heldt seg på ca. 10 V eller meir under start",
            CrankVerdict::Borderline => "Grensetilfelle: lågaste spenning var mellom 9,6 og 10 V",
            CrankVerdict::Weak => "Svakt: spenninga fall tydeleg under 9,6 V under start",
            CrankVerdict::NoCrankDetected => "Fann ikkje noko startfall – vart motoren starta medan målinga gjekk?",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrankResult {
    /// Spenning før start (median av første 1,5 s).
    pub before: f32,
    /// Lågaste måling.
    pub min: f32,
    /// Når lågaste måling kom (ms).
    pub min_t_ms: u64,
    /// Spenning på slutten (median av siste 2 s).
    pub after: f32,
    pub engine_running_after: bool,
    /// Snittid mellom målingar (ms).
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

    // Eit startfall er minst 1 V under spenninga før start.
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
            "Målingane kom berre kvar {:.0}. ms; det verkelege botnpunktet kan vere litt lågare enn målt.",
            mean_interval_ms
        ));
    }
    if before > 13.0 {
        warnings.push("Spenninga før start var over 13 V – gjekk motoren alt, eller var ein ladar kopla til?".into());
    }
    let gap = max_gap_ms(samples);
    if gap > 500 {
        warnings.push(format!(
            "Hol på {gap} ms i målingane – ELM-brikka kan ha mista straumen under start. Botnpunktet kan då vere lågare enn målt."
        ));
    }
    if crank_detected && !(after >= 13.0) {
        warnings.push("Spenninga etter start er under 13 V – starta motoren, og ladar generatoren? (Ford-smartlading kan òg halde spenninga nede ei stund.)".into());
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
// Lading
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChargingVerdict {
    /// Snitt 13,2–14,8 V og ingen toppar over 15 V.
    Normal,
    /// Aldri over 13,0 V.
    Low,
    /// Over 15,0 V.
    High,
    /// Anna mønster, sjå tal og trend.
    Variable,
}

impl ChargingVerdict {
    pub fn text(self) -> &'static str {
        match self {
            ChargingVerdict::Normal => "Innanfor vanleg ladeområde",
            ChargingVerdict::Low => {
                "Låg: spenninga var aldri over 13 V. Anten ladar ikkje generatoren, eller smartladinga held igjen. Gjenta med store forbrukarar på (lys, vifte, bakrutevarme)."
            }
            ChargingVerdict::High => "Uvanleg høg: over 15 V. Bør sjekkast.",
            ChargingVerdict::Variable => {
                "Varierande. Ford si smartlading veksla truleg mellom lading og kvile; sjå min/maks og trend."
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChargingResult {
    pub min: f32,
    pub max: f32,
    pub mean: f32,
    /// V per minutt (positiv = stigande).
    pub slope_v_per_min: f32,
    /// Del av tida (0–1) med spenning ≥ 13,2 V.
    pub share_above_13_2: f32,
    pub verdict: ChargingVerdict,
    pub warnings: Vec<String>,
}

impl ChargingResult {
    pub fn trend_text(&self) -> &'static str {
        if self.slope_v_per_min > 0.05 {
            "stigande"
        } else if self.slope_v_per_min < -0.05 {
            "fallande"
        } else {
            "stabil"
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
        warnings.push("Målinga var kortare enn eitt minutt; trenden er usikker.".into());
    }
    if min < 12.5 {
        warnings.push(format!(
            "Spenninga var nede i {} med motoren i gang – batteriet leverte straum i staden for å bli lada.",
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
// Lagring av resultat (brukt av `battery --summary` og seinare `report`)
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
    /// Kva transport som vart brukt (t.d. «mock» eller «/dev/ttyUSB0 @ 38400 baud»).
    pub source: String,
    pub analysis: Analysis,
    pub samples: Vec<Sample>,
}

impl BatteryTest {
    /// Lagre som `<dir>/battery-<modus>-ÅÅÅÅMMDD-TTMMSS.json`.
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

    /// Finn siste lagra test av kvar type i `dir`.
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
        // Filnamna inneheld tidsstempel, så sortering gjev kronologisk rekkjefølgje.
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

/// Samla vurdering ut frå dei testane vi har. Returnerer linjer med tekst.
pub fn overall_assessment(
    rest: Option<&RestResult>,
    crank: Option<&CrankResult>,
    charging: Option<&ChargingResult>,
) -> Vec<String> {
    let mut out = Vec::new();
    let crank_v = crank.map(|c| c.verdict).filter(|v| *v != CrankVerdict::NoCrankDetected);

    match (rest.map(|r| r.verdict), crank_v) {
        (Some(RestVerdict::Low), Some(CrankVerdict::Weak | CrankVerdict::Borderline)) => out.push(
            "Både kvilespenning og starttest er svake. Lad batteriet fullt med smartladaren, la det kvile 12–24 timar og gjenta. \
             Er resultata like, er batteriet truleg skada (to djupe utladingar kan gje varig skade på eit EFB-batteri) – få det testa med belastningstestar."
                .into(),
        ),
        (Some(RestVerdict::Low), Some(CrankVerdict::Healthy)) => out.push(
            "Kvilespenninga er låg, men starten er grei. Batteriet er truleg berre utlada. Lad fullt og mål kvilespenning på nytt etter 12–24 timar."
                .into(),
        ),
        (_, Some(CrankVerdict::Weak)) => out.push(
            "Starttesten er svak sjølv om kvilespenninga ser grei ut. Det tyder på redusert kapasitet eller høg indre motstand. Få batteriet belastningstesta."
                .into(),
        ),
        (Some(RestVerdict::Full | RestVerdict::Ok), Some(CrankVerdict::Healthy)) => out.push(
            "Kvilespenning og starttest ser normale ut. Batteriet verkar friskt ut frå spenninga.".into(),
        ),
        _ => {}
    }

    if let Some(c) = charging {
        match c.verdict {
            ChargingVerdict::Normal => out.push("Ladespenninga ser normal ut.".into()),
            ChargingVerdict::Low => out.push(
                "Ladespenninga er låg. Det kan vere smartlading som held igjen fordi batteriovervakinga (BMS) trur batteriet er fullt/ukjent – eller ein ladefeil. Sjå feilkodar (fase 3).".into(),
            ),
            ChargingVerdict::High => out.push("Ladespenninga er høg (over 15 V) og bør sjekkast.".into()),
            ChargingVerdict::Variable => out.push("Ladespenninga varierer, som er vanleg med Ford si smartlading.".into()),
        }
    }

    if out.is_empty() {
        out.push("For få testar til ei samla vurdering. Køyr `--rest`, `--crank` og `--charging`.".into());
    }
    out.push(
        "Merk: Spenningstestar måler ikkje kapasitet. Dei forklarer heller ikkje åleine symptoma på SYNC/12 V-uttak; \
         feilkodar frå BCM/ACM/APIM (fase 3) vil seie meir om straumsparing."
            .into(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lag målingar med fast intervall.
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
    fn kvile_grenser() {
        assert_eq!(
            analyse_rest(&series(500, &[12.6, 12.6, 12.7])).verdict,
            RestVerdict::Full
        );
        assert_eq!(
            analyse_rest(&series(500, &[12.5, 12.6, 12.5, 12.6])).verdict,
            RestVerdict::Ok
        ); // median 12,55
        assert_eq!(analyse_rest(&series(500, &[12.4, 12.4, 12.5])).verdict, RestVerdict::Ok);
        assert_eq!(
            analyse_rest(&series(500, &[12.3, 12.3, 12.4])).verdict,
            RestVerdict::Low
        );
    }

    #[test]
    fn kvile_varslar_om_lading() {
        let r = analyse_rest(&series(500, &[14.1, 14.2, 14.1]));
        assert_eq!(r.verdict, RestVerdict::NotResting);
        assert!(r.warnings.iter().any(|w| w.contains("ikkje kvilespenning")));
    }

    fn crank_profile(min: f32) -> Vec<Sample> {
        let mut v = vec![12.4; 40]; // 2 s før start (50 ms intervall)
        v.extend([11.0, min, min + 0.3, min + 0.5, 11.5, 12.5, 13.5]);
        v.extend(vec![14.3; 60]); // 3 s etter
        series(50, &v)
    }

    #[test]
    fn start_friskt() {
        let r = analyse_crank(&crank_profile(10.2));
        assert_eq!(r.verdict, CrankVerdict::Healthy);
        assert!((r.min - 10.2).abs() < 1e-4);
        assert!(r.engine_running_after);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn start_grense_og_svak() {
        assert_eq!(analyse_crank(&crank_profile(9.8)).verdict, CrankVerdict::Borderline);
        assert_eq!(analyse_crank(&crank_profile(9.6)).verdict, CrankVerdict::Borderline);
        assert_eq!(analyse_crank(&crank_profile(9.1)).verdict, CrankVerdict::Weak);
    }

    #[test]
    fn start_ikkje_oppdaga() {
        let r = analyse_crank(&series(50, &[12.4; 100]));
        assert_eq!(r.verdict, CrankVerdict::NoCrankDetected);
    }

    #[test]
    fn start_hol_i_maalingane() {
        let mut s = crank_profile(10.5);
        for x in s.iter_mut().skip(42) {
            x.t_ms += 800;
        }
        assert!(analyse_crank(&s).warnings.iter().any(|w| w.contains("Hol")));
    }

    #[test]
    fn lading_normal_og_trend() {
        // 3 minutt, kvart sekund, frå 14,4 ned mot 13,9.
        let v: Vec<f32> = (0..180).map(|i| 14.4 - i as f32 * 0.5 / 180.0).collect();
        let r = analyse_charging(&series(1000, &v));
        assert_eq!(r.verdict, ChargingVerdict::Normal);
        assert_eq!(r.trend_text(), "fallande");
        assert!(r.slope_v_per_min < -0.1 && r.slope_v_per_min > -0.25);
    }

    #[test]
    fn lading_laag_og_hoeg() {
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
    fn samla_vurdering() {
        let rest = analyse_rest(&series(500, &[12.2; 5]));
        let crank = analyse_crank(&crank_profile(10.4));
        let text = overall_assessment(Some(&rest), Some(&crank), None).join("\n");
        assert!(text.contains("truleg berre utlada"));
    }

    #[test]
    fn lagre_og_laste() {
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
    fn desimalkomma() {
        assert_eq!(fmt_v(12.456), "12,46 V");
    }
}
