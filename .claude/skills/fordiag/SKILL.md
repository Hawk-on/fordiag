---
name: fordiag
description: Drive the read-only fordiag CLI on the owner's own PC to diagnose their Ford S-Max Mk2 through the ELM327 cable - connection check, battery tests, DTC scans and the Markdown report - and interpret the saved JSON results. Use when the owner asks to test the battery, read fault codes, check the connection to the car, or make or explain a fordiag report.
allowed-tools: Read, Glob, Grep, Bash(fordiag *), Bash(./target/release/fordiag *), Bash(cargo build *), Bash(cargo test *), Bash(jq *), Bash(ls *)
---

# fordiag: diagnosing the car with the owner

You are helping the owner run `fordiag` on their own Linux PC, with the ELM327 cable
plugged into their Ford S-Max Mk2. Read `CLAUDE.md` first for the project rules and
`docs/MANUAL.md` for the full command reference. `docs/BACKGROUND.md` has what other
owners have found.

## Hard rules

- **fordiag only reads.** Never try to clear DTCs, reset modules, reset the BMS,
  calibrate anything or send commands that are not on the allowlist. There is no
  command for it, and you must not look for workarounds: no other tools on the
  serial port (`screen`, `minicom`, `python`, `echo > /dev/ttyUSB0`, …), and no
  edits to the allowlist in `src/command.rs` to get around a refusal. If the owner
  asks for any of this, say no and point to FORScan.
- **Never run a measurement before the owner has confirmed the physical setup** in
  the chat (ignition, engine, cable switch, car at rest). Only then run the command
  with `-y`. fordiag refuses to start without keyboard input unless `-y` is given,
  and that refusal is deliberate: do not add `-y` before the owner has confirmed.
- **FORScan and fordiag cannot share the cable.** Ask the owner to close FORScan and
  Wine first.
- **Do not interpret DTC meanings from memory.** fordiag prints the code, a category,
  a generic failure type (a suggestion) and the status bits. For the meaning, refer
  the owner to FORScan or Ford documentation. If you add context, mark it clearly as
  a suggestion to be checked against Ford documentation, and never invent meanings
  for Ford-specific B and U codes.
- **Module addresses are unverified** until a module has answered on the real car.
- **Privacy:** do not write the VIN or the registration number into the repository,
  the report or any commit.
- Reply in the owner's language (usually Norwegian Nynorsk). Code, CLI output and
  docs stay in English.

## Finding the binary

Use `fordiag` if it is on `PATH`, otherwise `./target/release/fordiag`. If neither
exists, run `cargo build --release` first. Results are saved in `fordiag-data/` in
the current directory unless `--data-dir` is given.

## Steps and what the owner must do physically

Do one step at a time. Before each step, tell the owner exactly what to do, wait for
their confirmation, then run the command, then explain the result briefly.

| Step | Owner does first | Command |
|---|---|---|
| Connection | Cable in, switch on **HS**, ignition **on**, engine off | `fordiag -y connect --probe` |
| Resting voltage | Car parked for several hours, no charger, ignition **off**, doors closed for 10–30 min after plugging in | `fordiag -y battery --rest` |
| Crank test | **Let the owner run this one in their own terminal** (see below) | `fordiag battery --crank` |
| Charging | Engine **running** at idle for the whole test (3 min) | `fordiag -y battery --charging` |
| DTCs, HS | Switch on **HS**, ignition **on**, engine off | `fordiag -y dtc --bus hs` |
| DTCs, MS | Switch on **MS**, ignition **on**, engine off | `fordiag -y dtc --bus ms` |
| Summary | Nothing | `fordiag battery --summary` |
| Report | Nothing (no cable needed) | `fordiag report --vehicle "Ford S-Max Mk2 2.0 TDCi"` |

**Crank test:** the owner has to start the engine 2–3 seconds after `>>> START NOW <<<`
appears, and you do not see the output until the command finishes. Ask the owner to
run `fordiag battery --crank` in their own terminal (it waits for Enter), then read
the result with `fordiag battery --summary` or from the newest
`fordiag-data/battery-crank-*.json`.

Long-running commands (`--charging` takes 3 minutes) can run in the background; tell
the owner how long it will take and not to switch the engine off before it is done.

## Reading the results

- Battery: `fordiag-data/battery-<mode>-*.json`, with `analysis` (verdict,
  warnings, key values) and every sample. `fordiag battery --summary` gives the
  overall assessment.
- DTCs: `fordiag-data/dtc-<bus>-*.json`, with one entry per module. Each `reads[]`
  item has an `outcome` with `kind` set to `dtcs`, `negative`, `no_response` or
  `error`, plus the raw response lines. Example:
  `jq '.modules[] | {name, reads: [.reads[] | {label, outcome}]}' fordiag-data/dtc-ms-*.json`
- Raw logs: `fordiag-data/logs/raw-*.jsonl`. Each result file names its log in
  `raw_log`.

When a module responds on the real car for the first time, suggest updating the
address table in `CLAUDE.md` (and `dtc::CANDIDATES` if needed) with the date and the
bus. When a recording is useful as a regression test, suggest copying the raw log
and its result file to `fixtures/replay/` (see `docs/MANUAL.md`, chapter 12), after
the owner has checked it.

## Troubleshooting

See `docs/MANUAL.md`, chapter 11. The most common causes are: FORScan still holding
the port, the user not being in the `dialout` group, the switch not matching `--bus`,
and the ignition being off.
