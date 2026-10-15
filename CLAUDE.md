# CLAUDE.md

Guidance for Claude (and other contributors) working in this repository.

## The project

`fordiag` is a **read-only** diagnostic CLI in Rust for a Ford S-Max Mk2 (CJ),
registered 2016, 2.0 TDCi 150 hp, automatic, start/stop. It talks to the car through
an ELM327 USB cable (`/dev/ttyUSB0`, HS/MS switch).

The main goal is to find out whether the battery (EFB, 75 Ah, 730 A) is healthy after
two deep discharges, and then to read DTCs. The symptoms after the discharges were
that SYNC 2 and the radio (ACM) got out of sync, and that the 12 V outlet cut out
every 3–4 seconds. The suspicion is that the battery monitoring system (BMS) is
driving load shedding.

Known context from other owners (recall 18P11 for the battery monitoring sensor,
charger connection point vs. the BMS, BMS reset caveats, SYNC/ACM after flat
batteries) is collected in `docs/BACKGROUND.md`. It is community information, not
verified on this car.

Everything that **writes** to the car is done by the owner in FORScan (under Wine).
FORScan and fordiag cannot use the cable at the same time.

## Absolute safety rules (must never be broken)

1. **The allowlist in `src/command.rs` is enforced by the type system.** Everything
   sent is a `Command` (`AtCommand` or `DiagRequest`). Never add a variant for raw
   bytes or raw text, or a `Transport` method that takes `&str`/`&[u8]`.
2. **Allowed:** AT commands to the ELM327 chip (see `AtCommand`), OBD `01`, `03`,
   `07`, `09`, `0A`, and UDS `19` (01/02/0A), `22` (one DID) and `3E 00`.
3. **Forbidden, always:** `04`, `08`, `10`, `11`, `14`, `27`, `28`, `2E`, `2F`, `31`,
   `34`–`37`, `3D` and `85`. The same goes for `ATCAF0`, any `ATPB` configuration
   other than `81 04`, and `ATSH`/`ATCRA` outside 0x700–0x7FF.
4. If the user asks for something not on the list, even in passing: **say no and
   point to FORScan.** Never extend the list with writing or session-changing
   services.
5. A new allowlist variant requires tests in `command.rs`: a round trip
   (`all_commands_survive_round_trip`) and rejection of malformed input.
6. Never send an empty line (CR). The ELM327 would repeat the previous command.
7. **Do not guess** Ford-specific DIDs or module addresses and present them as facts.
   Mark them as **unverified** until a module has actually responded.
8. DTC explanations must be marked as **suggestions to be checked against Ford
   documentation**. Do not invent meanings for Ford-specific B and U codes.
9. Do not clear DTCs, and do not attempt a BMS reset.

## Unverified candidate addresses (phase 3)

| Module | Request | Response (+8) | Bus | Status |
|---|---|---|---|---|
| PCM | 7E0 | 7E8 | HS | unverified |
| BCM | 726 | 72E | ? | unverified |
| ACM (radio) | 727 | 72F | MS? | unverified |
| APIM (SYNC) | 7D0 | 7D8 | MS? | unverified |
| IPC (instrument cluster) | 720 | 728 | ? | unverified |

Update the table once a module has responded on the real car, with the date and the
bus.

## Way of working

- Small steps. After each step, tell the owner what to do **physically** (ignition
  on/off, start the engine, flip the HS/MS switch). They then paste the result.
- Everything must be testable with `cargo test` without a car (`MockTransport`).
- The owner usually writes in Norwegian (Nynorsk). Reply in their language. The
  code, CLI output and documentation are in English.

## Code conventions

- **Comments, CLI help, user-facing text and test names in English.**
- Voltages are formatted via `battery::fmt_v` ("12.46 V").
- Disclaimer: `DISCLAIMER` in `main.rs` is shown in `--help`. Keep it in sync with the
  disclaimer in `README.md` and `docs/MANUAL.md`.
- Synchronous I/O, no async. Crates: `serialport` (default features off, so no
  libudev is needed), `clap` derive, `anyhow`/`thiserror`, `chrono`,
  `serde`/`serde_json`.
- Keep the code simple and well commented. The owner wants to read and understand it.
- Analysis functions must be pure functions of data (e.g. `&[Sample]`), separate from
  I/O.
- License: MIT (`LICENSE`). New dependencies must have a compatible license.

## Commands

```sh
cargo build                 # build
cargo test                  # all tests, no car needed
cargo fmt                   # formatting (rustfmt.toml: max_width = 120)
cargo clippy                # lint
cargo run -- --mock connect --probe
cargo run -- --mock --data-dir /tmp/fd battery --crank --seconds 8
cargo run -- check "14 FF FF FF"     # must print REJECTED
```

The serial layer can be tested against `ELM327-emulator` (`pip install
ELM327-emulator`, `python3 -m elm -s car`, then `--port /dev/pts/N`).

## Architecture

| File | Contents |
|---|---|
| `src/command.rs` | Allowlist: `AtCommand`, `DiagRequest`, `CanId`, `Command::parse`, `checked_wire` |
| `src/transport.rs` | `Transport` trait, `SerialTransport`, `MockTransport`, `RawLog` (JSONL), `Logged` |
| `src/elm.rs` | `Elm`: init (HS/MS), `ATI`/`ATDP`/`ATRV`, response cleaning, ELM errors, baud auto-detection |
| `src/battery.rs` | Sampling, analysis (rest/crank/charging), persistence, overall assessment |
| `src/main.rs` | CLI (clap), disclaimer and the `--mock` simulator scenarios |
| `docs/MANUAL.md` | User manual: keep it up to date when the CLI changes |
| `docs/BACKGROUND.md` | Research notes and sources from other owners (unverified) |

Data flow: `main` → `Elm` → `Logged` (raw log) → `SerialTransport`/`MockTransport`.
Every transport calls `checked_wire()` before writing.

Data is stored in `fordiag-data/` (git-ignored): `logs/raw-*.jsonl` and
`battery-<mode>-*.json`.

## Status and roadmap

- [x] Phase 1: `connect` (38400 → 115200, init, `ATI`/`ATDP`/`ATRV`, `--probe`, `--bus ms`)
- [x] Phase 2: `battery --rest/--crank/--charging/--summary`
- [ ] Phase 3: `dtc`. OBD `03`/`07`/`0A` against the PCM, UDS `19 02 FF` per module
  with `ATSH`/`ATCRA` (response = request + 8), `--bus ms`, decoding of 3-byte DTCs
  to P/C/B/U and status bits in plain text. Multi-frame responses (ISO-TP) must be
  handled.
- [ ] Phase 4: `report`. Markdown with date, battery tests, DTCs per module and the
  raw log as an attachment.
- [ ] Not tested against the real car yet. Phases 1 and 2 are tested with the mock
  and the emulator.

When the CLI changes, update both `README.md` and `docs/MANUAL.md`.
