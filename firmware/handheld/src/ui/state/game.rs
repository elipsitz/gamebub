use std::{cell::RefCell, rc::Rc, time::Duration};

use super::super::slint::Backend;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    core::CoreManager,
    device::Device,
    ui::slint::{CoreSettingType, ScreenId},
    worker,
};

use super::UiState;

pub struct CoreSettingUiItem {
    pub id: u16,
    pub label: SharedString,
    pub setting_type: CoreSettingType,
    pub value: u32,
    pub choices: Vec<SharedString>,
}

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
        let state_ = state.clone();
        backend.on_game_settings_load(move || {
            // Clear existing settings
            let state = state_.borrow_mut();
            let root: crate::ui::slint::MainWindow = state.root.unwrap();
            let backend = root.global::<Backend>();
            backend.set_core_settings(ModelRc::default());

            // Fetch new settings
            worker::send(worker::Message::CoreSettingsLoad);
        });
        backend.on_game_settings_set(move |id, value| {
            worker::send(worker::Message::CoreSettingChanged {
                id: id as u16,
                value: value as u32,
            });
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

    pub fn game_settings_list(&mut self, list: Vec<CoreSettingUiItem>) {
        let list = ModelRc::from(Rc::new(VecModel::from(
            list.into_iter()
                .map(|item| crate::ui::slint::CoreSetting {
                    id: item.id as i32,
                    label: item.label,
                    r#type: item.setting_type,
                    value: item.value as i32,
                    choices: ModelRc::from(Rc::new(VecModel::from(item.choices))),
                })
                .collect::<Vec<_>>(),
        )));

        let root = self.root.unwrap();
        let backend = root.global::<Backend>();
        backend.set_core_settings(list);
    }
}
