use std::ffi::OsStr;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use crate::bitstream::util::scratch_buffer::ScratchBuffer;
use crate::device::DisplayMode;
use crate::device::{drivers::fpga, Device};
use crate::led;
use crate::ui;

pub mod boot;
pub mod gameboy;
pub mod gba;

mod util;

pub static SCRATCH: ScratchBuffer<{ 16 * 1024 }> = ScratchBuffer::new();

/// Driver for a specific bitstream.
pub trait Bitstream {
    /// Called when a vblank IRQ occurs.
    fn on_vblank_irq(&mut self);
}

pub fn program_fpga(path: &Path) {
    log::info!("Loading bitstream {}", path.display());
    led::LedController::set_behavior(led::LedBehavior::LOADING);
    let mut device = Device::lock();
    let display_mode = device.get_display_mode();

    if let DisplayMode::Internal = display_mode {
        // Avoid LCD artifacts during FPGA reprogram.
        device.set_lcd_enabled(false);
        // For some reason, we need to sleep for a short amount of time here
        // (before doing FPGA program), otherwise the LCD won't properly sleep.
        // 2 ms is sometimes sufficient, 5 ms is always sufficient, 10 ms seems to always work.
        std::thread::sleep(Duration::from_millis(10));
    }

    let file = File::open(path).unwrap();
    let mut bitstream: Box<dyn std::io::Read> = if path.extension() == Some(OsStr::new("hs")) {
        Box::new(heatshrink_decompress_stream(file))
    } else if path.extension() == Some(OsStr::new("bit")) {
        Box::new(BufReader::with_capacity(512, file))
    } else {
        panic!("Unsupported bitstream extension");
    };

    device
        .fpga
        .program(&mut bitstream, &mut SCRATCH.take().unwrap())
        .unwrap();
    device.fpga.set_display_mode(display_mode).unwrap();
    device.fpga.enable_interrupt(fpga::Irq::Button).unwrap();
    ui::send(ui::Message::InputState(device.get_input_state().unwrap()));
    ui::send(ui::Message::Redraw);
    led::LedController::set_behavior(led::LedBehavior::OFF);

    if let DisplayMode::Internal = display_mode {
        device.set_lcd_enabled(true);
    }
}

pub fn initial_program_boot(device: &mut Device) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let file = crate::util::open_system_file("boot.bit.hs").context("Failed to read bitstream")?;
    let mut bitstream = heatshrink_decompress_stream(file);

    device
        .fpga
        .program(&mut bitstream, &mut SCRATCH.take().unwrap())
        .context("Failed to program FPGA")
}

fn heatshrink_decompress_stream(file: File) -> impl Read {
    // Heatshrink decoder parameters: W=9, L=6 (chosen empirically)
    type HeatshrinkDecoder = heatshrink::decoder::HeatshrinkDecoder<9, 6, 512, 512>;
    let reader = embedded_io_adapters::std::FromStd::new(file);
    let decoder = heatshrink::io::DecoderReader::<_, HeatshrinkDecoder>::new(reader);
    embedded_io_adapters::std::ToStd::new(decoder)
}

pub fn program_boot() {
    program_fpga(&crate::util::get_system_file_path("boot.bit.hs"));
}

/// Scale factor applied to accelerometer readings for tilt games.
pub fn tilt_sensitivity() -> f32 {
    match crate::kvs::keys::TILT_SENSITIVITY.get().unwrap_or(2) {
        0 => 0.25,
        1 => 0.5,
        3 => 1.5,
        _ => 1.0,
    }
}
