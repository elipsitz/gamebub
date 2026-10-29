use enum_map::{Enum, EnumMap};
use slint::platform::{Key, WindowEvent};
use std::time::{Duration, Instant};

const REPEAT_INITIAL_DELAY: Duration = Duration::from_millis(450);
const REPEAT_DELAY: Duration = Duration::from_millis(50);

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Enum)]
pub enum Button {
    A,
    B,
    X,
    Y,
    Up,
    Down,
    Left,
    Right,
    L,
    R,
    Start,
    Select,
    Home,
    VolUp,
    VolDown,
    Power,
}

pub type ButtonMap = EnumMap<Button, bool>;

#[derive(Copy, Clone, Debug)]
pub enum ButtonEvent {
    Pressed(Button, bool),
    Released(Button),
}

impl From<ButtonEvent> for slint::platform::WindowEvent {
    fn from(val: ButtonEvent) -> Self {
        let button = match val {
            ButtonEvent::Pressed(b, _) => b,
            ButtonEvent::Released(b) => b,
        };
        let text: slint::SharedString = match button {
            Button::A => "a".into(),
            Button::B => "b".into(),
            Button::X => "x".into(),
            Button::Y => "y".into(),
            Button::Up => Key::UpArrow.into(),
            Button::Down => Key::DownArrow.into(),
            Button::Left => Key::LeftArrow.into(),
            Button::Right => Key::RightArrow.into(),
            Button::L => "l".into(),
            Button::R => "r".into(),
            Button::Start => "Start".into(),
            Button::Select => "Select".into(),
            Button::Home => "Home".into(),
            Button::VolUp => "VolUp".into(),
            Button::VolDown => "VolDown".into(),
            Button::Power => "Power".into(),
        };
        match val {
            ButtonEvent::Pressed(_, false) => WindowEvent::KeyPressed { text },
            ButtonEvent::Pressed(_, true) => WindowEvent::KeyPressRepeated { text },
            ButtonEvent::Released(_) => WindowEvent::KeyReleased { text },
        }
    }
}

struct ButtonState {
    pressed: bool,
    last_press: Option<Instant>,
    repeat_count: u32,
}

impl ButtonState {
    fn new() -> Self {
        ButtonState {
            pressed: false,
            last_press: None,
            repeat_count: 0,
        }
    }

    fn next_press_repeat(&self) -> Option<Instant> {
        match (self.pressed, self.repeat_count) {
            (false, _) => None,
            (true, 0) => Some(self.last_press.unwrap() + REPEAT_INITIAL_DELAY),
            (true, _) => Some(self.last_press.unwrap() + REPEAT_DELAY),
        }
    }
}

/// Detects [`ButtonEvent`]s by diffing the state of buttons.
pub struct ButtonEventDetector {
    state: EnumMap<Button, ButtonState>,
}

impl Default for ButtonEventDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl ButtonEventDetector {
    pub fn new() -> Self {
        ButtonEventDetector {
            state: EnumMap::from_fn(|_| ButtonState::new()),
        }
    }

    pub fn update(&mut self, new_state: Option<ButtonMap>) -> impl Iterator<Item = ButtonEvent> {
        let now = Instant::now();
        let mut events = EnumMap::from_fn(|_| None);
        for (button, state) in self.state.iter_mut() {
            match (
                state.pressed,
                new_state.map_or(state.pressed, |s| s[button]),
            ) {
                (false, true) => {
                    // Just pressed
                    state.pressed = true;
                    state.last_press = Some(now);
                    state.repeat_count = 0;
                    events[button] = Some(ButtonEvent::Pressed(button, false));
                }
                (true, true) => {
                    // Still pressed
                    if state.next_press_repeat().unwrap() <= now {
                        state.last_press = Some(now);
                        state.repeat_count += 1;
                        events[button] = Some(ButtonEvent::Pressed(button, true));
                    }
                }
                (true, false) => {
                    // Just released
                    state.pressed = false;
                    events[button] = Some(ButtonEvent::Released(button));
                }
                _ => {}
            }
        }

        events.into_iter().filter_map(|(_, e)| e)
    }

    pub fn next_wakeup_time(&self) -> Option<Instant> {
        self.state
            .iter()
            .filter_map(|(_, s)| s.next_press_repeat())
            .max()
    }
}
