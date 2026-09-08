use std::{cell::RefCell, rc::Rc};

use super::super::slint::Backend;
use slint::ComponentHandle;

use crate::{device::Device, worker};

use super::UiState;

impl UiState {
    /// Set up the "Main Menu" screen.
    pub(super) fn setup_main_menu(&mut self, state: &Rc<RefCell<UiState>>, _device: &mut Device) {
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();

        let state_ = state.clone();
        backend.on_main_menu_run_cartridge(move || {
            state_.borrow_mut().game_cartridge = true;
            worker::send(worker::Message::RunCartridge)
        });
    }
}
