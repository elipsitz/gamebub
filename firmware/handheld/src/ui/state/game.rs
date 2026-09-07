use std::{cell::RefCell, rc::Rc, time::Duration};

use super::super::slint::Backend;
use slint::ComponentHandle;

use crate::{core::CoreManager, device::Device, ui::slint::ScreenId, worker};

use super::UiState;

impl UiState {
    /// Set up the "Game" screen.
    pub(super) fn setup_game(&mut self, state: &Rc<RefCell<UiState>>, _device: &mut Device) {
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();

        backend.on_game_set_focused(move |focused| {
            worker::send(worker::Message::CoreFocusChanged(focused));
        });

        backend.on_game_reset(move || {
            CoreManager::lock().reset_core();
        });
        backend.on_game_settings_load(move || {
            // TODO
            log::info!("setting load");
        });
        backend.on_game_settings_set(move |index, value| {
            // TODO
            log::info!("setting {index} = {value:?}");
        });

        let state_ = state.clone();
        backend.on_game_exit(move || {
            worker::send(worker::Message::ExitCore);
            // Give it a moment to start loading the boot bitstream (avoid screen flash)
            std::thread::sleep(Duration::from_millis(100));
            // Go back to the main menu
            let root = {
                let state = state_.borrow_mut();
                state.root.unwrap()
            };
            root.invoke_set_screen(ScreenId::MainMenu);
        });
    }
}
