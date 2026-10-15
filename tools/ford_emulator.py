#!/usr/bin/env python3
"""
Start ELM327-emulator with a Ford CD4-like scenario ("ford_cd4") for testing
fordiag's serial layer without a car.

The scenario is a copy of the emulator's built-in "car" scenario plus UDS
answers to `19 02 FF` (ReadDTCInformation) from modules on the usual Ford
addresses (BCM 726, ACM 727, APIM 7D0, IPC 720, PCM 7E0).

ALL DATA HERE IS MADE UP. The DTCs are examples chosen to exercise the parser
(single frame, multi-frame, negative response, no DTCs). They say nothing about
any real car, and the addresses are unverified candidates (see CLAUDE.md).

Requires ELM327-emulator (`pip install ELM327-emulator`). That package has its
own licence (CC BY-NC-SA 4.0); this script only uses it and does not copy it.

Usage (all emulator options are passed through):

    python3 tools/ford_emulator.py            # prints e.g. /dev/pts/3
    fordiag --port /dev/pts/3 dtc --bus hs -y

Note: the emulator does not separate HS-CAN and MS-CAN, so every module answers
whatever --bus is used.
"""

import copy
import sys

try:
    from elm import obd_message as om
    from elm.interpreter import main
except ImportError:
    sys.exit("ELM327-emulator is not installed: pip install ELM327-emulator")

SCENARIO = "ford_cd4"


def uds_dtcs(*records):
    """Payload of a positive 59 02 response: availability mask FF plus
    (DTC high, DTC mid, failure type, status) for each record."""
    data = ["59", "02", "FF"]
    for r in records:
        data += r.split()
    return data


# (name, request header, response header, payload of the answer to 19 02 FF)
MODULES = [
    # Multi-frame answer with three DTCs (made up).
    ("BCM", "726", "72E", uds_dtcs("F0 03 16 08", "92 00 00 28", "91 17 62 28")),
    # Negative response: request out of range.
    ("ACM", "727", "72F", ["7F", "19", "31"]),
    # Single frame, one DTC (made up).
    ("APIM", "7D0", "7D8", uds_dtcs("C1 55 00 2F")),
    # Answers, but no DTCs.
    ("IPC", "720", "728", uds_dtcs()),
    # The PCM answers UDS too (no DTCs).
    ("PCM", "7E0", "7E8", uds_dtcs()),
]


def build_scenario():
    scenario = copy.deepcopy(om.ObdMessage["car"])
    # The emulator does not know ATPB (protocol B setup, used for MS-CAN), so
    # accept fordiag's one allowed configuration.
    scenario["FORD_ATPB"] = {
        "Request": "^ATPB8104" + om.ELM_FOOTER,
        "Descr": "Protocol B options and baud rate (MS-CAN 125 kbit/s)",
        "Response": om.ELM_R_OK,
    }
    for name, request, response, payload in MODULES:
        scenario[f"FORD_{name}_DTC"] = {
            "Request": "^1902FF" + om.ELM_FOOTER,
            "Descr": f"{name}: ReadDTCInformation by status mask (made-up data)",
            "Header": request,
            "Response": om.iso_tp_frames(payload, header=response),
        }
    return scenario


def run():
    om.ObdMessage[SCENARIO] = build_scenario()
    # A pending OBD DTC on the PCM (mode 07), also made up: P0563.
    om.DTC_PENDING[:] = ["0563"]
    if "-s" not in sys.argv and "--scenario" not in sys.argv:
        sys.argv[1:1] = ["-s", SCENARIO]
    main()


if __name__ == "__main__":
    run()
