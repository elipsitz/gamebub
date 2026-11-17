use std::cell::RefCell;

use super::super::slint::Backend;
use slint::ComponentHandle;

use crate::kvs;

use super::UiState;

impl UiState {
    /// Set up the "Setup" screen.
    pub(super) fn setup_setup(
        &mut self,
        _state: &'static RefCell<UiState>,
        _device: &mut crate::Device,
    ) {
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();

        backend.on_setup_complete(move || {
            kvs::keys::SETUP_STAGE.set(&1);
        });
    }
}
