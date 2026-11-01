# Building Game Bub firmware and FPGA cores

How the firmware and FPGA cores are built, tested, and packaged into a
`.uf2` update file for a Rev 4 handheld.

## Toolchains

| Part | Host | Tools |
|---|---|---|
| ESP32-S3 firmware | macOS or Linux | `rustup`, `espup` (Xtensa Rust toolchain), `ldproxy`, `espflash` |
| FPGA (Chisel → Verilog) | Linux (x86-64) | OpenJDK 21, `./mill` (bundled launcher) |
| FPGA simulation | Linux | Verilator 5, SDL2, Python 3 with `edalize numpy pillow` |
| FPGA bitstreams | Linux (x86-64) | AMD Vivado 2023.2 (free ML Standard edition, Artix-7 device support only) |
| UF2 packaging | Python ≥ 3.12 | `construct`, ESP-IDF's `components/fatfs` scripts |

Ubuntu packages used: `openjdk-21-jdk-headless verilator libsdl2-dev flex bison
python3-venv rgbds`. The Python packages live in a venv at `fpga/.venv`:

```sh
cd fpga
python3 -m venv .venv
.venv/bin/pip install edalize heatshrink2 numpy pillow construct
```

Vivado can be installed without the GUI: download the 2023.2 Linux web installer
from AMD, then:

```sh
./FPGAs_AdaptiveSoCs_Unified_2023.2_*_Lin64.bin --keep --noexec --target /tmp/vivado-installer
/tmp/vivado-installer/xsetup -b AuthTokenGen          # asks for your AMD account
/tmp/vivado-installer/xsetup -b ConfigGen             # choose Vivado, ML Standard
# edit ~/.Xilinx/install_config.txt: Modules=...Artix-7:1 (all others :0)
/tmp/vivado-installer/xsetup -b Install -a XilinxEULA,3rdPartyEULA -c ~/.Xilinx/install_config.txt
```

On newer distributions (e.g. Ubuntu 24.04+) Vivado 2023.2 fails to start with
`libtinfo.so.5: cannot open shared object file`. Point it at the current
library from inside the Vivado install:

```sh
L=/tools/Xilinx/Vivado/2023.2/lib/lnx64.o
ln -s /usr/lib/x86_64-linux-gnu/libtinfo.so.6 $L/libtinfo.so.5
ln -s /usr/lib/x86_64-linux-gnu/libncurses.so.6 $L/libncurses.so.5
```

## Firmware

```sh
cd firmware/handheld
. ~/export-esp.sh
cargo build --release --features rev4
```

The first build downloads ESP-IDF v5.4.3 into `~/.espressif` (several minutes).

## FPGA

Run the unit tests (some upstream tests are known to fail; see below):

```sh
cd fpga
./mill -i GameBub.test
```

Build the three Rev 4 bitstreams (each takes roughly 10–20 minutes):

```sh
source /tools/Xilinx/Vivado/2023.2/settings64.sh
for core in boot:net.gamebub.core.boot.HandheldBoot \
            gameboy:net.gamebub.core.gameboy.HandheldGameboy \
            gba:net.gamebub.core.gba.HandheldGba; do
  .venv/bin/python scripts/build_core.py --name ${core%%:*} --target gamebub_rev4 \
      --build-root build/${core%%:*} --core-class ${core#*:}
done
```

Each build produces `build/<name>/<name>.bit.hs` (the heatshrink-compressed
bitstream the firmware loads).

### Simulation

```sh
.venv/bin/python scripts/build_sim.py gameboy --build-root build/sim-gb
.venv/bin/python scripts/build_sim.py gba --build-root build/sim-gba

# Interactive (needs a display):
build/sim-gba/VSimGba --bios-path gba.bios.bin game.gba
```

## Packaging a UF2 update

The system data partition holds the bitstreams and boot ROMs. Start from the
files in an official release (extract its `system_data` partition with ESP-IDF's
`fatfsparse.py`), replace the `.bit.hs` files with newly built ones, then:

```sh
cd firmware/handheld
export IDF_PATH=/path/to/esp-idf
python3 generate_firmware_uf2.py \
    --firmware target/xtensa-esp32s3-espidf/release/handheld \
    --system-data path/to/system_data_dir \
    --hw-revision 4 \
    --output gamebub-rev4.uf2
```

Install it the same way as an official update (see the Game Bub user guide).

## Known test status

- `gba.cpu.ARM7TDMISpec`: 13 of 14 tests fail on upstream code too (stale
  expectations for the CPU's bus signals).
- `gameboy.cpu.CpuSpec` and `platform.handheld.spi.SpiReceiverSpec` abort:
  they use `chiseltest`, which doesn't work with Chisel 7.
