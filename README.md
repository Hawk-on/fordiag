# fordiag

Les-berre diagnoseverktøy for Ford S-Max Mk2 (2.0 TDCi) via ELM327 USB-kabel.

**Verktøyet kan berre lese.** Alt som blir sendt til kabelen må vere ein variant
i tillatslista i `src/command.rs`. Sletting av feilkodar, skriving, rutinar,
reset, sesjonsbyte, security access og liknande kan ikkje uttrykkjast i koden.
Slikt gjer du i FORScan.

## Bygg og test

```sh
cargo build --release
cargo test            # alt er testa mot MockTransport, ingen bil trengst
```

Binæren hamnar i `target/release/fordiag`.

## Kommandoar

| Kommando | Kva han gjer |
|---|---|
| `fordiag connect [--bus hs\|ms] [--probe]` | Init (`ATZ ATE0 ATL0 ATS1 ATH1 ATSP6`), viser `ATI`, `ATDP` og `ATRV`. `--probe` sender òg OBD `01 00`. |
| `fordiag battery --rest [--seconds 10]` | Kvilespenning (median av målingar kvart 0,5 s) |
| `fordiag battery --crank [--seconds 15]` | Lågaste spenning medan du startar motoren |
| `fordiag battery --charging [--minutes 3]` | Ladespenning: min/maks/snitt og trend |
| `fordiag battery --summary` | Samla vurdering av dei siste lagra testane |
| `fordiag check "<kommando>"` | Sjekkar ein kommando mot tillatslista **utan** å sende han |

Globale flagg: `--port /dev/ttyUSB0`, `--baud 38400`, `--mock` (innebygd
simulator), `--data-dir fordiag-data`, `-y/--yes` (ikkje vent på Enter).

Batteritestane sender berre `ATZ`, `ATE0`, `ATL0` og `ATRV`. Ingen av dei går
ut på CAN-bussen, så bilen blir ikkje vekt av målinga.

## Filer

* `fordiag-data/logs/raw-*.jsonl`: rålogg med all kommunikasjon (sendt og
  motteke) med tidsstempel
* `fordiag-data/battery-*.json`: resultat og alle målingar frå batteritestane

## Test mot ELM327-emulator (valfritt)

```sh
pip install ELM327-emulator
python3 -m elm -s car        # skriv ut t.d. /dev/pts/3
fordiag --port /dev/pts/3 connect --probe
```

## Kodeoversikt

* `command.rs`: tillatsliste (typar for AT-kommandoar og diagnoseførespurnader) og testar
* `transport.rs`: `Transport`-trait, `SerialTransport`, `MockTransport` og rålogg
* `elm.rs`: init, svartolking og baud-autodeteksjon
* `battery.rs`: sampling og analyse av spenning
* `main.rs`: CLI og simulator for `--mock`

## Status

- [x] Fase 1: tilkopling
- [x] Fase 2: batteristatus
- [ ] Fase 3: feilkodar (`fordiag dtc`)
- [ ] Fase 4: rapport (`fordiag report`)
