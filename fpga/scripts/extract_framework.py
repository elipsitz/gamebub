import argparse
from pathlib import Path
import os
import sys
import shutil

ROOT_PATH = Path(__file__).resolve().parent.parent

FILES = [
    # Info
    ("framework/README.md", "README.md"),
    ("framework/LICENSE", "LICENSE"),
    ("framework/requirements.txt", "requirements.txt"),
    # Build files
    ("mill", None),
    ("framework/package.mill", "package.mill"),
    ("scripts/build_core.py", None),
    # Framework
    ("src/main/scala/net/gamebub/framework/Core.scala", None),
    ("src/main/scala/net/gamebub/framework/ExtModuleUtils.scala", None),
    *[
        (f"src/main/scala/net/gamebub/framework/interface/{interface}.scala", None)
        for interface in [
            "AudioV0",
            "CartridgePortV0",
            "ClocksV0",
            "HostV0",
            "InputV0",
            "LinkPortV0",
            "PmodV0",
            "SdramV0",
            "SramV0",
            "VibrateV0",
            "VideoFilterBasicV0",
            "VideoV0",
        ]
    ],
    # Game Bub Handheld platform
    ("src/main/scala/platform/handheld/HandheldTop.scala", None),
    ("src/main/scala/platform/handheld/display/DisplayDriverIO.scala", None),
    ("src/main/scala/platform/handheld/display/DpiDriver.scala", None),
    ("src/main/scala/platform/handheld/display/ILI9488.scala", None),
    ("src/main/scala/platform/handheld/display/ILI9806E.scala", None),
    ("src/main/scala/platform/handheld/display/ST7262E43.scala", None),
    ("src/main/scala/platform/handheld/spi/SpiReceiver.scala", None),
    ("src/main/scala/platform/handheld/spi/SpiReceiverFifo.scala", None),
    ("src/main/scala/lib/mem/MemoryInterface.scala", None),
    ("src/main/scala/lib/mem/MemoryMap.scala", None),
    ("src/main/scala/lib/mem/RegisterMap.scala", None),
    ("src/main/scala/lib/util/ButtonFilter.scala", None),
    ("src/main/scala/lib/util/FractionalDivider.scala", None),
    ("src/main/scala/lib/util/ResetSynchronizer.scala", None),
    ("src/main/scala/lib/video/Color.scala", None),
    ("src/main/scala/lib/video/ColorARGB.scala", None),
    ("src/main/scala/lib/video/ColorRGB.scala", None),
    ("src/main/scala/xilinx/XpmCdcHandshake.scala", None),
    ("src/main/scala/xilinx/XpmCdcSingle.scala", None),
    ("src/main/scala/xilinx/XpmCdcSyncRst.scala", None),
    ("src/main/scala/xilinx/XpmFifoAsync.scala", None),
    # Verilog / XDC
    ("verilog/handheld/top.sv", None),
    ("verilog/handheld/clk_wiz_hdmi.v", None),
    ("verilog/handheld/pll_reset_generator.sv", None),
    ("verilog/handheld/common.xdc", None),
    ("verilog/handheld/rev_1.xdc", None),
    ("verilog/handheld/rev_2.xdc", None),
    ("verilog/handheld/rev_3.xdc", None),
    ("verilog/handheld/rev_4.xdc", None),
    # Third-party Verilog
    ("verilog/picorv32.v", None),
    ("verilog/hdmi/README.md", None),
    ("verilog/hdmi/LICENSE-APACHE", None),
    ("verilog/hdmi/LICENSE-MIT", None),
    ("verilog/hdmi/src/hdmi.sv", None),
    ("verilog/hdmi/src/tmds_channel.sv", None),
    ("verilog/hdmi/src/packet_assembler.sv", None),
    ("verilog/hdmi/src/packet_picker.sv", None),
    ("verilog/hdmi/src/serializer.sv", None),
    ("verilog/hdmi/src/auxiliary_video_information_info_frame.sv", None),
    ("verilog/hdmi/src/source_product_description_info_frame.sv", None),
    ("verilog/hdmi/src/audio_clock_regeneration_packet.sv", None),
    ("verilog/hdmi/src/audio_info_frame.sv", None),
    ("verilog/hdmi/src/audio_sample_packet.sv", None),
]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out-dir", required=True, type=Path)
    args = parser.parse_args()

    if any(x for x in os.listdir(args.out_dir) if not x.startswith(".")):
        sys.exit("Output directory is not empty")

    files = []
    for src_file, dst_file in FILES:
        src_path = ROOT_PATH / src_file
        dst_path = args.out_dir / (dst_file or src_file)
        files.append((src_path, dst_path))

    for src_path, dst_path in files:
        dst_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(src_path, dst_path)


if __name__ == "__main__":
    main()
