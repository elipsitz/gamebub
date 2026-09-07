pub mod buttons;
mod screenshot;
pub mod slint;
mod state;

use ::slint::platform::WindowAdapter;
use ::slint::{ComponentHandle, WindowSize};
pub use buttons::{Button, ButtonEvent, ButtonMap};
use std::path::PathBuf;
use std::sync::{mpsc, OnceLock};
use std::time::Duration;
use std::{cell::RefCell, rc::Rc, sync::mpsc::Receiver, time::Instant};

use ::slint::{
    platform::software_renderer::{LineBufferProvider, RepaintBufferType, TargetPixel},
    PhysicalSize, Timer, TimerMode,
};

use crate::device::{Device, DisplayMode};
use crate::input::{self, InputManager};
pub use crate::ui::state::CoreSettingUiItem;
use crate::worker;
pub use state::notifications::Notification;

use self::{slint::Argb1555, slint::MinimalSoftwareWindow, state::UiState};

const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

cfg_if::cfg_if! {
    if #[cfg(any(feature = "rev1", feature = "rev2"))] {
        const DISPLAY_WIDTH: usize = 480 / 2;
        const DISPLAY_HEIGHT: usize = 320 / 2;
    } else {
        const DISPLAY_WIDTH: usize = 720 / 2;
        const DISPLAY_HEIGHT: usize = 480 / 2;
    }
}

pub enum Message {
    /// The state of the buttons have changed.
    /// TODO: combine this with InputState/Gamepad handling
    Button(ButtonMap),
    /// Battery status has changed.
    BatteryStatus { level: f32 },
    /// Redraw the entire screen (e.g. after a display change)
    Redraw,
    /// Go to the "Game" screen
    EnterGame,
    /// Enter the error screen, and show the given error
    FatalError(String),
    /// Internal input state changed
    InputState(input::InputState),
    /// Gamepad connected
    GamepadConnected(input::GamepadId),
    /// Gamepad disconnected
    GamepadDisconnected(input::GamepadId),
    /// Gamepad input event
    GamepadInput(input::GamepadId, input::InputState),
    /// Show a notification
    Notification(Notification),
    /// Docked
    DockBegin { serial: String, firmware: String },
    /// Dock ended
    DockEnd,
    /// Take a UI screenshot
    Screenshot,
    /// Core info listing
    CoreList(Vec<crate::core::CoreListEntry>),
    /// Open core file selection screen
    CoreFileSelectBegin { label: String, path: PathBuf },
    /// Update core file selection list
    CoreFileSelectList(Vec<(String, bool)>),
    /// Core file select error
    CoreFileSelectError(String),
    /// Core load error
    CoreLoadError(String),
    /// Core loading progress
    CoreLoadProgress(f32),
    /// Core settings
    CoreSettingsList(Vec<CoreSettingUiItem>),
}

/// Send a message to the UI thread.
pub fn send(message: Message) {
    match SENDER.get() {
        Some(sender) => sender.send(message).unwrap(),
        None => log::error!("Dropping UI message"),
    }
}

static SENDER: OnceLock<mpsc::Sender<Message>> = OnceLock::new();

/// Line renderer that renders to the FPGA
struct FpgaLineRenderer<'a, 'b, 'c> {
    device: &'a mut Device<'b>,
    line_buffer: &'c mut [Argb1555],
}

impl<'a, 'b, 'c> LineBufferProvider for &mut FpgaLineRenderer<'a, 'b, 'c> {
    type TargetPixel = Argb1555;

    fn process_line(
        &mut self,
        line: usize,
        range: core::ops::Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        let start = ((DISPLAY_WIDTH * line) + range.start) * 2;
        let buffer = &mut self.line_buffer[range];
        render_fn(buffer);

        let slice = {
            let len = buffer.len() * 2;
            unsafe { std::slice::from_raw_parts(buffer.as_ptr() as *const u8, len) }
        };
        let _ = self.device.fpga.write_overlay(start as u32, slice);
    }
}

#[allow(unused)]
pub struct UI {
    framebuffer: Vec<Argb1555>,
    lcd_line_buffer: Vec<u8>,
    window: Rc<MinimalSoftwareWindow>,
    message_queue: Receiver<Message>,
    root: slint::MainWindow,
    state: Rc<RefCell<UiState>>,
    button_event_detector: buttons::ButtonEventDetector,
    idle_timer: Timer,
}

impl UI {
    pub fn new(device: &mut Device) -> Self {
        device
            .fpga
            .write_u32(crate::bitstream::boot::REG_LOGO_Y, 38)
            .unwrap();
        let display_mode = if device.docked {
            DisplayMode::External
        } else {
            DisplayMode::Internal
        };
        device.change_display_mode(display_mode).unwrap();
        // Start the boot animation after a delay.
        Timer::single_shot(Duration::from_millis(500), || {
            let animation = 1 | (6 << 2); // 6: 0.66 seconds
            Device::lock()
                .fpga
                .write_u32(crate::bitstream::boot::REG_LOGO_ANIM, animation)
                .unwrap(); // Start animation (no loop)
        });

        // Set up the idle timer
        let idle_timer = Timer::default();
        idle_timer.start(TimerMode::Repeated, IDLE_TIMEOUT, || {
            worker::send(worker::Message::IdleTimerExpired);
        });

        let (sender, receiver) = mpsc::channel::<Message>();
        SENDER.set(sender).expect("UI already initialized");

        let framebuffer = vec![Argb1555::from_rgb(0, 0, 0); DISPLAY_WIDTH];
        let lcd_line_buffer = vec![0u8; DISPLAY_WIDTH * 3 * 2];

        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        ::slint::platform::set_platform(Box::new(slint::HandheldPlatform {
            window: window.clone(),
        }))
        .unwrap();
        window.set_size(WindowSize::Physical(PhysicalSize::new(
            DISPLAY_WIDTH as u32,
            DISPLAY_HEIGHT as u32,
        )));

        let root = slint::MainWindow::new().unwrap();

        let ui = UI {
            framebuffer,
            lcd_line_buffer,
            window,
            message_queue: receiver,
            state: UiState::new(&root, device),
            root,
            button_event_detector: buttons::ButtonEventDetector::new(),
            idle_timer,
        };
        ui
    }

    pub fn run(&mut self) -> ! {
        // Set this thread (UI) to higher priority than background threads.
        unsafe { esp_idf_svc::sys::vTaskPrioritySet(std::ptr::null_mut(), 10) };

        let mut pending_message = None;
        loop {
            // Process messages.
            while let Some(message) = pending_message {
                self.dispatch_message(message);
                pending_message = self.message_queue.try_recv().ok();
            }
            for button_event in self.button_event_detector.update(None) {
                self.window.dispatch_event(button_event.into());
                self.idle_timer.restart();
            }

            ::slint::platform::update_timers_and_animations();

            // Render UI if needed.
            self.window.draw_if_needed(|renderer| {
                let mut device = Device::lock();

                let render_start = Instant::now();
                {
                    let mut line_renderer = FpgaLineRenderer {
                        device: &mut device,
                        line_buffer: &mut self.framebuffer,
                    };
                    renderer.render_by_line(&mut line_renderer);
                };
                let render_duration = render_start.elapsed();
                log::info!("Render + display {}ms", render_duration.as_millis() as u32,);

                // If we changed the repaint buffer type to force a redraw, change it back.
                if renderer.repaint_buffer_type() == RepaintBufferType::NewBuffer {
                    renderer.set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
                    // For some reason, this doesn't get called automatically after the first render.
                    self.window.request_redraw();
                }
            });

            // Trigger a timer to wake us up for button repeat events.
            if let Some(wakeup) = self.button_event_detector.next_wakeup_time() {
                Timer::single_shot(wakeup.saturating_duration_since(Instant::now()), || ());
            }

            // Sleep until the next animation, timer, or event.
            if !self.window.has_active_animations() {
                match ::slint::platform::duration_until_next_timer_update() {
                    Some(duration) => {
                        pending_message = self.message_queue.recv_timeout(duration).ok();
                    }
                    None => {
                        pending_message = self.message_queue.recv().ok();
                    }
                }
            }
        }
    }

    /// Handle a message sent to the UI thread.
    fn dispatch_message(&mut self, message: Message) {
        match message {
            Message::Button(state) => {
                self.idle_timer.restart();
                for button_event in self.button_event_detector.update(Some(state)) {
                    self.window.dispatch_event(button_event.into());
                }
            }
            Message::BatteryStatus { level } => {
                self.state.borrow_mut().update_battery_level(level);
            }
            Message::Redraw => {
                log::info!("Refreshing screen");
                self.window.request_redraw();

                // Force renderer to clear the dirty region and re-render everything.
                self.window
                    .renderer
                    .set_repaint_buffer_type(RepaintBufferType::NewBuffer);
            }
            Message::EnterGame => {
                self.root
                    .global::<slint::Backend>()
                    .set_core_is_loading(false);
                self.root.invoke_set_screen(slint::ScreenId::Game);
            }
            Message::CoreLoadProgress(progress) => {
                self.root
                    .global::<slint::Backend>()
                    .set_core_is_loading(true);
                self.root
                    .global::<slint::Backend>()
                    .set_core_progress(progress * 100.0);
            }
            Message::FatalError(error) => {
                self.root
                    .global::<slint::Backend>()
                    .set_error_text(error.into());
                self.root.invoke_set_screen(slint::ScreenId::Error);
            }
            Message::InputState(state) => InputManager::lock().update_state(state),
            Message::GamepadConnected(id) => InputManager::lock().add_gamepad(id),
            Message::GamepadDisconnected(id) => InputManager::lock().remove_gamepad(id),
            Message::GamepadInput(id, state) => InputManager::lock().update_gamepad(id, state),
            Message::Notification(notification) => {
                self.state
                    .borrow_mut()
                    .queue_notification(self.state.clone(), notification);
            }
            Message::DockBegin { serial, firmware } => {
                let backend = self.root.global::<slint::Backend>();
                backend.set_dock_serial(serial.into());
                backend.set_dock_firmware_version(firmware.into());
                backend.set_docked(true);
            }
            Message::DockEnd => {
                self.root.global::<slint::Backend>().set_docked(false);
            }
            Message::Screenshot => {
                match screenshot::save_ui_screenshot(&self.window, &mut self.framebuffer) {
                    Ok(f) => log::info!("Saved UI screenshot to {f}"),
                    Err(e) => log::error!("Screenshot error: {e}"),
                }
            }
            Message::CoreList(list) => {
                self.state.borrow_mut().cores_list(list);
            }
            Message::CoreFileSelectBegin { label, path } => {
                self.state.borrow_mut().cores_file_select_begin(label, path);
            }
            Message::CoreFileSelectList(files) => {
                self.state.borrow_mut().cores_file_select_list(files);
            }
            Message::CoreFileSelectError(error) => {
                self.state.borrow_mut().cores_file_select_set_error(error);
            }
            Message::CoreLoadError(error) => {
                self.state.borrow_mut().cores_set_error(error);
            }
            Message::CoreSettingsList(items) => {
                self.state.borrow_mut().game_settings_list(items);
            }
        }
    }
}
