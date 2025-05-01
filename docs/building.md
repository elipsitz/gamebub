# Building

Building a Game Bub unit is a fairly involved process:

1. Manufacturing the PCB
2. Assembling the PCB
3. Printing the shell (and buttons)
4. Assembling the whole device
5. Building the FPGA bitstreams
6. Setting up the microSD card
7. Flashing the MCU firmware

## 1. Manufacturing the PCB

Fab the PCB (in `/pcb/handheld_rev2`) using your manufacturer of choice. The board was initially designed to be fabricated and assembled by JLCPCB.

The board has 6 layers, and rectangular dimensions of 90.1 mm by 135 mm. Due to the fine pitch (0.8mm) BGA FPGA package, the smallest vias are 0.4/0.25mm, with a minimum track width of 0.1mm. The board must have an ENIG finish, as HASL is too uneven for fine-pitch BGA components. 

The board should have a thickness of 1.6mm -- this isn't essential, but the enclosure was designed for this thickness. Only the JLCPCB default 6-layer `JLC06161H-3313` stackup has been tested, but other stackups are likely to work too.

## 2. Assembling the PCB

The board is primarily one-sided, and the vast majority of the components can be machine assembled. The board design files have LCSC component IDs (for easy JLCPCB assembly).

The remaining components are specialty or otherwise difficult to machine assemble:

Front:
* 3.5mm headphone jack: Same Sky SJ-43614-SMT-TR
* Rumble motor: generic 10mm diameter rumble motor
* Optional PMOD connector: 6-pin female 0.1 inch right angle SMD header

Rear:
* Generic GBA cartridge slot
* Generic GBA link port
* Cartridge slot shape detector: JJOV0UL650NONPRBK
* Battery connector: S2B-PH-SM4-TB (or generic 2-pin SMD JST-PH receptacle)
* Shoulder (L and R) buttons: C&K PTS645VN13SMTR92LFS (x2)

Solder each of these components (see the PCB design files if needed).

## 3. Printing the shell

The shell consists of two pieces: the front and rear. Both have a wall thickness of 1.5mm, with some small features of 1.0mm or 0.8mm.

FDM (filament) printing is almost certainly not suitable: this should be printed with a high-precision technology such as SLA (photosensitive resin).

Additionally, all of the buttons are custom and need to be printed as well:
* 4x Large front buttons (A, B, X, and Y)
* D-pad
* 3x Small front buttons (Start, Select, Home)
* 3x Side buttons

## 4. Assembling the whole device

### Parts needed
* ER-TFT035IPS-6 LCD module
* LCD Cover glass (tempered glass or transparent acrylic)
* 2x Speakers (CMS-160903-18S-X8)
* 9x M2.5x4mm heat-set inserts
* CA glue to glue the inserts
* A set of Nintendo DSi button membranes (ABXY and D-pad)
* M2.5 screws:
    * 5x 5mm (three PCB screws, 2 top screws)
    * 4x 14mm (middle and bottom screws)
* A set of torsion springs (1x left, 1x right):
    * Required: >1.5mm interior diameter
    * Suggested: 0.3mm wire diameter, 5 turns, 135 degree angle
* 2x 1.5mm diameter dowel pins, 8mm to 9mm long.
* 755068 size flat Lipo battery with a JST-PH connector (verify polarity)

### Steps
Before assembling, consider testing that the PCB works properly.

1. Attach the LCD cover glass to the front shell with adhesive glue or tape
2. Place the front shell face down on a surface
3. Carefully align and press the LCD module onto the cover glass (ensure flex is facing the correct direction)
4. Glue all 9 heat-set inserts into place, and let the glue harden
5. Insert the speakers into the shell
6. Place the face buttons into the shell, followed by the D-pad and ABXY membranes
7. Open the LCD flex connector on the PCB
8. Carefully set the PCB face down onto the shell, putting the LCD module flex into place on the PCB connector, and close it.
9. Ensure the PCB is aligned well onto the front shell
8. Use the 5mm screws to attach the PCB to the front shell (triangle of screw holes in the middle, towards the bottom half of the PCB). Tighten carefully.
9. Insert the three side buttons into the front shell.
10. Attach the battery to the rear shell with adhesive
11. Assemble the shoulder buttons: align each shoulder button with the rear shell. Place the correct spring in line (inserting one end of the spring into the button), and then insert a dowel pin to hold the assembly together. Hook the other end of the spring into the hook in the rear shell.
12. Plug the battery into the battery connector on the PCB
13. Place the rear shell onto the front shell
14. Use 6 screws (5mm on top, 14mm on the middle and bottom) to close up the shell

## 5. Building the FPGA bitstreams

1. Install Xilinx Vivado 2023.2 with support for Artix-7 devices
2. Install JDK 8 or later, and [Scala Build Tool (`sbt`)](https://www.scala-sbt.org/download.html)
3. Install [FuseSoC](https://github.com/olofk/fusesoc).

From the `/fpga` directory, use FuseSoC to build the bitstreams:

```sh
$ fusesoc --cores-root . run --work-root=build/boot --target=handheld_rev2 --flag=module_boot elipsitz:gameboy:gameboy
$ fusesoc --cores-root . run --work-root=build/gameboy --target=handheld_rev2 --flag=module_gameboy elipsitz:gameboy:gameboy
$ fusesoc --cores-root . run --work-root=build/gba --target=handheld_rev2 --flag=module_gba elipsitz:gameboy:gameboy
```

Save the generated `*.bit` files.

## 6. Setting up the microSD card

Format a good quality microSD card with FAT32, then create the following directory structure:

```
system/
  boot.bit.gz
  gameboy.bit.gz
  gameboy.bios-dmg.bin
  gameboy.bios-cgb.bin
  gba.bit.gz
  gba.bios.bin
roms/
```

* `boot.bit.gz` should be a gzip'd copy of the `boot` bitstream built previously.
* `gameboy.bit.gz` should be a gzip'd copy of the `gameboy` bitstream.
* `gba.bit.gz` should be a gzip'd copy of the `gba` bitstream.
* `gameboy.bios-dmg.bin` and `gameboy.bios-cgb.bin` should be the bootrom files for the original Game Boy and Game Boy Color, or open-source alternatives (e.g. from [SameBoy](https://github.com/LIJI32/SameBoy)).
* `gba.bios.bin` should be the Game Boy Advance bootrom. Either the official one, extracted from a GBA (best compatibility), or a free alternative ([e.g. this one](https://github.com/Cult-of-GBA/BIOS)).
* `roms/` should be a directory containing ROM files (if desired), with `.gb`, `.gbc`, and `.gba` extensions. This directory can be further organized into more directories.

## 7. Flashing the MCU firmware

### Setup

1. Install Rust for ESP32 by [following these instructions (Xtensa targets)](https://docs.esp-rs.org/book/installation/index.html)
2. Install [espflash](https://github.com/esp-rs/espflash)

### Building and installing

Connect the device to your computer with a USB-C cable. Hold the "Home" button (in the center) while turning the device on (by pressing the power button in the top right).

From `/firmware/handheld`, run the following command to build and flash the firmware:

```sh
$ cargo run --release --features=rev2
```

Then, flash the device-specific "factory" data:

```sh
$ python3 flash_nvs.py --serial serialno --revision 2
```

The device should automatically reboot into the main menu.
