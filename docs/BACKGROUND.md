# Background: what other owners have found

Context gathered from public forums, recall databases and tool documentation for the
**Ford S-Max Mk2 (CJ)**. The same CD4 platform is shared with the Galaxy Mk3 and
Mondeo Mk5, so reports from those cars are included where relevant.

> [!IMPORTANT]
> Everything here is **community reports or third-party information, not verified on
> this car**. Use it to decide what to check, not as a diagnosis. Confirm anything
> important against official Ford documentation or a Ford workshop.

Vehicle: S-Max Mk2, first registered 2016-03, 2.0 Duratorq TDCi 150 hp (110 kW),
6-speed PowerShift automatic (MPS6), FWD, SYNC 2, about 150 500 km (Oct 2026).
Battery: EFB 12 V 75 Ah 730 A (EN).

---

## 1. Safety recall 18P11: battery monitoring sensor (check this first)

**What:** In 2019 Ford recalled about 322 000 **Mondeo, S-Max and Galaxy built
between February 2014 and February 2019** in Europe ([Carscoops][carscoops],
[Quelle Occasion][qo]).

**Why:** Battery acid can leak around the negative terminal. Copper sulphate can then
build up under the battery monitoring sensor (BMS) and form a low-resistance path.
The surrounding material may then heat up and ignite, possibly while the car is
parked ([Australian recall REC-000317][au]).

**Remedy:** Owners on talkford.com describe the fix as an intermediate cable with a
fuse, not a new sensor ([talkford: drain since recall][tf-drain]).
[Quelle Occasion][qo] says the same.

**Relevance here:** This car (built about early 2016) falls inside the build window,
and the sensor in question is the one the BMS uses to estimate the state of charge.

**Status for this car (checked 2026-10-15 on ford.no/brukerstoette/tilbakekallinger):**
the result was "Ingen tilbakekallinger" (no recalls) and "Intet kundetilfredshetsprogram"
(no customer satisfaction programme). The lookup most likely only lists outstanding
recalls, so 18P11 has **probably already been done**. A Ford dealer can confirm this from
the service history, and the fused intermediate cable at the negative terminal is a
physical sign.

**Action:**
- Check whether 18P11 has been done. Use the VIN on Ford's recall page, or ask a Ford
  dealer. The service history may also show it.
- Look at the negative terminal and the sensor for corrosion or white/blue-green
  deposits. Do not touch it while you are wearing metal jewellery, and do not
  disconnect anything you are unsure about.

One 2016 owner reported a battery drain after the recall work (about 3 A
continuously). The thread later concluded that the drain was mostly a measurement
artefact caused by the test setup and a connected smart charger ([talkford][tf-recall]).
It is therefore not a known side effect, but it shows how easily measurements of
this kind mislead.

---

## 2. How the BMS sees (or does not see) charging

Ford's battery monitoring sensor sits on the **negative battery cable** and measures
the current going in and out of the battery. From that it estimates the state of
charge (SOC) and the battery's health.

- **Where the charger is connected matters.** If the charger's negative clamp goes
  directly on the battery's negative post, the charging current bypasses the sensor,
  and the BMS does not register that the battery was charged ([f150ecoboost.net][f150]).
  Ford's usual advice is to connect the negative clamp to a **body/engine ground
  point**, so the current flows through the sensor. Check the S-Max owner's manual
  for the exact point on this car.
- **The BMS recalibrates while the car rests.** Reports say it needs a quiescent
  period of at least about 4 hours ([fordtransitusaforum][transit]). Snap-on says
  that letting the car sit for about 8 hours, or forcing a reset with a scan tool,
  restores it ([Snap-on][snapon]).
- **Load shedding.** When the BMS believes the SOC is low, it switches off
  accessories to protect starting ability. Snap-on gives the example of the radio
  stopping working after the key was left on ([Snap-on][snapon]).
- **The charging target is deliberately below 100 %.** A 2016 S-Max 2.0 TDCi owner
  reported setting a 95 % target in FORScan, and the car still only charged to
  70–80 % ([Facebook][fb-soc]). FORScan lists the BCM settings as "BMS SOC Max Cfg"
  and "BMS SOC Off Cfg" at block `726-06`. That information comes from an F-150
  list, so it is unverified for this car.

**Hypothesis for this car (unverified):** If the Biltema charger was clamped
directly to the battery posts, the BMS may never have seen the recharge after the
deep discharge. It would then still believe the SOC is very low and keep shedding
load. That fits the 12 V outlet cutting out even with the engine running.

**Update (2026-10-15): this hypothesis is unlikely.** The owner confirms the charger was
connected correctly: positive on the battery post, negative on a ground point. The
charging current should therefore have passed through the sensor.

Remaining explanations (all unverified):
- The BMS has not yet had a long enough rest (≥ 4–8 h asleep) to recalibrate after
  the charge.
- The battery was actually damaged by the deep discharges (below 9 V), and the BMS has
  correctly estimated reduced health or capacity.
- The sensor itself, or its connection, is faulty (see recall 18P11 above).

The battery tests and the DTCs from the BCM (phase 3) should help tell these apart.

---

## 3. BMS reset

- A **BMS reset** tells the car that a new battery has been fitted, and it resets the
  recorded battery age. Several owners warn **against resetting the BMS on an old
  battery** ([fordownersclub][foc-ss], [kugaownersclub][kuga]). Do it only after a
  battery replacement, and in FORScan (it is out of scope for fordiag).
- A widely shared **manual procedure** exists, but the forum post about it was for an
  older Mondeo, and it reportedly does not work on all models ([talkford][tf-bms],
  [fordescape][fe-bms]). The procedure: within 10 s, ignition on, rear fog switch on/off
  5×, then hazard switch on/off 3×. The battery light is meant to flash 3×. Treat it
  as unverified on the S-Max Mk2.
- One YouTube guide for the Galaxy Mk3 / S-Max Mk2 says a "low battery" warning after
  fitting a new battery needs both a BMS and a BCM reset ([YouTube][yt-galaxy]).

---

## 4. Infotainment (SYNC 2 / ACM) after a flat battery

- On these cars the radio audio is handled by the **ACM** (audio control module). The
  **APIM** runs SYNC (screen, Bluetooth, voice). The ACM can misbehave while SYNC
  looks fine ([Reddit r/cmaxhybrid][reddit-acm]).
- A **2013 S-Max** owner had a flat battery and then a jump start. Afterwards the
  central locking acted up and the radio lost sound. A battery disconnect fixed the
  locking, but the audio fault remained. Forum members suggested reloading as-built
  data, updating firmware or replacing the ACM ([fordownersclub][foc-smax]).
- Ford's own SYNC soft reset: press the audio power button five times quickly, or
  hold it for 10 seconds ([Ford][ford-sync]). This is safe to try, and it is
  independent of fordiag.
- Low voltage can make several modules log U-codes (lost communication). Expect some
  of those in phase 3, and read them as possible after-effects of the discharges, not
  necessarily as faults in the modules themselves.

---

## 5. PowerShift (MPS6) and other modules

- PowerShift TCMs are sensitive to low voltage. In "Transmission limited function"
  threads, the first advice is to **fully charge the battery** and then check whether
  the fault clears ([fordownersclub][foc-trans]). If a transmission warning appears,
  charge first and read DTCs before anything else.
- Relearn or adaptation procedures exist, but they are write operations and
  therefore **FORScan/workshop only**.

---

## 6. Diagnostic tooling notes from other owners

- Owners of 2013-and-later S-Max cars are generally advised to use an adapter with
  **automatic HS/MS-CAN switching** (e.g. vLinker FS) instead of an older ELM327.
  FORScan only sees the MS-CAN modules when the bus is switched ([fordownersclub][foc-smax],
  [bronco6g][bronco]). An S-Max Mk2 owner on the FORScan forum uses a vLinker FS
  ([FORScan forum][forscan-ipc]). Our cable has a manual switch, so `--bus ms` plus
  flipping the switch is the equivalent.
- Ford module addresses are largely shared across models. In FORScan lists for other
  Fords (F-150), IPC = 720, BCM = 726, ACM = 727, RCM = 737, ABS = 760 and
  APIM = 7D0 ([Scribd F-150 list][scribd]). This matches our candidate list, but
  **it is not verified for the S-Max**. Phase 3 must confirm each module by sending a
  read request and seeing whether it answers. Some newer Fords route traffic through a
  gateway module (GWM) ([MaverickTruckClub][maverick]), and it is unknown whether the
  2016 CD4 cars do.

---

## Suggested next steps (in order)

1. Check whether **recall 18P11** has been carried out on this car (VIN on Ford's
   recall page, or ask a dealer).
2. Look at the negative terminal and the BMS sensor for corrosion.
3. Charge as before (negative clamp on the ground point, which was already done), then
   let the car sleep, locked, for at least 4–8 hours.
4. Run `fordiag battery --rest`, `--crank` and `--charging`.
5. Read DTCs (phase 3), first on HS-CAN (PCM), then on MS-CAN (BCM, ACM, APIM, IPC),
   confirming each address as we go.
6. Only after a **battery replacement**: do a BMS reset in FORScan with battery type
   EFB.

---

## Sources

[carscoops]: https://www.carscoops.com/2019/09/ford-recalling-322000-cars-due-to-battery-fire-risks-in-europe/
[qo]: https://quelleoccasion.com/en/guides/ford-galaxy-iii/pannes
[au]: https://www.vehiclerecalls.gov.au/recalls/rec-000317
[tf-drain]: https://www.talkford.com/threads/battery-drain-since-bms-safety-recall-fix.395431/
[tf-recall]: https://www.talkford.com/threads/documentation-18p11-battery-monitoring-sensor-safety-recall.393307/
[f150]: https://www.f150ecoboost.net/threads/interesting-battery-charging-video-regarding-bms.87159/
[transit]: https://www.fordtransitusaforum.com/threads/ford-bms-battery-monitoring-system-how-it-works-and-how-to-properly-charge-your-battery.92592/
[snapon]: https://www.snapon.com/Diagnostics/US/KB/DQT-AA-Ford-Battery-Monitor-System.htm
[fb-soc]: https://www.facebook.com/61591343434644/posts/battery-swap-this-morning-for-a-ford-s-max/122117280411378114/
[foc-ss]: https://www.fordownersclub.com/forums/topic/120397-auto-stopstart-not-working/
[kuga]: https://www.kugaownersclub.co.uk/threads/auto-stop-start.29339/
[tf-bms]: https://www.talkford.com/threads/battery-monitor-system-reset.337385/
[fe-bms]: https://www.fordescape.org/threads/this-may-reset-fords-battery-management-system-bms-without-forscan-doesnt-work-on-all-models.123091/
[yt-galaxy]: https://www.youtube.com/watch?v=OdJ8Wl6tV6Y
[reddit-acm]: https://www.reddit.com/r/cmaxhybrid/comments/1f5de8v/sync_2_system_not_working/
[foc-smax]: https://www.fordownersclub.com/forums/topic/148232-will-forscan-work-the-same-on-an-s-max/
[ford-sync]: https://www.ford.com/support/how-tos/sync/troubleshooting/how-do-i-reset-the-sync-system
[foc-trans]: https://www.fordownersclub.com/forums/topic/149004-transmission-limited-function-ford-s-max-auto-2010/
[bronco]: https://www.bronco6g.com/forum/threads/forscan-not-giving-access-to-all-modules.73504/
[forscan-ipc]: https://forum.forscan.org/viewtopic.php?t=17197&start=20
[scribd]: https://www.scribd.com/document/586086720/FORScan-2015-2020-F150s-Livnitup
[maverick]: https://www.mavericktruckclub.com/forum/threads/forscan-101-step-by-step-how-to-start-guide.7406/post-1323324

- Carscoops: [Ford recalling 322,000 cars due to battery fire risks in Europe][carscoops] (2019)
- Quelle Occasion: [Ford Galaxy III known faults][qo]
- Australian Government: [Recall REC-000317, Ford Mondeo 2014–2018][au]
- talkford.com: [Battery drain since BMS safety recall fix][tf-drain], [18P11 documentation][tf-recall], [Battery Monitor System Reset][tf-bms]
- f150ecoboost.net: [Battery charging and the BMS][f150]
- fordtransitusaforum.com: [Ford BMS: how it works and how to charge][transit]
- Snap-on: [Quick Tip: Ford Battery Monitor System][snapon]
- Facebook: [S-Max Mk2 2016 2.0 TDCi SOC target][fb-soc]
- fordownersclub.com: [Auto Stop/Start not working][foc-ss], [Will FORScan work the same on an S-Max?][foc-smax], [Transmission Limited Function – S-Max][foc-trans]
- kugaownersclub.co.uk: [Auto stop start][kuga]
- fordescape.org: [BMS reset without FORScan][fe-bms]
- YouTube: [Ford Galaxy / S-Max battery reset][yt-galaxy]
- Reddit r/cmaxhybrid: [SYNC 2 not working (ACM)][reddit-acm]
- Ford: [How do I reset the SYNC system?][ford-sync]
- bronco6g.com: [FORScan not giving access to all modules][bronco]
- FORScan forum: [IPC firmware files (Mondeo Mk5)][forscan-ipc]
- Scribd: [FORScan 2015–2020 F-150 settings list][scribd]
- MaverickTruckClub: [FORScan 101][maverick]
