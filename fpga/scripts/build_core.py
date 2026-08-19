import argparse
import os
import subprocess
from dataclasses import dataclass
from pathlib import Path
from edalize.edatool import get_edatool
import os.path
import struct
import sys
import shutil

FRAMEWORK_ROOT = Path(__file__).resolve().parent.parent
MILL_PATH = FRAMEWORK_ROOT / "mill"

@dataclass
class Target:
    name: str
    main_class: str
    chisel_args: list[str]
    verilog_defines: list[str]
    files: list[str]
    top_module: str
    part: str


# Generate Target list
TARGETS = []
for r in [1, 2, 3, 4]:
    TARGETS.append(
        Target(
            name=f"gamebub_rev{r}",
            main_class="platform.handheld.HandheldTop",
            chisel_args=[str(r)],
            verilog_defines=[f"BOARD_REV_{r}"],
            files=[
                f"verilog/handheld/rev_{r}.xdc",
                # Common
                "verilog/handheld/common.xdc",
                "verilog/handheld/top.sv",
                "verilog/handheld/pll_reset_generator.sv",
                "verilog/handheld/clk_wiz_hdmi.v",
                "verilog/picorv32.v",
                # HDMI
                "verilog/hdmi/src/audio_clock_regeneration_packet.sv",
                "verilog/hdmi/src/audio_info_frame.sv",
                "verilog/hdmi/src/audio_sample_packet.sv",
                "verilog/hdmi/src/auxiliary_video_information_info_frame.sv",
                "verilog/hdmi/src/hdmi.sv",
                "verilog/hdmi/src/packet_assembler.sv",
                "verilog/hdmi/src/packet_picker.sv",
                "verilog/hdmi/src/serializer.sv",
                "verilog/hdmi/src/source_product_description_info_frame.sv",
                "verilog/hdmi/src/tmds_channel.sv",
            ],
            top_module="top_handheld",
            part="xc7a100tcsg324-1",
        )
    )


def get_git_revision() -> str:
    try:
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True)
        revision = revision.strip()
    except:
        print("Failed to find Git revision")
        return ""

    return revision[:12]


def insert_bitstream_metadata(bitstream: bytearray, extra: str):
    # Very crude way of not parsing the Xilinx header
    # Assumes the first tag is "a" (design name)
    HEADER = b"\x00\x09\x0f\xf0\x0f\xf0\x0f\xf0\x0f\xf0\x00\x00\x01a"
    if bitstream[: len(HEADER)] != HEADER:
        raise SystemExit("Unexpected bitstream header")

    # Find the length of the tag
    index = len(HEADER)
    (length,) = struct.unpack(">H", bitstream[index : (index + 2)])
    if length < 1:
        return
    index += 2
    # Extract the tag
    tag = bitstream[index : (index + length)]
    # Modify the tag
    tag = tag[:-1] + extra.encode("utf-8") + bytearray([0])
    bitstream[index : (index + length)] = tag
    # Modify the length
    length = struct.pack(">H", len(tag))
    index = len(HEADER)
    bitstream[index : (index + 2)] = length


def build(
    name: str,
    target: Target,
    build_root: Path,
    core_class: str,
    project_root: Path,
    additional_file_dirs: list[Path] = [],
):
    files = []
    if build_root.exists():
        shutil.rmtree(build_root)
    build_root.mkdir(parents=True, exist_ok=True)

    # Run Chisel
    generate_root = build_root / "generated"
    args = [
        MILL_PATH,
        "-i",
        "--no-build-lock",
        "root.runMain",
        target.main_class,
        core_class,
        *target.chisel_args,
        f"--target-dir={generate_root}",
    ]

    result = subprocess.run(args, cwd=project_root)
    if result.returncode != 0:
        sys.exit("Chisel build failed")

    # Collect files from Chisel output
    filelist_path = generate_root / "filelist.f"
    with open(filelist_path) as f:
        for filename in f:
            path = filelist_path.parent / filename.rstrip()
            files.append(dict(name=path, file_type="systemVerilogSource"))

    # Add additional target RTL files
    additional_files = [FRAMEWORK_ROOT / f for f in target.files]
    for dir in additional_file_dirs:
        for (dir, _, dir_files) in os.walk(dir):
            dir = Path(dir)
            for file in dir_files:
                additional_files.append(dir / file)

    for file in additional_files:
        file_extension = file.suffix
        file_type = {
            ".xdc": "xdc",
            ".v": "verilogSource",
            ".sv": "systemVerilogSource",
            ".vhdl": "vhdlSource",
        }.get(file_extension)
        if not file_type:
            print("WARNING: Skipping unknown file type:", file)
        files.append(dict(name=file, file_type=file_type))

    # Make all file paths relative to build root.
    for f in files:
        f["name"] = os.path.relpath(str(f["name"]), str(build_root))

    # Set Verilog defines
    parameters = {
        name: {"datatype": "bool", "paramtype": "vlogdefine", "default": True}
        for name in target.verilog_defines
    }

    # Run Vivado (via Edalize)
    edam = {
        "name": name,
        "toplevel": target.top_module,
        "files": files,
        "parameters": parameters,
        "tool_options": {"vivado": {"part": target.part}},
    }
    backend = get_edatool("vivado")(edam=edam, work_root=build_root)
    backend.configure()
    backend.build()

    # After a successful build, the bitstream will be at <name>.bit.
    bitstream_path = build_root / f"{name}.bit"
    if not bitstream_path.exists():
        sys.exit("Bitstream path doesn't exist")
    print("Found bitstream at", bitstream_path)
    bitstream = bytearray(open(bitstream_path, "rb").read())

    # Determine Git Revision
    git_revision = get_git_revision()
    if git_revision:
        print("Git revision:", git_revision)

    # Insert Git information into bitstream
    insert_bitstream_metadata(bitstream, ";GitRev=" + git_revision)
    open(bitstream_path, "wb").write(bitstream)

    # Compress bitstream (heatshrink)
    try:
        import heatshrink2
        compressed_path = bitstream_path.with_name(bitstream_path.name + ".hs")
        compressed = heatshrink2.encode(bitstream, window_sz2=9, lookahead_sz2=6)
        with open(compressed_path, "wb") as f:
            f.write(compressed)
    except ImportError:
        # Skip heatshrink
        pass

    print("*" * 80)
    print("Bitstream:")
    print(bitstream_path.resolve())
    print("*" * 80)


def main(args) -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--name", required=True)
    parser.add_argument(
        "--target",
        choices=[t.name for t in TARGETS],
        required=True,
        help="Target hardware",
    )
    parser.add_argument("--build-root", required=True, type=Path)
    parser.add_argument("--project-root", type=Path)
    parser.add_argument("--core-class", required=True)
    parser.add_argument("--additional-file-dirs", nargs="*", default=[], type=Path)

    args = parser.parse_args(args)
    target = next(t for t in TARGETS if t.name == args.target)
    build(
        args.name,
        target,
        args.build_root.resolve(),
        args.core_class,
        args.project_root or FRAMEWORK_ROOT,
        args.additional_file_dirs,
    )


if __name__ == "__main__":
    main(sys.argv[1:])
