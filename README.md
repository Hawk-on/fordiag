# fordiag

[![CI](https://github.com/Hawk-on/fordiag/actions/workflows/ci.yml/badge.svg)](https://github.com/Hawk-on/fordiag/actions/workflows/ci.yml)

Read-only diagnostic CLI for the Ford S-Max Mk2 (2.0 TDCi) using an ELM327 USB cable.

**The tool can only read.** Everything sent to the cable must be a variant of the
allowlist in `src/command.rs`. Clearing DTCs, writing, routines, resets, session
changes, security access and similar operations cannot be expressed in the code.
Use FORScan for anything like that.

> [!WARNING]
> **Disclaimer: use at your own risk.** fordiag is provided "as is", without
> warranty of any kind (see [LICENSE](LICENSE)). Connecting any device to a vehicle's
> diagnostic port carries some risk. You alone are responsible for how you use
> this software and for any consequences, including damage to the vehicle, its
> battery or electronics, data loss, or personal injury.
>
> - Verify results with proper tools (multimeter, battery load tester, official
>   Ford documentation) before acting on them.
> - DTC explanations and module addresses are suggestions and are marked
>   *unverified* until confirmed.
> - Never operate the tool while driving. Run tests with the vehicle parked
>   safely.
> - fordiag is not affiliated with, endorsed by or supported by Ford Motor Company,
>   FORScan or ELM Electronics. All trademarks belong to their respective owners.

**Documentation:**
* [`docs/MANUAL.md`](docs/MANUAL.md): the full user manual (every command, interpretation, troubleshooting)
* [`docs/BACKGROUND.md`](docs/BACKGROUND.md): what other S-Max Mk2 / Galaxy / Mondeo owners have found (recall 18P11, BMS and charging), with sources
* [`CLAUDE.md`](CLAUDE.md): development guide (safety rules, conventions, architecture, roadmap)

## Install

**Kubuntu / Ubuntu 26.04 (amd64):** download `fordiag_<version>-1_amd64.deb` from
[Releases](https://github.com/Hawk-on/fordiag/releases) and install it:

```sh
sudo apt install ./fordiag_0.1.0-1_amd64.deb
sudo usermod -aG dialout $USER     # access to /dev/ttyUSB0; log out and in again
```

The package also works on Ubuntu 22.04 and later and on Debian 12 and later (it only
needs glibc 2.34). The manual is installed in `/usr/share/doc/fordiag/`.

**Windows 10/11 (x64, optional):** a `fordiag-<version>-x86_64-windows.zip` may also be
attached to the release. Use `--port COM3` (check Device Manager for the number).
Windows builds are tested in CI but not on a real car.

## Build and test

```sh
cargo build --release
cargo test            # unit, replay and safety tests; no car needed (~15 s)
```

The binary ends up in `target/release/fordiag`.

## Commands

| Command | What it does |
|---|---|
| `fordiag connect [--bus hs\|ms] [--probe]` | Init (`ATZ ATE0 ATL0 ATS1 ATH1 ATSP6`), shows `ATI`, `ATDP` and `ATRV`. `--probe` also sends OBD `01 00`. |
| `fordiag battery --rest [--seconds 10]` | Resting voltage (median of readings every 0.5 s) |
| `fordiag battery --crank [--seconds 15]` | Lowest voltage while you crank the engine |
| `fordiag battery --charging [--minutes 3 \| --seconds N]` | Charging voltage: min/max/mean and trend |
| `fordiag battery --summary` | Overall assessment of the latest saved tests |
| `fordiag dtc [--bus hs\|ms] [--module NAME]` | Reads DTCs: OBD `03`/`07`/`0A` from the PCM and UDS `19 02 FF` from each candidate module. Never clears anything. |
| `fordiag dtc --list` | Lists the candidate modules (addresses unverified) |
| `fordiag report [--vehicle "TEXT"] [--no-raw]` | Markdown report from the latest saved results, with raw logs as an appendix |
| `fordiag check "<command>"` | Checks a command against the allowlist **without** sending it |

Global flags: `--port /dev/ttyUSB0`, `--baud 38400`, `--mock` (built-in simulator), `--replay LOG` (replay a recorded raw log),
`--data-dir fordiag-data`, `-y/--yes` (do not wait for Enter).

The battery tests only send `ATZ`, `ATE0`, `ATL0` and `ATRV`. None of these go out on
the CAN bus, so measuring does not wake the car.

## Quick start

```sh
fordiag --mock connect --probe          # try it without a car
fordiag connect --probe                 # cable in, switch on HS, ignition on
fordiag battery --rest                  # car has been parked for several hours
fordiag battery --crank                 # start the engine when "START NOW" appears
fordiag battery --charging              # engine idling for 3 minutes
fordiag battery --summary
fordiag dtc --bus hs                    # switch on HS, ignition on, engine off
fordiag dtc --bus ms                    # switch on MS
fordiag report --vehicle "Ford S-Max Mk2 2.0 TDCi"   # at home, no cable needed
```

See the [manual](docs/MANUAL.md) for what to do physically before each test and how
to interpret the results.

## Files

* `fordiag-data/logs/raw-*.jsonl`: raw log of all communication (sent and
  received) with timestamps
* `fordiag-data/battery-*.json`: results and every measurement from the battery tests
* `fordiag-data/dtc-*.json`: DTC scan results per module, with raw responses
* `fordiag-data/report-*.md`: reports from `fordiag report`

## Testing against an ELM327 emulator (optional)

```sh
pip install ELM327-emulator
python3 -m elm -s car        # prints e.g. /dev/pts/3
fordiag --port /dev/pts/3 connect --probe
```

For a Ford-like scenario (BCM, ACM, APIM, IPC with made-up DTCs, and MS-CAN setup):
`python3 tools/ford_emulator.py`. See the manual, chapter 12.

## Using it with Claude Code

`.claude/skills/fordiag/` is a Claude Code skill for running fordiag on your own PC:
Claude tells you what to do physically, waits for your confirmation, runs the
read-only commands and reads the results. See the manual, chapter 13.

## Code overview

* `command.rs`: allowlist (types for AT commands and diagnostic requests) and tests
* `transport.rs`: `Transport` trait, `SerialTransport`, `MockTransport` and raw log
* `elm.rs`: init, response parsing and baud auto-detection
* `battery.rs`: voltage sampling and analysis
* `isotp.rs`: reassembly of multi-frame (ISO-TP) responses
* `dtc.rs`: candidate modules, DTC decoding and module scans
* `report.rs`: the Markdown report
* `safety_tests.rs`, `tests/cli_safety.rs`: exhaustive and end-to-end safety tests
* `replay.rs`: replay of recorded raw logs (`--replay`, regression tests in `fixtures/replay/`)
* `main.rs`: CLI and the `--mock` simulator

## Status

- [x] Phase 1: connection
- [x] Phase 2: battery status
- [x] Phase 3: DTCs (`fordiag dtc`), not yet tested on the car
- [x] Phase 4: report (`fordiag report`)

## CI/CD

- **CI** (`.github/workflows/ci.yml`, on every push and pull request, Ubuntu 26.04):
  `cargo fmt`, `clippy -D warnings`, all tests including the safety tests, the
  end-to-end test against ELM327-emulator (`tools/ci_emulator.sh`), and a `.deb`
  that is installed and smoke-tested. The `.deb` is kept as a build artifact.
- **Windows (optional)** (`.github/workflows/windows.yml`): started by hand under
  Actions; tests and builds a Windows x64 zip.
- **Release** (`.github/workflows/release.yml`): pushing a tag `vX.Y.Z` (matching
  `Cargo.toml`) builds and publishes a GitHub Release with the `.deb`, a Linux
  tarball, an optional Windows zip and SHA-256 checksums:

  ```sh
  git tag v0.1.0 && git push origin v0.1.0
  ```

## License

[MIT](LICENSE). Provided without warranty; use at your own risk.
