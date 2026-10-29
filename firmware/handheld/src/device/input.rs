use super::Device;
use crate::{device::drivers::fpga, input::InputState};

impl Device<'_> {
    /// Get the current state of the internal buttons from the FPGA.
    #[allow(clippy::field_reassign_with_default)]
    pub fn get_input_state(&mut self) -> Result<InputState, ()> {
        let fpga_buttons = self
            .fpga
            .read_u32(fpga::REG_STATUS_BUTTON)
            .map_err(|_| ())?;
        let mut state = InputState::default();
        state.btn_a = (fpga_buttons & (1 << 11)) != 0;
        state.btn_b = (fpga_buttons & (1 << 10)) != 0;
        state.btn_x = (fpga_buttons & (1 << 9)) != 0;
        state.btn_y = (fpga_buttons & (1 << 8)) != 0;
        state.btn_up = (fpga_buttons & (1 << 7)) != 0;
        state.btn_down = (fpga_buttons & (1 << 6)) != 0;
        state.btn_left = (fpga_buttons & (1 << 5)) != 0;
        state.btn_right = (fpga_buttons & (1 << 4)) != 0;
        state.btn_start = (fpga_buttons & (1 << 1)) != 0;
        state.btn_select = (fpga_buttons & (1 << 0)) != 0;
        state.btn_l1 = (fpga_buttons & (1 << 3)) != 0;
        state.btn_r1 = (fpga_buttons & (1 << 2)) != 0;
        state.btn_system = self.button_home.is_low();
        state.btn_vol_up = self.button_vol_up.is_low();
        state.btn_vol_down = self.button_vol_down.is_low();
        state.btn_power = self.button_power.is_low();
        Ok(state)
    }

    /// Get whether an HDMI cable is plugged in based on IO expander state
    #[cfg(feature = "rev1")]
    pub(super) fn parse_hdmi_detect(&mut self, io_expander: [bool; 16]) -> Result<bool, ()> {
        // Rev 1: HDMI hot plug detect is active-low.
        Ok(!io_expander[5])
    }

    #[cfg(feature = "rev1")]
    pub fn read_hdmi_detect(&mut self) -> Result<bool, ()> {
        let io_expander = self.io_expander.get_pins().map_err(|_| ())?;
        self.parse_hdmi_detect(io_expander)
    }
}
