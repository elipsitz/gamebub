#![allow(unused)]

use std::time::{Duration, Instant};

use crate::device::{drivers::fpga, Device};

use super::{CoreError, CoreManager, CORE_POLL_INTERVAL};

// Host -> Core commands
pub const GET_STATUS: u32 = 0x0000;
pub const CORE_RUN: u32 = 0x0100;
pub const CORE_HALT: u32 = 0x0101;
pub const SETUP_COMPLETE: u32 = 0x0102;
pub const NOTIFY_FOCUS: u32 = 0x0200;
pub const FILE_WRITE_START: u32 = 0x0300;
pub const FILE_WRITE_END: u32 = 0x0301;
pub const FILE_READ_START: u32 = 0x0302;
pub const FILE_READ_END: u32 = 0x0303;

// Core -> Host commands
pub const FILE_GET_SIZE: u32 = 0x0400;
pub const FILE_SET_SIZE: u32 = 0x0401;
pub const FILE_READ: u32 = 0x0402;
pub const FILE_WRITE: u32 = 0x0403;
pub const FILE_FLUSH: u32 = 0x0404;

// Status
#[allow(unused)]
pub const STATUS_UNKNOWN: u32 = 0;
#[allow(unused)]
pub const STATUS_INITIALIZE: u32 = 1;
pub const STATUS_SETUP: u32 = 2;
pub const STATUS_CORE_HALT: u32 = 3;
#[allow(unused)]
pub const STATUS_CORE_RUN: u32 = 4;

// File status codes
pub const FILE_STATUS_OK: u32 = 0;
pub const FILE_STATUS_ERR_INVALID_FILE: u32 = 1;
pub const FILE_STATUS_ERR_IO: u32 = 2;
pub const FILE_STATUS_ERR_BOUNDS: u32 = 3;

impl CoreManager {
    pub(super) fn poll_core_status(
        &mut self,
        expected: u32,
        timeout: Duration,
    ) -> Result<(), CoreError> {
        let start = Instant::now();
        loop {
            if start.elapsed() > timeout {
                return Err(CoreError::CoreReadyTimeout);
            }
            let mut status = [0u32];
            self.run_core_command(&[GET_STATUS], &mut status, timeout)
                .map_err(|_| CoreError::CommandFailed(GET_STATUS))?;

            if status[0] == expected {
                return Ok(());
            }
            std::thread::sleep(CORE_POLL_INTERVAL);
        }
    }

    pub(super) fn run_core_command(
        &self,
        request: &[u32],
        response: &mut [u32],
        timeout: Duration,
    ) -> Result<(), CoreError> {
        assert!(!request.is_empty());
        // Write command and arguments
        {
            let mut device = Device::lock();
            for (i, &x) in request.iter().enumerate() {
                let _ = device
                    .fpga
                    .write_u32(fpga::REG_CMD_HOST_BASE + (4 * i as u32), x);
            }
            let _ = device.fpga.write_u32(fpga::REG_CTRL_CMD_HOST, 0b1000);
        }

        // Poll for command completion.
        let start = Instant::now();
        let status = loop {
            if start.elapsed() > timeout {
                log::error!("Command {:08X} timed out", request[0]);
                return Err(CoreError::CommandTimeout(request[0]));
            }
            let mut device = Device::lock();
            let status = device.fpga.read_u32(fpga::REG_CTRL_CMD_HOST).unwrap();
            if (status & 0b0011) != 0 {
                break status;
            }
            drop(device);
            std::thread::sleep(CORE_POLL_INTERVAL);
        };

        // Read result values and end command
        {
            let mut device = Device::lock();
            for (i, x) in response.iter_mut().enumerate() {
                *x = device
                    .fpga
                    .read_u32(fpga::REG_CMD_HOST_BASE + (4 * i as u32))
                    .unwrap();
            }

            let _ = device.fpga.write_u32(fpga::REG_CTRL_CMD_HOST, 0);
        }

        // Check status.
        if (status & 0b0001) == 0 {
            Ok(())
        } else {
            Err(CoreError::CommandFailed(request[0]))
        }
    }

    pub fn handle_core_request(&mut self) {
        let mut device = Device::lock();

        // Check that there's a request pending.
        let status = device.fpga.read_u32(fpga::REG_CTRL_CMD_CORE).unwrap();
        if (status & 0b1000) == 0 {
            return;
        }

        // Read command ID and up to 5 args.
        let command = {
            let mut words = [0u32; 6];
            let buf = unsafe {
                std::slice::from_raw_parts_mut(
                    words.as_mut_ptr() as *mut u8,
                    std::mem::size_of_val(&words),
                )
            };
            let _ = device.fpga.spi_read(
                Some(fpga::MAX_SPI_READ_CLOCK),
                fpga::SpiCommand::new(fpga::FpgaSpiWordSize::Bits32),
                fpga::REG_CMD_CORE_BASE,
                buf,
            );
            words
        };
        let command_id = command[0];
        let params = &command[1..];

        let func = match command_id {
            FILE_GET_SIZE => Self::handle_file_get_size,
            FILE_SET_SIZE => Self::handle_file_set_size,
            FILE_READ => Self::handle_file_read,
            FILE_WRITE => Self::handle_file_write,
            FILE_FLUSH => Self::handle_file_flush,
            _ => {
                log::warn!("Unknown core request 0x{:04X}", command_id);
                // Mark command as "done" and "error".
                let _ = device.fpga.write_u32(fpga::REG_CTRL_CMD_CORE, 0b1011);
                return;
            }
        };

        // Mark command as "busy".
        let _ = device.fpga.write_u32(fpga::REG_CTRL_CMD_CORE, 0b1100);

        // Run the command.
        drop(device);
        let mut results = [0u32; 3];
        let success = func(self, params, &mut results);

        // Write back any results.
        let mut device = Device::lock();
        {
            let buf = unsafe {
                std::slice::from_raw_parts(
                    results.as_ptr() as *const u8,
                    std::mem::size_of_val(&results),
                )
            };
            // TODO: pick better max_clock rate: base off of system clock?
            let _ = device.fpga.spi_write(
                Some(fpga::MAX_SPI_READ_CLOCK),
                fpga::SpiCommand::new(fpga::FpgaSpiWordSize::Bits32),
                fpga::REG_CMD_CORE_BASE,
                buf,
            );
        }

        // Mark command as complete.
        let status = if success { 0b1010 } else { 0b1011 };
        let _ = device.fpga.write_u32(fpga::REG_CTRL_CMD_CORE, status);
    }
}
