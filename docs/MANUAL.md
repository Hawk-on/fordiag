# fordiag – user manual

`fordiag` is a command-line tool that reads diagnostic data from a Ford S-Max Mk2
(2016, 2.0 TDCi) through an ELM327 USB cable. The tool **can only read**. Anything
that writes to the car, such as clearing DTCs or resetting the battery monitoring
system (BMS), is done in FORScan.

This manual covers version 0.1 (phases 1 and 2: connection and battery).

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
6. [`fordiag check`](#6-fordiag-check)
7. [Files created by the tool](#7-files-created-by-the-tool)
8. [Safety and the allowlist](#8-safety-and-the-allowlist)
9. [Troubleshooting](#9-troubleshooting)
10. [Testing without a car](#10-testing-without-a-car)

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

### Building

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
| `fordiag check "<command>"` | Check a command against the allowlist | No |
| `fordiag help [command]` | Built-in help | No |

Planned: `fordiag dtc` (DTCs, phase 3) and `fordiag report` (Markdown report,
phase 4).

---

## 3. Global flags

These flags can go before or after the command name.

| Flag | Default | Meaning |
|---|---|---|
| `--port <PATH>` | `/dev/ttyUSB0` | Serial port, or a pseudo-terminal from an emulator (e.g. `/dev/pts/3`) |
| `--baud <N>` | (auto) | Force a baud rate. Without it, fordiag tries 38400 and then 115200. |
| `--mock` | off | Use the built-in simulator instead of cable and car |
| `--data-dir <DIR>` | `fordiag-data` | Where logs and results are stored (relative to the current directory) |
| `-y`, `--yes` | off | Do not wait for Enter before measurements |
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
[chapter 7](#7-files-created-by-the-tool)).

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

## 6. `fordiag check`

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

## 7. Files created by the tool

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

### `fordiag-data/battery-<mode>-YYYYMMDD-HHMMSS.json`

The result of one battery test: start time, source, analysis and **every
measurement** (`t_ms`, `volts`). These are the files `--summary` reads, and later
`report`.

---

## 8. Safety and the allowlist

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
- fordiag never sends an empty line. A bare CR makes the ELM327 repeat the *previous*
  command, which could have been something FORScan sent.

If you need something that is not on the list, do it in **FORScan**.

---

## 9. Troubleshooting

| Symptom | Possible causes and fixes |
|---|---|
| `could not open /dev/ttyUSB0` | The cable is not plugged in, FORScan/Wine is holding the port, or you are not in `dialout`. Check `ls -l /dev/ttyUSB*` and `groups`. |
| `Permission denied` | Add yourself to `dialout`, then log out and back in. |
| `no ELM327 found on …` | The cable is not connected to the car (the ELM is powered by the OBD connector), the port is wrong, or the baud rate is unusual. Try `--baud 9600` or `--baud 115200`. |
| `NO DATA` with `--probe` | The ignition is off, the switch is on MS, or the module is asleep. |
| `CAN ERROR` / `BUS ERROR` | Probably the wrong bus for the switch position, e.g. `--bus ms` with the switch on HS. |
| `ELM327 replied "?"` | The chip does not know the command. Basic clones often lack `ATPB` (needed for `--bus ms`). |
| `timeout` | Loose cable, or the ELM lost power. Look at the `err` lines in the raw log. |
| Rest test says "Invalid" | The engine is running, a charger is connected, or you measured too soon after driving or charging. |
| Crank test finds no cranking | The engine was started too late. Try `--seconds 25`. |
| Voltage differs from the multimeter | Clones can be off by 0.1–0.2 V. Keep the difference in mind when reading the numbers. |

Attach the latest `raw-*.jsonl` when asking for help.

---

## 10. Testing without a car

### Built-in simulator (`--mock`)

All commands work with `--mock`. The simulator also skips "Press Enter".

```sh
D=$(mktemp -d)
fordiag --mock --data-dir $D connect --probe
fordiag --mock --data-dir $D battery --rest --seconds 3
fordiag --mock --data-dir $D battery --crank --seconds 8
fordiag --mock --data-dir $D battery --charging --seconds 20
fordiag --mock --data-dir $D battery --summary
```

### Unit tests

```sh
cargo test
```

The tests cover the allowlist (all 256 service bytes), response parsing, init
sequences against `MockTransport`, and the analyses with their thresholds.

### ELM327 emulator (tests the serial layer)

```sh
pip install ELM327-emulator
python3 -m elm -s car          # prints e.g. /dev/pts/3
fordiag --port /dev/pts/3 connect --probe
```

The emulator reports about 13 V, so `--rest` will say "Invalid". That is expected.
