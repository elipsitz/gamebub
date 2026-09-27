use std::sync::{LazyLock, Mutex, MutexGuard};

use crate::device::drivers::fpga;
use crate::device::Device;
use crate::ui::{self, ButtonMap};

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GamepadId(pub u32);

struct Gamepad {
    /// ID of the gamepad
    pub id: GamepadId,
    /// Current input state.
    pub state: InputState,
}

#[derive(Debug, Clone, Default)]
pub struct InputState {
    pub btn_a: bool,
    pub btn_b: bool,
    pub btn_x: bool,
    pub btn_y: bool,

    pub btn_up: bool,
    pub btn_down: bool,
    pub btn_right: bool,
    pub btn_left: bool,

    pub btn_system: bool,
    pub btn_select: bool,
    pub btn_start: bool,
    pub btn_capture: bool,

    pub btn_vol_up: bool,
    pub btn_vol_down: bool,
    pub btn_power: bool,

    pub btn_l1: bool,
    pub btn_r1: bool,
    pub btn_l2: bool,
    pub btn_r2: bool,
    pub btn_l3: bool,
    pub btn_r3: bool,

    pub axis_lx: i16,
    pub axis_ly: i16,
    pub axis_lz: i16,
    pub axis_rx: i16,
    pub axis_ry: i16,
    pub axis_rz: i16,
}

impl InputState {
    fn merge_axis(a: &mut i16, b: i16) {
        if b.unsigned_abs() > a.unsigned_abs() {
            *a = b;
        }
    }

    fn merge(&mut self, other: &InputState) {
        self.btn_a |= other.btn_a;
        self.btn_b |= other.btn_b;
        self.btn_x |= other.btn_x;
        self.btn_y |= other.btn_y;
        self.btn_up |= other.btn_up;
        self.btn_down |= other.btn_down;
        self.btn_right |= other.btn_right;
        self.btn_left |= other.btn_left;
        self.btn_system |= other.btn_system;
        self.btn_select |= other.btn_select;
        self.btn_start |= other.btn_start;
        self.btn_capture |= other.btn_capture;
        self.btn_vol_up |= other.btn_vol_up;
        self.btn_vol_down |= other.btn_vol_down;
        self.btn_power |= other.btn_power;
        self.btn_l1 |= other.btn_l1;
        self.btn_r1 |= other.btn_r1;
        self.btn_l2 |= other.btn_l2;
        self.btn_r2 |= other.btn_r2;
        self.btn_l3 |= other.btn_l3;
        self.btn_r3 |= other.btn_r3;
        Self::merge_axis(&mut self.axis_lx, other.axis_lx);
        Self::merge_axis(&mut self.axis_ly, other.axis_ly);
        Self::merge_axis(&mut self.axis_lz, other.axis_lz);
        Self::merge_axis(&mut self.axis_rx, other.axis_rx);
        Self::merge_axis(&mut self.axis_ry, other.axis_ry);
        Self::merge_axis(&mut self.axis_rz, other.axis_rz);
    }
}

static INPUT_MANAGER: LazyLock<Mutex<InputManager>> =
    LazyLock::new(|| Mutex::new(InputManager::default()));

#[derive(Default)]
pub struct InputManager {
    /// The state of the internal buttons.
    internal_state: InputState,

    /// External gamepads
    gamepads: Vec<Gamepad>,

    /// Last button map sent to UI
    last_button_map: ButtonMap,
}

impl InputManager {
    pub fn lock() -> MutexGuard<'static, Self> {
        // TODO: can we just only do this from the worker thread?
        INPUT_MANAGER.lock().unwrap()
    }

    /// Update the state of the internal buttons.
    pub fn update_state(&mut self, state: InputState) {
        self.internal_state = state;
        self.send_event();
    }

    fn find_gamepad(&mut self, id: GamepadId) -> Option<&mut Gamepad> {
        self.gamepads.iter_mut().find(|g| g.id == id)
    }

    pub fn add_gamepad(&mut self, id: GamepadId) {
        match self.find_gamepad(id) {
            Some(_) => log::warn!("Ignoring duplicate gamepad"),
            None => log::info!("Adding gamepad id={}", id.0),
        }
        self.gamepads.push(Gamepad {
            id,
            state: InputState::default(),
        });
    }

    pub fn remove_gamepad(&mut self, id: GamepadId) {
        let index = match self.gamepads.iter().position(|g| g.id == id) {
            Some(index) => index,
            None => {
                log::warn!("Not removing unknown gamepad");
                return;
            }
        };
        log::info!("Removing gamepad id={}", id.0);
        self.gamepads.remove(index);
        self.send_event();
    }

    pub fn remove_all_gamepads(&mut self) {
        log::info!("Removing all gamepads");
        self.gamepads.clear();
        self.send_event();
    }

    pub fn update_gamepad(&mut self, id: GamepadId, state: InputState) {
        if let Some(gamepad) = self.find_gamepad(id) {
            gamepad.state = state;

            if crate::kvs::keys::DOCK_SWAP_ABXY.get().unwrap() {
                std::mem::swap(&mut gamepad.state.btn_a, &mut gamepad.state.btn_b);
                std::mem::swap(&mut gamepad.state.btn_x, &mut gamepad.state.btn_y);
            }

            self.send_event();
        }
    }

    /// Send an input event to the UI thread, if needed.
    ///
    /// Also update the FPGA, if needed?
    fn send_event(&mut self) {
        // TODO: handle button remapping before merge
        let mut state = self.internal_state.clone();
        for gamepad in &self.gamepads {
            state.merge(&gamepad.state);

            // Temporary: map left analog stick to DPAD
            if gamepad.state.axis_lx <= -16384 {
                state.btn_left = true;
            }
            if gamepad.state.axis_lx >= 16384 {
                state.btn_right = true;
            }
            if gamepad.state.axis_ly <= -16384 {
                state.btn_up = true;
            }
            if gamepad.state.axis_ly >= 16384 {
                state.btn_down = true;
            }
        }

        use ui::buttons::Button;
        let buttons = enum_map::enum_map! {
            Button::A => state.btn_a,
            Button::B => state.btn_b,
            Button::X => state.btn_x,
            Button::Y => state.btn_y,
            Button::Up => state.btn_up,
            Button::Down => state.btn_down,
            Button::Left => state.btn_left,
            Button::Right => state.btn_right,
            Button::Start => state.btn_start,
            Button::Select => state.btn_select,
            Button::L => state.btn_l1,
            Button::R => state.btn_r1,
            Button::Home => state.btn_system,
            Button::VolUp => state.btn_vol_up,
            Button::VolDown => state.btn_vol_down,
            Button::Power => state.btn_power,
        };

        if self.last_button_map != buttons {
            ui::send(ui::Message::Button(buttons));

            // Send new button state to FPGA
            let fpga_buttons = if !self.gamepads.is_empty() {
                0u32 | ((state.btn_a as u32) << 11)
                    | ((state.btn_b as u32) << 10)
                    | ((state.btn_x as u32) << 9)
                    | ((state.btn_y as u32) << 8)
                    | ((state.btn_up as u32) << 7)
                    | ((state.btn_down as u32) << 6)
                    | ((state.btn_left as u32) << 5)
                    | ((state.btn_right as u32) << 4)
                    | ((state.btn_l1 as u32) << 3)
                    | ((state.btn_r1 as u32) << 2)
                    | ((state.btn_start as u32) << 1)
                    | ((state.btn_select as u32) << 0)
            } else {
                0
            };
            let _ = Device::lock()
                .fpga
                .write_u32(fpga::REG_CTRL_BUTTON_FORCE, fpga_buttons);
        }
        self.last_button_map = buttons;
    }
}
