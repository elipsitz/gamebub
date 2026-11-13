use std::sync::{LazyLock, Mutex, MutexGuard};

use crate::device::drivers::fpga;
use crate::device::Device;
use crate::ui::{self, ButtonMap};

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GamepadId(pub u32);

struct Gamepad {
    /// ID of the gamepad
    pub id: GamepadId,
    /// Whether the gamepad has analog sticks
    pub has_analog: bool,
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

    fn merge_gamepads(&mut self, gamepads: &[Gamepad]) {
        for gamepad in gamepads {
            self.merge(&gamepad.state);

            const THRESHOLD: i16 = i16::MAX / 2;
            self.btn_left |= gamepad.state.axis_lx <= -THRESHOLD;
            self.btn_right |= gamepad.state.axis_lx >= THRESHOLD;
            self.btn_up |= gamepad.state.axis_ly <= -THRESHOLD;
            self.btn_down |= gamepad.state.axis_ly >= THRESHOLD;
        }
    }
}

static INPUT_MANAGER: LazyLock<Mutex<InputManager>> =
    LazyLock::new(|| Mutex::new(InputManager::default()));

#[derive(Default)]
enum CoreInputMode {
    #[default]
    None,
    Basic {
        prev_update: u32,
    },
    Extended {
        num_slots: usize,
        prev_update: [[u32; 3]; 4],
    },
}

#[derive(Default)]
pub struct InputManager {
    /// The state of the internal buttons.
    internal_state: InputState,

    /// External gamepads
    gamepads: Vec<Gamepad>,

    /// Assigned gamepad slots
    slots: [Option<GamepadId>; 4],

    /// Current core input mode
    core_input: CoreInputMode,

    /// Last button map sent to UI
    last_button_map: ButtonMap,
}

const INPUT_CAPS_NUM_SLOTS_MASK: u32 = 0xF;
const INPUT_CAPS_EXTENDED: u32 = 1 << 7;

const DEVICE_TYPE_DIGITAL_GAMEPAD: u32 = 2;
const DEVICE_TYPE_ANALOG_GAMEPAD: u32 = 3;

fn pack_buttons(state: &InputState) -> u32 {
    ((state.btn_a as u32) << 15)
        | ((state.btn_b as u32) << 14)
        | ((state.btn_x as u32) << 13)
        | ((state.btn_y as u32) << 12)
        | ((state.btn_up as u32) << 11)
        | ((state.btn_down as u32) << 10)
        | ((state.btn_left as u32) << 9)
        | ((state.btn_right as u32) << 8)
        | ((state.btn_l1 as u32) << 7)
        | ((state.btn_r1 as u32) << 6)
        | ((state.btn_l2 as u32) << 5)
        | ((state.btn_r2 as u32) << 4)
        | ((state.btn_l3 as u32) << 3)
        | ((state.btn_r3 as u32) << 2)
        | ((state.btn_start as u32) << 1)
        | (state.btn_select as u32)
}

fn pack_gamepad_slot(gamepad: &Gamepad) -> [u32; 3] {
    let clamp_stick = |v: i16| -> u8 { (v / 256).clamp(-127, 127) as i8 as u8 };
    let device_type = if gamepad.has_analog {
        DEVICE_TYPE_ANALOG_GAMEPAD
    } else {
        DEVICE_TYPE_DIGITAL_GAMEPAD
    };
    let trigger_l = (gamepad.state.axis_lz.max(0) >> 7) as u8;
    let trigger_r = (gamepad.state.axis_rz.max(0) >> 7) as u8;

    let buttons = (device_type << 28) | pack_buttons(&gamepad.state);
    let sticks = u32::from_le_bytes([
        clamp_stick(gamepad.state.axis_lx),
        clamp_stick(gamepad.state.axis_ly),
        clamp_stick(gamepad.state.axis_rx),
        clamp_stick(gamepad.state.axis_ry),
    ]);
    let triggers = ((trigger_r as u32) << 8) | ((trigger_l as u32) << 0);
    [buttons, sticks, triggers]
}

impl InputManager {
    pub fn lock() -> MutexGuard<'static, Self> {
        // TODO: can we just only do this from the worker thread?
        INPUT_MANAGER.lock().unwrap()
    }

    /// Update the state of the internal buttons.
    pub fn update_state(&mut self, state: InputState) {
        self.internal_state = state;
        self.update_ui();
    }

    fn find_gamepad(&mut self, id: GamepadId) -> Option<&mut Gamepad> {
        self.gamepads.iter_mut().find(|g| g.id == id)
    }

    pub fn add_gamepad(&mut self, id: GamepadId, has_analog: bool) {
        if self.find_gamepad(id).is_some() {
            log::warn!("Ignoring duplicate gamepad id={}", id.0);
            return;
        }
        log::info!("Adding gamepad id={}", id.0);
        if let Some(slot) = self.slots.iter_mut().find(|s| s.is_none()) {
            *slot = Some(id);
        }
        self.gamepads.push(Gamepad {
            id,
            has_analog,
            state: InputState::default(),
        });
        ui::send(ui::Message::Notification(ui::Notification::new_short(
            "Gamepad connected".to_string(),
        )));
        self.update_ui();
        self.sync_fpga();
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
        if let Some(slot) = self.slots.iter_mut().find(|s| **s == Some(id)) {
            *slot = None;
        }
        ui::send(ui::Message::Notification(ui::Notification::new_short(
            "Gamepad disconnected".to_string(),
        )));
        self.update_ui();
        self.sync_fpga();
    }

    pub fn remove_all_gamepads(&mut self) {
        log::info!("Removing all gamepads");
        self.slots = [None; 4];
        self.gamepads.clear();
        self.update_ui();
        self.sync_fpga();
    }

    pub fn update_gamepad(&mut self, id: GamepadId, state: InputState) {
        if let Some(gamepad) = self.find_gamepad(id) {
            gamepad.state = state;

            if crate::kvs::keys::DOCK_SWAP_ABXY.get().unwrap() {
                std::mem::swap(&mut gamepad.state.btn_a, &mut gamepad.state.btn_b);
                std::mem::swap(&mut gamepad.state.btn_x, &mut gamepad.state.btn_y);
            }

            self.update_ui();
            self.sync_fpga();
        }
    }

    pub fn set_core_input_caps(&mut self, caps: u32) {
        let num_slots = ((caps & INPUT_CAPS_NUM_SLOTS_MASK) as usize).min(4);
        let is_extended = (caps & INPUT_CAPS_EXTENDED) != 0;
        log::info!("Core input caps: 0x{caps:08X} (num_slots={num_slots}, ext={is_extended})");
        self.core_input = if num_slots == 0 {
            CoreInputMode::None
        } else if is_extended {
            CoreInputMode::Extended {
                num_slots,
                prev_update: [[0; 3]; 4],
            }
        } else {
            CoreInputMode::Basic { prev_update: 0 }
        };
        self.sync_fpga();
    }

    pub fn clear_core_input_caps(&mut self) {
        self.core_input = CoreInputMode::None;
    }

    fn sync_fpga(&mut self) {
        match &mut self.core_input {
            CoreInputMode::None => {}
            CoreInputMode::Basic { prev_update } => {
                let mut state = InputState::default();
                state.merge_gamepads(&self.gamepads);
                let update = pack_buttons(&state);
                if update != *prev_update {
                    *prev_update = update;
                    let _ = Device::lock()
                        .fpga
                        .write_u32(fpga::REG_CTRL_INPUT_SLOT_BASE, update);
                }
            }
            CoreInputMode::Extended {
                num_slots,
                prev_update,
            } => {
                for (index, prev) in prev_update[..*num_slots].iter_mut().enumerate() {
                    let words = match self.slots[index] {
                        Some(id) => {
                            let gamepad = self.gamepads.iter().find(|g| g.id == id).unwrap();
                            pack_gamepad_slot(gamepad)
                        }
                        None => [0; 3],
                    };
                    if words != *prev {
                        *prev = words;
                        let _ = Device::lock().fpga.write_input_slot(index, words);
                    }
                }
            }
        }
    }

    /// Send an input event to the UI thread, if needed.
    fn update_ui(&mut self) {
        // TODO: handle button remapping before merge
        let mut state = self.internal_state.clone();
        state.merge_gamepads(&self.gamepads);

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
            self.last_button_map = buttons;
        }
    }
}
