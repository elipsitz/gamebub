"""Decode /sdcard/cartlog.bin (physical cartridge header reads logged by the GBA core) and
compare each read against a ROM dump.

Usage: python scripts/parse_cart_log.py cartlog.bin game.gba
"""

import struct
import sys


def main() -> None:
    log = open(sys.argv[1], "rb").read()
    rom = open(sys.argv[2], "rb").read() if len(sys.argv) > 2 else None
    (count,) = struct.unpack_from("<I", log)
    entries = [struct.unpack_from("<II", log, 4 + 8 * i) for i in range((len(log) - 4) // 8)]
    print(f"{count} reads logged, {len(entries)} stored")
    bad = 0
    prev_cycle = None
    for lo, hi in entries:
        data = lo & 0xFFFF
        address = ((hi & 0xFF) << 16 | lo >> 16) << 1
        cycle = hi >> 8
        gap = "" if prev_cycle is None else f"+{(cycle - prev_cycle) & 0xFFFFFF}"
        prev_cycle = cycle
        expected = None
        if rom is not None and address < len(rom):
            (expected,) = struct.unpack_from("<H", rom, address)
        mark = ""
        if expected is not None and expected != data:
            mark = f"  MISMATCH expected {expected:04x} (xor {expected ^ data:04x})"
            bad += 1
        print(f"{cycle:8d} {gap:>8} 0x{0x08000000 + address:08x} {data:04x}{mark}")
    print(f"{bad} mismatches")


if __name__ == "__main__":
    main()
