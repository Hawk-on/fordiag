# fordiag – user manual

`fordiag` is a command-line tool that reads diagnostic data from a Ford S-Max Mk2
(2016, 2.0 TDCi) through an ELM327 USB cable. The tool **can only read**. Anything
that writes to the car, such as clearing DTCs or resetting the battery monitoring
system (BMS), is done in FORScan.

This manual covers version 0.1 (phases 1–4: connection, battery, DTCs and report).

> [!WARNING]
> **Disclaimer: use at your own risk.** fordiag is provided "as is", without
> warranty of any kind (see [LICENSE](../LICENSE)). You alone are responsible for how
> you use it and for any consequences, including damage to the vehicle, its battery
> or electronics, or personal injury.
>
> - Never operate the tool while driving.
> - Verify results with proper tools and official Ford documentation before acting
>   on them.
> - fordiag is not affiliated with or endorsed by Ford, FORScan or ELM Electronics.

---

## Contents

1. [Before you start](#1-before-you-start)
2. [Command overview](#2-command-overview)
3. [Global flags](#3-global-flags)
4. [`fordiag connect`](#4-fordiag-connect)
5. [`fordiag battery`](#5-fordiag-battery)
6. [`fordiag dtc`](#6-fordiag-dtc)
7. [`fordiag report`](#7-fordiag-report)
8. [`fordiag check`](#8-fordiag-check)
9. [Files created by the tool](#9-files-created-by-the-tool)
10. [Safety and the allowlist](#10-safety-and-the-allowlist)
11. [Troubleshooting](#11-troubleshooting)
12. [Testing without a car](#12-testing-without-a-car)
13. [Using fordiag with Claude Code](#13-using-fordiag-with-claude-code)

---

## 1. Before you start

### Requirements

- Linux (tested on Kubuntu) with stable Rust (`rustup`/`cargo`)
- An ELM327 USB cable, typically labelled "Ford OBD2 to USB", preferably with an
  HS/MS switch
- Your user in the `dialout` group, so you can open `/dev/ttyUSB0`:

  ```sh
  groups               # should include "dialout"
  sudo usermod -aG dialout $USER   # if not; then log out and back in
  ```

### Installing

**From a release (recommended on Kubuntu 26.04):** download the `.deb` from the
project's GitHub Releases page and install it with
`sudo apt install ./fordiag_<version>-1_amd64.deb`. The binary goes to
`/usr/bin/fordiag`, and this manual to `/usr/share/doc/fordiag/`. Remove it with
`sudo apt remove fordiag`. Check the download against `SHA256SUMS-linux.txt` with
`sha256sum -c SHA256SUMS-linux.txt --ignore-missing`.

**Windows 10/11 (x64, optional):** unzip `fordiag-<version>-x86_64-windows.zip` and run
`fordiag.exe` from a terminal. The default port on Windows is `COM3`; find the right
one in Device Manager under "Ports (COM & LPT)" and pass it with `--port COM5` or
similar. The `dialout` group does not apply. The emulator test and the Claude Code
skill assume Linux.

### Building from source

```sh
cargo build --release
```

The binary ends up in `target/release/fordiag`. The examples below just write
`fordiag`. Either use the full path, or copy the binary somewhere in your `PATH`,
e.g. `cp target/release/fordiag ~/.local/bin/`.

### About the cable

- **FORScan and fordiag cannot use the cable at the same time.** Close FORScan (and
  Wine) before running `fordiag`.
- The switch on the cable decides which bus you are connected to:
  - **HS** (500 kbit/s): engine control (PCM) and standard OBD-II
  - **MS** (125 kbit/s): comfort modules. Many of them are probably on this bus,
    but that is not verified for this car.
- The battery tests do not care about the switch, because they only measure
  voltage.

---

## 2. Command overview

| Command | Purpose | Needs a car? |
|---|---|---|
| `fordiag connect` | Test the connection: version, protocol and voltage | Yes (or `--mock`) |
| `fordiag battery --rest` | Resting voltage | Yes |
| `fordiag battery --crank` | Lowest voltage while cranking | Yes |
| `fordiag battery --charging` | Charging voltage with the engine running | Yes |
| `fordiag battery --summary` | Overall assessment of saved tests | No |
| `fordiag dtc` | Read DTCs from the PCM and other modules | Yes (or `--mock`) |
| `fordiag dtc --list` | List the candidate modules and addresses | No |
| `fordiag report` | Markdown report from the latest saved results | No |
| `fordiag check "<command>"` | Check a command against the allowlist | No |
| `fordiag help [command]` | Built-in help | No |


---

## 3. Global flags

These flags can go before or after the command name.

| Flag | Default | Meaning |
|---|---|---|
| `--port <PATH>` | `/dev/ttyUSB0` (Windows: `COM3`) | Serial port, or a pseudo-terminal from an emulator (e.g. `/dev/pts/3`) |
| `--baud <N>` | (auto) | Force a baud rate. Without it, fordiag tries 38400 and then 115200. |
| `--mock` | off | Use the built-in simulator instead of cable and car |
| `--replay <LOG>` | off | Replay a recorded raw log instead of using the cable (see [chapter 12](#12-testing-without-a-car)) |
| `--data-dir <DIR>` | `fordiag-data` | Where logs and results are stored (relative to the current directory) |
| `-y`, `--yes` | off | Do not wait for Enter before measurements. Without it, fordiag refuses to start a measurement when there is no keyboard input (e.g. when run by a script or an AI agent), so that `-y` is always a deliberate choice. |
| `-h`, `--help` | | Show help (includes the disclaimer) |
| `-V`, `--version` | | Show version |

**Exit code:** `0` when everything went well, `1` on error. The error message is
then written to stderr, prefixed with `Error:`.

---

## 4. `fordiag connect`

Opens the cable, runs the init sequence and shows what the ELM327 chip reports.

```
fordiag connect [--bus hs|ms] [--probe]
```

| Flag | Default | Meaning |
|---|---|---|
| `--bus hs` | ✓ | HS-CAN 500 kbit/s: `ATSP6` |
| `--bus ms` | | MS-CAN 125 kbit/s: `ATPB 81 04` + `ATSPB`. Asks you to flip the switch first. |
| `--probe` | off | Also sends OBD `01 00` (supported PIDs) to see if the engine control module responds |

### What happens

1. Opens the port at 38400 baud and asks `ATI`. If there is no reply, 115200 is
   tried.
2. Init sequence: `ATZ`, `ATE0`, `ATL0`, `ATS1`, `ATH1`, then the protocol (`ATSP6`,
   or `ATPB8104` + `ATSPB`).
3. Prints `ATI` (version), `ATDP` (protocol) and `ATRV` (voltage).
4. With `--probe`: sends `01 00` and shows the response.

Only step 4 sends anything out on the CAN bus. The other steps only talk to the
ELM327 chip.

### Physical setup

- Without `--probe`: cable plugged in, ignition may be off.
- With `--probe`: **ignition on** (the engine does not need to run), and the switch
  on **HS**.

### Example

```
$ fordiag connect --probe
Connected: /dev/ttyUSB0 @ 38400 baud
ELM version: ELM327 v1.5
Protocol:    ISO 15765-4 (CAN 11/500)
Voltage:     12.60 V

Sending OBD "01 00" (supported PIDs) …
Response:
  7E8 06 41 00 BE 3F A8 13

Raw log: fordiag-data/logs/raw-20261014-164038.jsonl
```

The response line starts with the CAN ID of the module that responded (`7E8` is the
reply from the engine control module to a request sent to `7E0`). If you get
`NO DATA`, no module responded.

---

## 5. `fordiag battery`

Battery tests based on `ATRV`, which is the voltage the ELM327 measures on pin 16 of
the OBD connector (permanent 12 V). You must choose **exactly one** mode.

```
fordiag battery --rest     [--seconds N]
fordiag battery --crank    [--seconds N]
fordiag battery --charging [--minutes N | --seconds N]
fordiag battery --summary
```

| Flag | Default | Meaning |
|---|---|---|
| `--seconds N` | rest: 10, crank: 15 | Duration in seconds. For `--charging` it overrides `--minutes`. |
| `--minutes N` | 3 | Duration for `--charging` |

The battery tests only send `ATZ`, `ATE0`, `ATL0` and `ATRV`. **Nothing goes out on
the CAN bus**, so the measurement itself does not wake the car.

Each test is saved as JSON in `fordiag-data/` (see
[chapter 9](#9-files-created-by-the-tool)).

### 5.1 `--rest`: resting voltage

**Physical setup:** The car should have been parked for several hours without a
charger and unused, with the ignition off. Opening the door to plug in the cable
wakes the modules, and they draw current for a while. Close the door and wait 10–30
minutes before measuring. If you have just charged with a smart charger, let the
battery rest for 12–24 hours first, or you will measure surface charge.

fordiag measures every 0.5 seconds and uses the **median**.

| Median | Verdict |
|---|---|
| > 13.0 V | Invalid: the engine is running or a charger is connected |
| ≥ 12.6 V | Fully charged and healthy |
| 12.4–12.5 V | OK, but not fully charged |
| < 12.4 V | Discharged or weakened battery |

You get a warning if the voltage varies by more than 0.3 V during the measurement,
because that means something in the car is still drawing current.

### 5.2 `--crank`: crank test

**Physical setup:** Sit in the car and switch the ignition on. Run the command, and
start the engine 2–3 seconds after `>>> START NOW <<<` appears. The measurement lasts
15 seconds.

fordiag samples `ATRV` as fast as the cable allows, typically every 20–60 ms.

| Lowest voltage | Verdict |
|---|---|
| ≥ 10.0 V | Healthy |
| 9.6–10.0 V | Borderline |
| < 9.6 V | Weak battery |
| No drop of at least 1 V | No cranking found. Was the engine started in time? |

The output shows the voltage before cranking (median of the first 1.5 s), the lowest
reading, and the voltage after cranking (median of the last 2 s). You get a warning
if:

- the mean interval between readings exceeds 100 ms, because the minimum may then be
  lower than measured
- there is a gap of more than 500 ms in the readings, because the ELM chip may have
  lost power while cranking
- the voltage before cranking is above 13 V
- the voltage after cranking is below 13 V

### 5.3 `--charging`: charging test

**Physical setup:** Start the engine and let it idle for the whole measurement, which
lasts 3 minutes. A reading is printed every 10 seconds while the test runs.

fordiag measures once per second and reports:

- minimum, mean and maximum
- the **trend** in V/min (linear regression): rising (> +0.05), falling (< −0.05) or
  stable
- the share of the time the voltage was ≥ 13.2 V

| Pattern | Verdict |
|---|---|
| Max > 15.0 V | Unusually high, should be checked |
| Max < 13.0 V | Low: the alternator is not charging, or smart charging is holding back |
| Mean 13.2–14.8 V and min ≥ 12.8 V | Normal |
| Anything else | Variable, which is common with Ford smart charging |

Ford smart charging deliberately keeps the voltage low when the BMS thinks the battery
is full. If you get "Low", repeat the test with large loads on (lights, blower, rear
window heater).

### 5.4 `--summary`: overall assessment

Reads the latest saved test of each type in `--data-dir` and prints an overall
assessment. This command needs neither cable nor car.

```
$ fordiag battery --summary
Latest saved battery tests in fordiag-data:
  Rest:     12.50 V – OK, but not fully charged (12.4–12.5 V)  (2026-10-14 16:40 [/dev/ttyUSB0 @ 38400 baud])
  Crank:    lowest 10.20 V – Healthy: …
  Charging: mean 14.48 V – Within the normal charging range  (…)

Overall assessment:
  • Resting voltage and crank test look normal. …
```

Results from `--mock` are labelled `[mock (no car)]`. Use a separate `--data-dir`
when testing, so they do not mix with real measurements.

### Limitations

- The ELM327 measures with 0.1 V resolution, and clones can be off by 0.1–0.2 V.
  Compare once with a multimeter on the battery terminals.
- The voltage at the OBD connector is slightly lower than at the terminals,
  especially while cranking.
- Voltage cannot measure **capacity** (Ah). A reliable battery test requires a load
  or conductance tester.
- The state of charge (SOC) is not read from the BCM, because we have no verified DID
  for this car.

---

## 6. `fordiag dtc`

Reads diagnostic trouble codes. **It only reads; nothing is cleared.**

```
fordiag dtc [--bus hs|ms] [--module NAME]... [--list]
```

| Flag | Default | Meaning |
|---|---|---|
| `--bus hs` | ✓ | HS-CAN 500 kbit/s. Set the cable switch to **HS**. |
| `--bus ms` | | MS-CAN 125 kbit/s. Set the cable switch to **MS**. |
| `--module NAME` | all | Only these modules; repeat the flag for several (e.g. `--module BCM --module ACM`). |
| `--list` | | Show the candidate modules and addresses, then exit. |

### Physical setup

- **Ignition on, engine off.** The scan takes about 1 second per module.
- Run it twice, once with `--bus hs` (switch on HS) and once with `--bus ms`
  (switch on MS). Which modules live on which bus is not verified for this car, so
  every candidate is tried on both.

### What happens

1. Init as in `connect`, plus `ATCAF1` (automatic ISO-TP formatting) and `ATST64`
   (400 ms response timeout).
2. For each module: `ATSH <request ID>` and `ATCRA <request ID + 8>`, so only that
   module's answers are received.
3. For the **PCM** only: OBD-II modes `03` (stored), `07` (pending) and `0A`
   (permanent).
4. For every module: UDS `19 02 FF` (all DTCs with their status byte).

Multi-frame answers (ISO-TP) are reassembled. A "response pending" (`7F 19 78`)
followed by the real answer is handled.

### Candidate modules

**All addresses are unverified for this car.** They are common Ford addresses from
other models. A module only counts as verified once it has answered. Run
`fordiag dtc --list` for the current list:

| Module | Request → response | Guessed bus | Description |
|---|---|---|---|
| PCM | 7E0 → 7E8 | HS | Powertrain control module (engine) |
| TCM | 7E1 → 7E9 | HS | Transmission control module (PowerShift) |
| ABS | 760 → 768 | HS | Anti-lock brakes / stability control |
| RCM | 737 → 73F | HS | Restraints control module (airbags) |
| PSCM | 730 → 738 | HS | Power steering control module |
| IPC | 720 → 728 | ? | Instrument panel cluster |
| BCM | 726 → 72E | ? | Body control module (incl. battery monitoring) |
| ACM | 727 → 72F | MS | Audio control module (radio) |
| APIM | 7D0 → 7D8 | MS | SYNC |
| HVAC | 733 → 73B | ? | Climate control |
| GWM | 716 → 71E | ? | Gateway module (may not exist) |
| DDM | 740 → 748 | MS | Driver door module |
| PDM | 741 → 749 | MS | Passenger door module |
| PAM | 736 → 73E | ? | Parking aid module |

### Reading the output

```
== BCM (726 → 72E) – Body control module (incl. battery monitoring) [responded] ==
  UDS 19 02 FF         3 DTC(s):
    U3003:16   SAE-defined, type: circuit voltage below threshold (suggestion), status 0x08 [confirmed]
```

(Example from `--mock`; the codes are made up.)

- **Code:** `U3003:16` is the DTC (`U3003`) plus the failure type byte (`16`).
  Standard OBD-II codes (modes 03/07/0A) have no failure type byte.
- **Category:** `SAE-defined` or `manufacturer-specific`, from the first two
  characters. This is a rough guide only.
- **Type:** a generic SAE J2012 description of the failure type byte. It is a
  **suggestion** and must be checked against Ford documentation.
- **Status:** the UDS status bits (ISO 14229). `confirmed` means the fault has been
  confirmed. `test failed now` means it is present right now. `failed since clear`
  means it has failed at least once since the codes were last cleared.
- **What the code means is not included.** fordiag does not guess the meaning of
  Ford-specific codes. Look them up in FORScan or Ford documentation.

Per module, the result is one of:

| Result | Meaning |
|---|---|
| `no DTCs` | The module answered and has no stored codes |
| `N DTC(s)` | The module answered with codes |
| `negative response 0x.. (…)` | The module answered but refused the request; the address is still confirmed |
| `no response` | Nothing answered on this address on this bus |
| `error: …` | ELM error or malformed answer; see the raw log |

The summary at the end lists which modules responded on the bus that was scanned.
**Please share it**, so the address table can be marked as verified.

After deep discharges, expect some `U` codes (lost communication) and voltage-related
failure types. These may be after-effects of the low voltage rather than faults in the
modules themselves.

The result is saved as `fordiag-data/dtc-<bus>-YYYYMMDD-HHMMSS.json`.

## 7. `fordiag report`

Writes a Markdown report from the **latest saved results** in `--data-dir`. It needs
neither cable nor car, so you can run it at home afterwards.

```
fordiag report [--vehicle "TEXT"] [-o FILE] [--no-raw]
```

| Flag | Default | Meaning |
|---|---|---|
| `--vehicle "TEXT"` | none | Vehicle description at the top, e.g. `"Ford S-Max Mk2 2.0 TDCi 150, PowerShift"` |
| `-o`, `--output FILE` | `fordiag-data/report-YYYYMMDD-HHMMSS.md` | Where to write the report |
| `--no-raw` | off | Leave out the raw logs appendix (gives a much shorter report) |

### What it contains

1. **Header:** date, fordiag version, the vehicle text, and a short disclaimer. If any
   result came from `--mock`, a clear warning that the report contains simulated data.
2. **Summary:** the overall battery assessment (as in `battery --summary`) and, per
   bus, the number of DTCs and which modules responded.
3. **Battery tests:** the latest resting, crank and charging test with values,
   verdict and warnings. Missing tests are listed with the command to run.
4. **DTCs:** for the latest HS and MS scan, a table of every module (address, whether
   it responded, result per read), and then a table of codes for each module that
   has any: code, category, failure type (suggestion) and status bits.
5. **Appendix: raw logs:** the complete raw logs of those runs, one line per event
   (`timestamp dir data`, with `\r` made visible).

Only the latest test of each kind and the latest scan per bus are used. Older results
stay in `fordiag-data/` but are not included.

### Privacy

The report contains no VIN or registration number, unless you put them in
`--vehicle` yourself. The raw logs only contain what fordiag sent and received.
fordiag never asks for the VIN (OBD `09 02`). Read the report before sharing it
anyway.

### Sharing

The report is plain Markdown. GitHub, GitLab and most editors render it nicely, and it
can be converted to PDF or HTML with e.g. `pandoc report.md -o report.pdf`. Use
`--no-raw` for a version to read, and the full version for someone who wants to
check the details.

## 8. `fordiag check`

Checks whether a command is on the allowlist, **without sending anything**. This is
useful for seeing what the tool accepts.

```
fordiag check "<command>"
```

Text starting with `AT` is parsed as an AT command. Anything else is parsed as hex
(whitespace and letter case do not matter).

```
$ fordiag check "19 02 ff"
ALLOWED: Diag(Uds(ReadDtcInformation(ByStatusMask(255)))) → sent as "1902FF"

$ fordiag check "14 FF FF FF"
REJECTED: service 0x14 is FORBIDDEN (UDS: ClearDiagnosticInformation) – use FORScan if you really need this

$ fordiag check "ATSH 123"
REJECTED: CAN ID 0x123 is outside the diagnostic range 0x700–0x7FF
```

---

## 9. Files created by the tool

Everything goes under `--data-dir`, which is `fordiag-data/` unless you choose
something else. The directory is listed in `.gitignore`.

### `fordiag-data/logs/raw-YYYYMMDD-HHMMSS.jsonl`

Raw log of all communication, one JSON line per event. Each line is written to disk
immediately, so the log survives if the program crashes.

```json
{"ts":"2026-10-14T16:40:38.918+02:00","dir":"info","data":"transport: /dev/ttyUSB0 @ 38400 baud"}
{"ts":"2026-10-14T16:40:38.918+02:00","dir":"tx","data":"ATZ"}
{"ts":"2026-10-14T16:40:38.943+02:00","dir":"rx","data":"\r\rELM327 v1.5\r\r"}
```

| `dir` | Meaning |
|---|---|
| `tx` | Sent to the cable (exact text, without CR) |
| `rx` | Received from the cable (raw, without the `>` prompt) |
| `err` | Error, e.g. a timeout or a rejected command |
| `info` | Events such as opening the port and baud search |

For a quick overview with `jq` (if installed):

```sh
jq -r '"\(.ts) \(.dir) \(.data)"' fordiag-data/logs/raw-*.jsonl | less
```

### `fordiag-data/dtc-<bus>-YYYYMMDD-HHMMSS.json`

The result of one DTC scan: start time, source, bus, and for each module every
request, its outcome (DTCs, negative response, no response or error) and the raw
response lines.

Both result files also record the file name of their raw log (`raw_log`), so the
report can include the right logs.

### `fordiag-data/report-YYYYMMDD-HHMMSS.md`

Reports written by `fordiag report` (unless `-o` is given).

### `fordiag-data/battery-<mode>-YYYYMMDD-HHMMSS.json`

The result of one battery test: start time, source, analysis and **every
measurement** (`t_ms`, `volts`). These are the files `--summary` reads, and later
`report`.

---

## 10. Safety and the allowlist

The allowlist is enforced in the code (`src/command.rs`), not only in the
documentation. There is no way to send arbitrary bytes, and fordiag has no command
for sending raw hex.

### Allowed

| Type | Commands |
|---|---|
| AT to the ELM327 | `ATZ`, `ATE0`, `ATL0`, `ATS0/1`, `ATH0/1`, `ATSP0/6/B`, `ATPB 81 04`, `ATSH 7xx`, `ATCRA 7xx`, `ATCRA`, `ATCAF1`, `ATST xx`, `ATRV`, `ATI`, `ATDP` |
| OBD-II | `01 pp`, `03`, `07`, `09 pp`, `0A` |
| UDS | `19 01 mm`, `19 02 mm`, `19 0A`, `22 hhll` (one DID), `3E 00` |

### Rejected, among others

| What | Why |
|---|---|
| `04` | Clears OBD DTCs |
| `08` | OBD: controls on-board systems |
| `10` | Changes the diagnostic session |
| `11` | Resets a module (ECU reset) |
| `14` | Clears UDS DTCs |
| `27` | Security access |
| `28` | Communication control |
| `2E`, `3D` | Writes data or memory |
| `2F` | Controls outputs |
| `31` | Runs routines |
| `34`–`37` | Download/upload |
| `85` | Turns DTC recording on/off |
| `3E 80` | Only `3E 00` is allowed |
| `ATCAF0` | Would allow raw CAN frames to be sent |
| `ATMA`, `ATBRD`, `ATCV`, `ATPP…`, `ATWM…` | Not needed, and can change the chip |
| `ATPB` with anything other than `81 04` | A wrong bit rate can disturb the bus |
| `ATSH`/`ATCRA` outside 0x700–0x7FF | Restricted to diagnostic IDs |

Other safety measures:

- Right before anything is written, the text is parsed again. It must produce exactly
  the same command, or it is not sent.
- Text commands (`fordiag check`) accept only spaces and tabs as separators. Line
  breaks and other control characters are rejected.
- fordiag never sends an empty line. A bare CR makes the ELM327 repeat the *previous*
  command, which could have been something FORScan sent.

If you need something that is not on the list, do it in **FORScan**.

---

## 11. Troubleshooting

| Symptom | Possible causes and fixes |
|---|---|
| `could not open /dev/ttyUSB0` | The cable is not plugged in, FORScan/Wine is holding the port, or you are not in `dialout`. Check `ls -l /dev/ttyUSB*` and `groups`. |
| `Permission denied` | Add yourself to `dialout`, then log out and back in. |
| `no ELM327 found on …` | The cable is not connected to the car (the ELM is powered by the OBD connector), the port is wrong, or the baud rate is unusual. Try `--baud 9600` or `--baud 115200`. |
| `NO DATA` with `--probe` | The ignition is off, the switch is on MS, or the module is asleep. |
| `dtc`: every module says `no response` | Ignition off, or the switch does not match `--bus`. |
| `dtc`: `BUFFER FULL` | A long answer overflowed the ELM's buffer. Try `--baud 115200` if the cable supports it. |
| `CAN ERROR` / `BUS ERROR` | Probably the wrong bus for the switch position, e.g. `--bus ms` with the switch on HS. |
| `ELM327 replied "?"` | The chip does not know the command. Basic clones often lack `ATPB` (needed for `--bus ms`). |
| `timeout` | Loose cable, or the ELM lost power. Look at the `err` lines in the raw log. |
| Rest test says "Invalid" | The engine is running, a charger is connected, or you measured too soon after driving or charging. |
| Crank test finds no cranking | The engine was started too late. Try `--seconds 25`. |
| Voltage differs from the multimeter | Clones can be off by 0.1–0.2 V. Keep the difference in mind when reading the numbers. |

Attach the latest `raw-*.jsonl` when asking for help.

---

## 12. Testing without a car

### Built-in simulator (`--mock`)

All commands work with `--mock`. The simulator also skips "Press Enter".

```sh
D=$(mktemp -d)
fordiag --mock --data-dir $D connect --probe
fordiag --mock --data-dir $D battery --rest --seconds 3
fordiag --mock --data-dir $D battery --crank --seconds 8
fordiag --mock --data-dir $D battery --charging --seconds 20
fordiag --mock --data-dir $D battery --summary
fordiag --mock --data-dir $D dtc --bus ms
fordiag --data-dir $D report --vehicle "Test"
```

### Unit tests

```sh
cargo test
```

The tests cover the allowlist (all 256 service bytes), response parsing, init
sequences against `MockTransport`, ISO-TP reassembly, DTC decoding, module scans
with recorded answers, and the analyses with their thresholds.

### Safety tests

`cargo test` also runs the safety tests (`src/safety_tests.rs` and
`tests/cli_safety.rs`). They take about 15 seconds:

- **Exhaustive allowlist:** every byte string of 0–3 bytes (16.8 million) is accepted
  exactly when it is one of the allowed read forms, and is sent byte for byte as
  given. Longer strings are checked with 500 000 random samples. Every forbidden
  service is rejected with any arguments.
- **AT commands:** every AT command of up to four characters, and every `ATSH`/`ATCRA`
  with a 3-digit ID, is accepted exactly when it is on the list. Dangerous commands
  (`ATCAF0`, `ATMA`, `ATPP…`, `ATCV…`, other protocols, …) are rejected.
- **Wire format:** all 66 000+ commands that can be constructed give a non-empty
  string of only letters and digits, which parses back to the same command.
- **Text tricks:** line breaks, control characters, separators, prefixes such as
  `0x`, and look-alike characters (full-width or Arabic digits, Cyrillic letters) are
  rejected, so a second command can never ride along.
- **Robustness:** random and broken responses never crash the parsers or a module
  scan.
- **Tripwires in the source code:**
  - Only `SerialTransport` opens and writes to the serial port.
  - Every transport takes a `&Command` and calls `checked_wire`.
  - The `Transport` trait has no other methods.
- **End to end with the binary:**
  - A full `--mock` session (all commands) only sends commands that `fordiag check`
    accepts.
  - Without `--yes` and without a keyboard, nothing is sent.
  - A tampered log cannot smuggle a command in through `--replay`.

The tests have also been checked against deliberate mistakes in the code (a widened
CAN ID range, line breaks accepted as separators, an extra write to the port). Each
mistake made at least one test fail.

### Replaying a recording (`--replay`)

Every run writes a raw log (`fordiag-data/logs/raw-*.jsonl`). `--replay` plays such a
log back instead of talking to the cable: each command fordiag sends must match the
next recorded command exactly, and the recorded answer is returned.

```sh
fordiag --replay fordiag-data/logs/raw-20261018-101500.jsonl dtc --bus ms
```

- Run the **same command with the same options** as when it was recorded. If fordiag
  would now send something different, the replay stops with a message saying where
  it diverged.
- Useful for `connect` and `dtc`, for example to check that an improved parser
  interprets an old answer correctly. Battery tests are time-based and cannot be
  replayed meaningfully, but their JSON files already contain every sample.
- Results from a replay are labelled `replay of raw-….jsonl`, and the report warns
  that they are not from a live connection.

### Recordings as regression tests (`fixtures/replay/`)

`cargo test` replays every `fixtures/replay/dtc-*.jsonl`. The bus and the modules
are taken from the recording itself. If there is a matching `.expected.json` (a
saved `dtc-*.json` result), the module results must match it exactly.

To add a recording from the car:

```sh
cp fordiag-data/logs/raw-20261018-101500.jsonl fixtures/replay/dtc-ms-smax-2026-10-18.jsonl
cp fordiag-data/dtc-ms-20261018-101500.json   fixtures/replay/dtc-ms-smax-2026-10-18.expected.json
cargo test
```

The raw log and the result file from the same run belong together (same timestamp).
Recordings contain no VIN, but check them before committing them to a public
repository.

The repository currently has three fixtures: an HS scan against ELM327-emulator's
"car" scenario, an HS scan against the Ford scenario below, and an MS scan against
`--mock`.

### ELM327 emulator (tests the serial layer)

```sh
pip install ELM327-emulator
python3 -m elm -s car          # prints e.g. /dev/pts/3
fordiag --port /dev/pts/3 connect --probe
```

The emulator reports about 13 V, so `--rest` will say "Invalid". That is expected.

### Ford-like emulator scenario (`tools/ford_emulator.py`)

The built-in "car" scenario only has an engine control module. `tools/ford_emulator.py`
starts the same emulator with a scenario called `ford_cd4`, which adds made-up
`19 02 FF` answers on the usual Ford addresses:

| Module | Answer (made up) | Exercises |
|---|---|---|
| BCM 726 | three DTCs | multi-frame (ISO-TP) answer |
| ACM 727 | `7F 19 31` | negative response |
| APIM 7D0 | one DTC | single frame |
| IPC 720 | no DTCs | empty positive answer |
| PCM 7E0 | no UDS DTCs, one pending OBD DTC | OBD answer without count byte |

It also accepts `ATPB 81 04`, so `--bus ms` can be tested (the real emulator does not
know `ATPB`).

```sh
python3 tools/ford_emulator.py           # prints e.g. /dev/pts/3
fordiag --port /dev/pts/3 -y dtc --bus hs
fordiag --port /dev/pts/3 -y dtc --bus ms
```

The emulator does not separate HS and MS, so every module answers on both buses, and
`ATDP` always reports 500 kbit/s. The data says nothing about any real car. The
emulator has its own licence (CC BY-NC-SA 4.0); the script only uses it.

---

## 13. Using fordiag with Claude Code

The repository contains a Claude Code skill in `.claude/skills/fordiag/`. If you run
Claude Code **on your own PC** in the repository directory, with the cable plugged
in, Claude can run the fordiag commands for you and read the JSON results directly.

The skill keeps the same rules as the tool:
- It only uses fordiag's read-only commands, and it must not use other tools on the
  serial port.
- Before each measurement, it tells you what to do physically and waits for your
  confirmation before it runs the command with `-y`.
- You run the crank test yourself in your own terminal, because you have to start
  the engine at the right moment. Claude then reads the result.
- It does not interpret Ford-specific DTC meanings from memory.

A cloud session (claude.ai/code) cannot reach your cable. There, run the commands
yourself and share the output or the report.
