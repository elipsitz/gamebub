use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::CoreManager;
use crate::ui::CoreSettingUiItem;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CoreSettings {
    /// A list of files and their most recently selected paths.
    #[serde(rename = "_file_paths")]
    pub file_paths: Vec<(u16, PathBuf)>,

    #[serde(flatten)]
    pub settings: HashMap<String, serde_json::Value>,
}

impl CoreManager {
    pub fn ui_load_settings(&self) {
        use super::CoreSettingType::*;
        let core = match self.core_info.as_ref() {
            Some(x) => x,
            None => return,
        };

        let list = core
            .settings
            .iter()
            .map(|setting| CoreSettingUiItem {
                id: setting.id,
                label: setting.label.clone(),
                setting_type: match &setting.inner {
                    Action { .. } => crate::ui::slint::CoreSettingType::Action,
                    Checkbox { .. } => crate::ui::slint::CoreSettingType::Checkbox,
                    List { .. } => crate::ui::slint::CoreSettingType::List,
                },
                value: 0, // TODO
                choices: match &setting.inner {
                    List { items } => items.iter().map(|x| x.label.clone()).collect(),
                    _ => Vec::new(),
                },
            })
            .collect();
        crate::ui::send(crate::ui::Message::CoreSettingsList(list));
    }

    pub fn setting_changed(&mut self, id: u16, value: u32) {
        let core = match self.core_info.as_ref() {
            Some(x) => x,
            None => return,
        };
        let setting = match core.settings.iter().find(|x| x.id == id) {
            Some(x) => x,
            None => return,
        };
        log::info!("Core setting {} => {}", setting.label, value);
    }
}
