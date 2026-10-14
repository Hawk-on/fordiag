# fordiag – brukarmanual

`fordiag` er eit kommandolinjeverktøy som les diagnosedata frå ein Ford S-Max Mk2
(2016, 2.0 TDCi) via ein ELM327 USB-kabel. Verktøyet **kan berre lese**. Alt som
skriv til bilen, til dømes sletting av feilkodar eller nullstilling av
batteriovervakinga (BMS), gjer du i FORScan.

Manualen gjeld versjon 0.1 (fase 1 og 2: tilkopling og batteri).

---

## Innhald

1. [Før du byrjar](#1-før-du-byrjar)
2. [Oversikt over kommandoane](#2-oversikt-over-kommandoane)
3. [Globale flagg](#3-globale-flagg)
4. [`fordiag connect`](#4-fordiag-connect)
5. [`fordiag battery`](#5-fordiag-battery)
6. [`fordiag check`](#6-fordiag-check)
7. [Filer verktøyet lagar](#7-filer-verktøyet-lagar)
8. [Tryggleik og tillatsliste](#8-tryggleik-og-tillatsliste)
9. [Feilsøking](#9-feilsøking)
10. [Testing utan bil](#10-testing-utan-bil)

---

## 1. Før du byrjar

### Krav

- Linux (testa på Kubuntu) med Rust stable (`rustup`/`cargo`)
- ELM327 USB-kabel, typisk merkt «Ford OBD2 til USB», helst med HS/MS-brytar
- At brukaren din er i `dialout`-gruppa, slik at du får opne `/dev/ttyUSB0`:

  ```sh
  groups               # skal innehalde «dialout»
  sudo usermod -aG dialout $USER   # om ikkje; logg ut og inn att etterpå
  ```

### Bygging

```sh
cargo build --release
```

Binæren hamnar i `target/release/fordiag`. Døma under skriv berre `fordiag`. Du kan
anten bruke full sti, eller kopiere binæren til ein stad i `PATH`, t.d.
`cp target/release/fordiag ~/.local/bin/`.

### Viktig om kabelen

- **FORScan og fordiag kan ikkje bruke kabelen samstundes.** Lukk FORScan (og Wine)
  før du køyrer `fordiag`.
- Brytaren på kabelen avgjer kva buss du er kopla til:
  - **HS** (500 kbit/s): motorstyring (PCM) og standard OBD-II
  - **MS** (125 kbit/s): komfortmodular. Mange av desse ligg truleg der, men det er
    ikkje verifisert for denne bilen.
- Batteritestane bryr seg ikkje om brytaren, sidan dei berre måler spenning.

---

## 2. Oversikt over kommandoane

| Kommando | Formål | Treng bil? |
|---|---|---|
| `fordiag connect` | Test tilkopling: versjon, protokoll og spenning | Ja (eller `--mock`) |
| `fordiag battery --rest` | Kvilespenning | Ja |
| `fordiag battery --crank` | Lågaste spenning under start | Ja |
| `fordiag battery --charging` | Ladespenning med motoren i gang | Ja |
| `fordiag battery --summary` | Samla vurdering av lagra testar | Nei |
| `fordiag check "<kommando>"` | Sjekk ein kommando mot tillatslista | Nei |
| `fordiag help [kommando]` | Innebygd hjelp | Nei |

Planlagt seinare: `fordiag dtc` (feilkodar, fase 3) og `fordiag report`
(Markdown-rapport, fase 4).

---

## 3. Globale flagg

Desse flagga kan stå før eller etter kommandonamnet.

| Flagg | Standard | Tyding |
|---|---|---|
| `--port <STI>` | `/dev/ttyUSB0` | Seriellport, eller pseudoterminal frå ein emulator (t.d. `/dev/pts/3`) |
| `--baud <TAL>` | (auto) | Tving baudrate. Utan dette prøver fordiag 38400 og så 115200. |
| `--mock` | av | Bruk innebygd simulator i staden for kabel og bil |
| `--data-dir <MAPPE>` | `fordiag-data` | Kvar loggar og resultat blir lagra (relativt til der du står) |
| `-y`, `--yes` | av | Ikkje vent på Enter før målingar |
| `-h`, `--help` | | Vis hjelp |
| `-V`, `--version` | | Vis versjon |

**Returkode:** `0` når alt gjekk bra, og `1` ved feil. Feilmeldinga blir då skriven til
stderr, med prefikset `Feil:`.

---

## 4. `fordiag connect`

Opnar kabelen, køyrer initsekvensen og viser kva ELM327-brikka rapporterer.

```
fordiag connect [--bus hs|ms] [--probe]
```

| Flagg | Standard | Tyding |
|---|---|---|
| `--bus hs` | ✓ | HS-CAN 500 kbit/s: `ATSP6` |
| `--bus ms` | | MS-CAN 125 kbit/s: `ATPB 81 04` + `ATSPB`. Ber deg slå om brytaren først. |
| `--probe` | av | Sender i tillegg OBD `01 00` (støtta PID-ar) for å sjå om motorstyringa svarar |

### Kva skjer

1. Opnar porten med 38400 baud og spør `ATI`. Kjem det ikkje noko svar, blir 115200
   prøvd.
2. Initsekvens: `ATZ`, `ATE0`, `ATL0`, `ATS1`, `ATH1` og deretter protokoll (`ATSP6`
   eller `ATPB8104` + `ATSPB`).
3. Skriv ut `ATI` (versjon), `ATDP` (protokoll) og `ATRV` (spenning).
4. Med `--probe`: sender `01 00` og viser svaret.

Berre steg 4 sender noko ut på CAN-bussen. Dei andre stega snakkar berre med
ELM327-brikka.

### Fysisk

- Utan `--probe`: kabelen i, tenninga kan vere av.
- Med `--probe`: **tenninga på** (motoren treng ikkje gå), og brytaren på **HS**.

### Døme

```
$ fordiag connect --probe
Tilkopla: /dev/ttyUSB0 @ 38400 baud
ELM-versjon: ELM327 v1.5
Protokoll:   ISO 15765-4 (CAN 11/500)
Spenning:    12,60 V

Sender OBD «01 00» (støtta PID-ar) …
Svar:
  7E8 06 41 00 BE 3F A8 13

Rålogg: fordiag-data/logs/raw-20261014-164038.jsonl
```

Svarlinja byrjar med CAN-ID-en til modulen som svarte (`7E8` = svar frå motorstyringa
på førespurnad til `7E0`). Får du `NO DATA`, svarte ingen modul.

---

## 5. `fordiag battery`

Batteritestar basert på `ATRV`, altså spenninga ELM327 måler på pinne 16 i
OBD-kontakten (fast 12 V). Du må velje **nøyaktig éin** modus.

```
fordiag battery --rest     [--seconds N]
fordiag battery --crank    [--seconds N]
fordiag battery --charging [--minutes N | --seconds N]
fordiag battery --summary
```

| Flagg | Standard | Tyding |
|---|---|---|
| `--seconds N` | rest: 10, crank: 15 | Målelengd i sekund. For `--charging` overstyrer han `--minutes`. |
| `--minutes N` | 3 | Målelengd for `--charging` |

Batteritestane sender berre `ATZ`, `ATE0`, `ATL0` og `ATRV`. **Ingenting går ut på
CAN-bussen**, så bilen blir ikkje vekt av sjølve målinga.

Kvar test blir lagra som JSON i `fordiag-data/` (sjå [kapittel 7](#7-filer-verktøyet-lagar)).

### 5.1 `--rest`: kvilespenning

**Fysisk:** Bilen skal ha stått i fleire timar utan ladar og utan bruk. Tenninga skal
vere av. Opnar du døra for å kople til kabelen, vaknar modulane og trekkjer straum ei
stund. Lukk døra og vent 10–30 minutt før du måler. Har du nett ladd med smartladar,
bør batteriet kvile 12–24 timar først, elles måler du overflateladning.

Fordiag måler kvart 0,5 sekund og brukar **medianen**.

| Median | Vurdering |
|---|---|
| > 13,0 V | Ugyldig: motoren går eller ein ladar er kopla til |
| ≥ 12,6 V | Fullt og friskt |
| 12,4–12,5 V | Greitt, men ikkje heilt fullt |
| < 12,4 V | Utlada eller svekka batteri |

Du får åtvaring om spenninga varierer meir enn 0,3 V under målinga, sidan det tyder
på at noko i bilen framleis trekkjer straum.

### 5.2 `--crank`: starttest

**Fysisk:** Set deg i bilen og slå tenninga på. Køyr kommandoen, og start motoren 2–3
sekund etter at `>>> START NO <<<` kjem. Målinga varer i 15 sekund.

Fordiag samplar `ATRV` så raskt kabelen klarer, typisk kvart 20.–60. ms.

| Lågaste spenning | Vurdering |
|---|---|
| ≥ 10,0 V | Friskt |
| 9,6–10,0 V | Grensetilfelle |
| < 9,6 V | Svakt batteri |
| Inga fall på minst 1 V | Fann ikkje noko start. Vart motoren starta i tide? |

Utskrifta viser spenninga før start (median av første 1,5 s), den lågaste målinga og
spenninga etter start (median av siste 2 s). Du får åtvaring dersom:

- snittintervallet mellom målingar er over 100 ms, sidan botnpunktet då kan vere
  lågare enn målt
- det er eit hol på over 500 ms i målingane, sidan ELM-brikka då kan ha mista
  straumen under start
- spenninga før start er over 13 V
- spenninga etter start er under 13 V

### 5.3 `--charging`: ladetest

**Fysisk:** Start motoren og la han gå på tomgang heile målinga, som varer i 3 minutt.
Utskrifta viser ei måling kvart 10. sekund medan testen går.

Fordiag måler éin gong i sekundet og rapporterer:

- minimum, snitt og maksimum
- **trend** i V/min (lineær regresjon): stigande (> +0,05), fallande (< −0,05) eller
  stabil
- kor stor del av tida spenninga var ≥ 13,2 V

| Mønster | Vurdering |
|---|---|
| Maks > 15,0 V | Uvanleg høg, bør sjekkast |
| Maks < 13,0 V | Låg: generatoren ladar ikkje, eller smartladinga held igjen |
| Snitt 13,2–14,8 V og min ≥ 12,8 V | Normal |
| Anna | Varierande, vanleg med Ford si smartlading |

Ford si smartlading kan med vilje halde spenninga nede når BMS-en trur batteriet er
fullt. Får du «Låg», kan du gjenta testen med store forbrukarar på (lys, vifte,
bakrutevarme).

### 5.4 `--summary`: samla vurdering

Les dei siste lagra testane av kvar type i `--data-dir` og skriv ei samla vurdering.
Kommandoen treng verken kabel eller bil.

```
$ fordiag battery --summary
Siste lagra batteritestar i fordiag-data:
  Kvile:  12,50 V – Greitt, men ikkje heilt fullt (12,4–12,5 V)  (14.10.2026 16:40 [/dev/ttyUSB0 @ 38400 baud])
  Start:  lågaste 10,20 V – Friskt: …
  Lading: snitt 14,48 V – Innanfor vanleg ladeområde  (…)

Samla vurdering:
  • Kvilespenning og starttest ser normale ut. …
```

Resultat frå `--mock` er merka `[mock (ingen bil)]`. Bruk ei eiga `--data-dir` når du
testar, så dei ikkje blandar seg med ekte målingar.

### Avgrensingar

- ELM327 måler med 0,1 V oppløysing, og klonar kan bomme med 0,1–0,2 V. Samanlikn
  gjerne éin gong med eit multimeter på batteripolane.
- Spenninga i OBD-kontakten er litt lågare enn på polane, særleg under start.
- Spenning kan ikkje måle **kapasitet** (Ah). Ein sikker batteritest krev ein
  belastnings- eller konduktanstestar.
- Ladestatus (SOC) frå BCM blir ikkje lesen, fordi vi ikkje har nokon verifisert DID
  for denne bilen.

---

## 6. `fordiag check`

Sjekkar om ein kommando står på tillatslista, **utan å sende noko**. Kommandoen er
nyttig når du vil sjå kva verktøyet godtek.

```
fordiag check "<kommando>"
```

Tekst som byrjar med `AT`, blir tolka som AT-kommando. Alt anna blir tolka som hex
(mellomrom og små/store bokstavar spelar inga rolle).

```
$ fordiag check "19 02 ff"
TILLATEN: Diag(Uds(ReadDtcInformation(ByStatusMask(255)))) → sendast som «1902FF»

$ fordiag check "14 FF FF FF"
AVVIST: teneste 0x14 er FORBODEN (UDS: ClearDiagnosticInformation) – bruk FORScan om dette verkeleg trengst

$ fordiag check "ATSH 123"
AVVIST: CAN-ID 0x123 er utanfor diagnoseområdet 0x700–0x7FF
```

---

## 7. Filer verktøyet lagar

Alt hamnar under `--data-dir`, som er `fordiag-data/` dersom du ikkje vel noko anna.
Mappa er lagd til i `.gitignore`.

### `fordiag-data/logs/raw-ÅÅÅÅMMDD-TTMMSS.jsonl`

Rålogg med all kommunikasjon, éi JSON-linje per hending. Kvar linje blir skriven til
disk med ein gong, så loggen overlever om programmet krasjar.

```json
{"ts":"2026-10-14T16:40:38.918+02:00","dir":"info","data":"transport: /dev/ttyUSB0 @ 38400 baud"}
{"ts":"2026-10-14T16:40:38.918+02:00","dir":"tx","data":"ATZ"}
{"ts":"2026-10-14T16:40:38.943+02:00","dir":"rx","data":"\r\rELM327 v1.5\r\r"}
```

| `dir` | Tyding |
|---|---|
| `tx` | Sendt til kabelen (nøyaktig tekst, utan CR) |
| `rx` | Motteke frå kabelen (rått, utan `>`-prompten) |
| `err` | Feil, t.d. tidsavbrot eller avvist kommando |
| `info` | Hendingar som portopning og baud-søk |

Eit kjapt oversyn får du med `jq` (om du har det):

```sh
jq -r '"\(.ts) \(.dir) \(.data)"' fordiag-data/logs/raw-*.jsonl | less
```

### `fordiag-data/battery-<modus>-ÅÅÅÅMMDD-TTMMSS.json`

Resultatet frå éin batteritest: starttid, kjelde, analyse og **alle målingane**
(`t_ms`, `volts`). Det er desse filene `--summary` les, og seinare `report`.

---

## 8. Tryggleik og tillatsliste

Tillatslista er handheva i koden (`src/command.rs`), ikkje berre i dokumentasjonen.
Det finst ingen måte å sende vilkårlege bytes på. Fordiag har heller ingen kommando
for å sende rå hex.

### Tillate

| Type | Kommandoar |
|---|---|
| AT til ELM327 | `ATZ`, `ATE0`, `ATL0`, `ATS0/1`, `ATH0/1`, `ATSP0/6/B`, `ATPB 81 04`, `ATSH 7xx`, `ATCRA 7xx`, `ATCRA`, `ATCAF1`, `ATST xx`, `ATRV`, `ATI`, `ATDP` |
| OBD-II | `01 pp`, `03`, `07`, `09 pp`, `0A` |
| UDS | `19 01 mm`, `19 02 mm`, `19 0A`, `22 hhll` (éin DID), `3E 00` |

### Avvist, mellom anna

| Kva | Kvifor |
|---|---|
| `04` | Slettar OBD-feilkodar |
| `08` | OBD: styrer system om bord |
| `10` | Byter diagnosesesjon |
| `11` | Restartar modul (ECU-reset) |
| `14` | Slettar UDS-feilkodar |
| `27` | Security access |
| `28` | Kommunikasjonskontroll |
| `2E`, `3D` | Skriv data eller minne |
| `2F` | Styrer utgangar |
| `31` | Køyrer rutinar |
| `34`–`37` | Nedlasting/opplasting |
| `85` | Slår av/på DTC-registrering |
| `3E 80` | Berre `3E 00` er tillate |
| `ATCAF0` | Ville gjort det mogleg å sende rå CAN-rammer |
| `ATMA`, `ATBRD`, `ATCV`, `ATPP…`, `ATWM…` | Ikkje nødvendig, og kan endre brikka |
| `ATPB` med anna enn `81 04` | Feil bitrate kan forstyrre bussen |
| `ATSH`/`ATCRA` utanfor 0x700–0x7FF | Avgrensa til diagnose-ID-ar |

Andre tryggleikstiltak:

- Rett før noko blir skrive, blir teksten tolka på nytt. Han må gje nøyaktig same
  kommando, elles blir han ikkje send.
- Fordiag sender aldri ei tom linje. Ein tom CR får ELM327 til å gjenta *førre*
  kommando, og den kunne vore noko FORScan sende.

Treng du noko som ikkje står på lista, gjer det i **FORScan**.

---

## 9. Feilsøking

| Symptom | Moglege årsaker og tiltak |
|---|---|
| `klarte ikkje opne /dev/ttyUSB0` | Kabelen er ikkje i, FORScan/Wine held porten, eller du manglar `dialout`. Sjekk `ls -l /dev/ttyUSB*` og `groups`. |
| `Permission denied` | Legg deg i `dialout` og logg ut og inn att. |
| `fann ingen ELM327 på …` | Kabelen er ikkje kopla til bilen (ELM-en får straum frå OBD-kontakten), feil port, eller uvanleg baudrate. Prøv `--baud 9600` eller `--baud 115200`. |
| `NO DATA` på `--probe` | Tenninga er av, brytaren står på MS, eller modulen søv. |
| `CAN ERROR` / `BUS ERROR` | Truleg feil buss for brytarstillinga, t.d. `--bus ms` med brytaren på HS. |
| `ELM327 svarte «?»` | Brikka kjenner ikkje kommandoen. Enkle klonar manglar ofte `ATPB` (trengst for `--bus ms`). |
| `tidsavbrot` | Laus kabel, eller ELM-en har mista straumen. Sjå `err`-linjene i råloggen. |
| Kvile-test seier «Ugyldig» | Motoren går, ein ladar er tilkopla, eller du målte for kort tid etter køyring eller lading. |
| Starttest finn ikkje noko start | Motoren vart starta for seint. Prøv `--seconds 25`. |
| Spenninga skil seg frå multimeteret | Klonar kan bomme med 0,1–0,2 V. Hugs differansen når du les tala. |

Legg ved den nyaste `raw-*.jsonl` når du ber om hjelp.

---

## 10. Testing utan bil

### Innebygd simulator (`--mock`)

Alle kommandoane fungerer med `--mock`. Simulatoren hoppar òg over «Trykk Enter».

```sh
D=$(mktemp -d)
fordiag --mock --data-dir $D connect --probe
fordiag --mock --data-dir $D battery --rest --seconds 3
fordiag --mock --data-dir $D battery --crank --seconds 8
fordiag --mock --data-dir $D battery --charging --seconds 20
fordiag --mock --data-dir $D battery --summary
```

### Einingstestar

```sh
cargo test
```

Testane dekkjer tillatslista (alle 256 tenestebytar), svartolking, init-sekvensar mot
`MockTransport` og analysane med grenseverdiane.

### ELM327-emulator (testar serielaget)

```sh
pip install ELM327-emulator
python3 -m elm -s car          # skriv ut t.d. /dev/pts/3
fordiag --port /dev/pts/3 connect --probe
```

Emulatoren rapporterer spenning rundt 13 V, så `--rest` vil melde «Ugyldig». Det er
venta.
