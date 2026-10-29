use std::{io::Write, time::Duration};

use esp_idf_svc::hal::units::Hertz;

use crate::bitstream::boot::{FIFO_BASE, REG_BASE};
use crate::device::{
    drivers::{fpga, usb::CdcStream},
    Device,
};

const REG_CPU_RESET_N: u32 = REG_BASE | 0x0;
const REG_TX_COUNT: u32 = REG_BASE | 0x100;
const REG_TX_READ_LIMIT: u32 = REG_BASE | 0x104;
const REG_RX_COUNT: u32 = REG_BASE | 0x110;
const REG_CART_EN: u32 = REG_BASE | 0x120;

const TX_SIZE: u32 = 4096;
const RX_SIZE: u32 = 4096;
const MAX_CLOCK: Hertz = Hertz(10_000_000);

pub fn start_task(cdc_interface: u32) {
    std::thread::Builder::new()
        .name("cart_backup".to_string())
        .stack_size(4 * 1024)
        .spawn(move || {
            let stream = CdcStream::new(cdc_interface);
            task(stream);
        })
        .unwrap();
}

fn load_firmware() {
    let file = crate::util::open_system_file("fw_cart_backup.bin").expect("open firmware");
    let mut device = Device::lock();
    crate::bitstream::boot::load_cpu_memory(&mut device, 0, file).expect("load firmware");

    // Take CPU out of reset.
    device.fpga.write_u32(REG_CPU_RESET_N, 1).unwrap();
}

fn task(mut stream: CdcStream) {
    log::info!("Starting cartridge utility");
    load_firmware();
    log::info!("Firmware loaded");

    let mut buf = vec![0u8; TX_SIZE as usize];
    let mut cart_enabled = false;
    loop {
        let mut device = Device::lock();

        // Try moving from device -> usb
        let tx_ready = device.fpga.read_u32(REG_TX_COUNT).unwrap();
        if tx_ready > 0 {
            // Read the data.
            let count = tx_ready.min(buf.len() as u32);
            let buf = &mut buf[0..count as usize];
            device.fpga.write_u32(REG_TX_READ_LIMIT, count).unwrap();
            let command = fpga::SpiCommand::new(fpga::FpgaSpiWordSize::Bits8);
            device
                .fpga
                .spi_read(Some(MAX_CLOCK), command, FIFO_BASE, buf)
                .unwrap();

            // Write it to CDC.
            let _ = stream.write_all(buf);
            let _ = stream.flush();
        }

        // Try moving from usb -> device
        let rx_ready = RX_SIZE - device.fpga.read_u32(REG_RX_COUNT).unwrap();
        if rx_ready > 0 {
            let count = rx_ready.min(buf.len() as u32);
            let buf = &mut buf[0..count as usize];
            if let Ok(read) = stream.try_read(buf) {
                if read > 0 {
                    let data = &buf[..read];

                    let command = fpga::SpiCommand::new(fpga::FpgaSpiWordSize::Bits8);
                    device
                        .fpga
                        .spi_write(Some(MAX_CLOCK), command, FIFO_BASE, data)
                        .unwrap();
                }
            }
        }

        let new_cart_enable = device.fpga.read_u32(REG_CART_EN).unwrap() != 0;
        if cart_enabled != new_cart_enable {
            device.set_cart_power(new_cart_enable);
            cart_enabled = new_cart_enable;
        }
        drop(device);

        // TODO: more efficient way of moving bytes that isn't a busy sleep loop.
        std::thread::sleep(Duration::from_millis(1));
    }
}
