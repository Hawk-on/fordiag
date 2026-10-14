# CLAUDE.md

Rettleiing for Claude (og andre) som jobbar i dette repoet.

## Prosjektet

`fordiag` er eit **les-berre** diagnose-CLI i Rust for ein Ford S-Max Mk2 (CJ), reg.
2016, 2.0 TDCi 150 hk, automat, start/stopp. Det snakkar med bilen via ein ELM327
USB-kabel (`/dev/ttyUSB0`, HS/MS-brytar).

Hovudmålet er å finne ut om batteriet (EFB, 75 Ah, 730 A) er friskt etter to djupe
utladingar, og deretter å lese feilkodar. Symptoma etter utladingane var at SYNC 2 og
radioen (ACM) kom ut av synk, og at 12 V-uttaket kutta kvart 3.–4. sekund.
Mistanken er at batteriovervakinga (BMS) driv straumsparing (load shed).

Alt som **skriv** til bilen gjer eigaren i FORScan (under Wine). FORScan og fordiag
kan ikkje bruke kabelen samstundes.

## Absolutte tryggleiksreglar (må aldri brytast)

1. **Tillatslista i `src/command.rs` er handheva av typesystemet.** Alt som blir sendt,
   er ein `Command` (`AtCommand` eller `DiagRequest`). Legg aldri til ein variant for
   rå bytes eller rå tekst, eller ein `Transport`-metode som tek `&str`/`&[u8]`.
2. **Tillate:** AT til ELM327-brikka (sjå `AtCommand`), OBD `01`, `03`, `07`, `09`, `0A`,
   og UDS `19` (01/02/0A), `22` (éin DID) og `3E 00`.
3. **Forbode, uansett:** `04`, `08`, `10`, `11`, `14`, `27`, `28`, `2E`, `2F`, `31`,
   `34`–`37`, `3D` og `85`. Det same gjeld `ATCAF0`, andre `ATPB`-oppsett enn
   `81 04`, og `ATSH`/`ATCRA` utanfor 0x700–0x7FF.
4. Ber brukaren om noko som ikkje står på lista, sjølv i farten: **sei nei og vis til
   FORScan.** Ikkje utvid lista med skrivande eller sesjonsendrande tenester.
5. Ny variant på tillatslista krev testar i `command.rs`: tur-retur
   (`alle_kommandoar_overlever_tur_retur`) og avvisning av feil format.
6. Send aldri ei tom linje (CR). ELM327 gjentek då førre kommando.
7. **Ikkje gjett** Ford-spesifikke DID-ar eller moduladresser og presenter dei som
   fakta. Merk dei som **uverifiserte** til ein modul faktisk har svart.
8. Forklaringar av feilkodar skal merkast som **forslag som må sjekkast mot
   Ford-dokumentasjon**. Ikkje dikt opp tydingar av Ford-spesifikke B- og U-kodar.
9. Ikkje slett feilkodar, og ikkje forsøk nullstilling av BMS.

## Uverifiserte kandidatadresser (fase 3)

| Modul | Førespurnad | Svar (+8) | Buss | Status |
|---|---|---|---|---|
| PCM | 7E0 | 7E8 | HS | uverifisert |
| BCM | 726 | 72E | ? | uverifisert |
| ACM (radio) | 727 | 72F | MS? | uverifisert |
| APIM (SYNC) | 7D0 | 7D8 | MS? | uverifisert |
| IPC (instrument) | 720 | 728 | ? | uverifisert |

Oppdater tabellen når ein modul har svart på ekte bil, med dato og kva buss.

## Arbeidsform

- Små steg. Etter kvart steg: fortel eigaren kva hen skal gjere **fysisk** (tenning
  på/av, starte motor, slå om HS/MS-brytaren). Hen limer så inn resultatet.
- Alt skal kunne testast med `cargo test` utan bil (`MockTransport`).
- Kommunikasjon med eigaren skjer på **nynorsk**.

## Kodekonvensjonar

- **Kommentarar, CLI-hjelp og brukartekst på nynorsk.** Identifikatorar på engelsk.
- Testnamn er på nynorsk (t.d. `forbodne_tenester_blir_avviste`).
- Spenningar blir viste med desimalkomma via `battery::fmt_v` («12,46 V»).
- Synkron I/O, ingen async. Crates: `serialport` (utan standardfunksjonar, så vi
  ikkje treng libudev), `clap` derive, `anyhow`/`thiserror`, `chrono`,
  `serde`/`serde_json`.
- Hald koden enkel og godt kommentert. Eigaren skal kunne lese og forstå han.
- Analysefunksjonar skal vere reine funksjonar av data (t.d. `&[Sample]`), skilde frå
  I/O.

## Kommandoar

```sh
cargo build                 # bygg
cargo test                  # alle testar, utan bil
cargo fmt                   # formatering (rustfmt.toml: max_width = 120)
cargo clippy                # lint
cargo run -- --mock connect --probe
cargo run -- --mock --data-dir /tmp/fd battery --crank --seconds 8
cargo run -- check "14 FF FF FF"     # skal gje AVVIST
```

Serielaget kan testast mot `ELM327-emulator` (`pip install ELM327-emulator`,
`python3 -m elm -s car`, så `--port /dev/pts/N`).

## Arkitektur

| Fil | Innhald |
|---|---|
| `src/command.rs` | Tillatsliste: `AtCommand`, `DiagRequest`, `CanId`, `Command::parse`, `checked_wire` |
| `src/transport.rs` | `Transport`-trait, `SerialTransport`, `MockTransport`, `RawLog` (JSONL), `Logged` |
| `src/elm.rs` | `Elm`: init (HS/MS), `ATI`/`ATDP`/`ATRV`, svarreinsking, ELM-feil, baud-autodeteksjon |
| `src/battery.rs` | Sampling, analyse (rest/crank/charging), lagring, samla vurdering |
| `src/main.rs` | CLI (clap) og simulator-scenario for `--mock` |
| `docs/MANUAL.md` | Brukarmanual: hald han oppdatert når CLI-en endrar seg |

Dataflyt: `main` → `Elm` → `Logged` (rålogg) → `SerialTransport`/`MockTransport`.
Alle transportar kallar `checked_wire()` før dei skriv.

Data blir lagra i `fordiag-data/` (git-ignorert): `logs/raw-*.jsonl` og
`battery-<modus>-*.json`.

## Status og plan

- [x] Fase 1: `connect` (38400 → 115200, init, `ATI`/`ATDP`/`ATRV`, `--probe`, `--bus ms`)
- [x] Fase 2: `battery --rest/--crank/--charging/--summary`
- [ ] Fase 3: `dtc`. OBD `03`/`07`/`0A` mot PCM, UDS `19 02 FF` per modul med
  `ATSH`/`ATCRA` (svar = førespurnad + 8), `--bus ms`, dekoding av 3-byte DTC til
  P/C/B/U og statusbit i klartekst. Multi-frame-svar (ISO-TP) må handterast.
- [ ] Fase 4: `report`. Markdown med dato, batteritestar, feilkodar per modul og rålogg
  som vedlegg.
- [ ] Ikkje testa mot ekte bil enno. Fase 1 og 2 er testa med mock og emulator.

Når CLI-en endrar seg, oppdater både `README.md` og `docs/MANUAL.md`.
