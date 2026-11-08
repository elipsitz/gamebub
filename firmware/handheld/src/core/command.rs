#![allow(unused)]

use std::time::{Duration, Instant};

use crate::device::{drivers::fpga, Device};

use super::{CoreError, CoreManager, CORE_POLL_INTERVAL};

// Commands
pub const GET_STATUS: u32 = 0x0000;
pub const CORE_RUN: u32 = 0x0100;
pub const CORE_HALT: u32 = 0x0101;
pub const SETUP_COMPLETE: u32 = 0x0102;
pub const NOTIFY_FOCUS: u32 = 0x0200;
pub const FILE_WRITE_START: u32 = 0x0300;
pub const FILE_WRITE_END: u32 = 0x0301;
pub const FILE_READ_START: u32 = 0x0302;
pub const FILE_READ_END: u32 = 0x0303;

// Status
#[allow(unused)]
pub const STATUS_UNKNOWN: u32 = 0;
#[allow(unused)]
pub const STATUS_INITIALIZE: u32 = 1;
pub const STATUS_SETUP: u32 = 2;
pub const STATUS_CORE_HALT: u32 = 3;
#[allow(unused)]
pub const STATUS_CORE_RUN: u32 = 4;

impl CoreManager {
    pub(super) fn poll_core_status(&mut self, expected: u32, timeout: Duration) -> Result<(), CoreError> {
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
}
