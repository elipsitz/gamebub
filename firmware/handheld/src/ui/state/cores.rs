use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use super::super::slint::Backend;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::{
    device::Device,
    ui::slint::{CoreSubscreen, FileIcon, ScreenId},
    worker,
};

use super::UiState;

pub const BASE_DIR: &str = "/sdcard/";

struct FileListModel {
    data: Vec<(String, bool)>,
}

impl Model for FileListModel {
    type Data = crate::ui::slint::FileListEntry;

    fn row_count(&self) -> usize {
        self.data.len()
    }

    fn row_data(&self, row: usize) -> Option<Self::Data> {
        if row >= self.data.len() {
            return None;
        }
        let (name, is_dir) = &self.data[row];
        Some(crate::ui::slint::FileListEntry {
            name: SharedString::from(name),
            icon: if *is_dir {
                FileIcon::Folder
            } else {
                FileIcon::Blank
            },
        })
    }

    fn model_tracker(&self) -> &dyn slint::ModelTracker {
        &()
    }
}

impl UiState {
    /// Set up the "Cores" screen.
    pub(super) fn setup_cores(&mut self, state: &Rc<RefCell<UiState>>, _device: &mut Device) {
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();

        backend.on_core_list_fetch(move || {
            worker::send(worker::Message::FetchCoreList);
        });

        let state_ = state.clone();
        backend.on_core_run(move |core_id| {
            let mut state = state_.borrow_mut();
            state.cores_handle_run(core_id);
        });

        let state_ = state.clone();
        backend.on_core_file_select_selected(move |index| {
            let mut state = state_.borrow_mut();
            let root: crate::ui::slint::MainWindow = state.root.unwrap();
            let backend = root.global::<Backend>();
            let list = backend.get_core_file_select_list();
            if let Some(data) = list.row_data(index as usize) {
                let path = state.core_file_select_directory.join(data.name.as_str());
                state.cores_handle_file_select_selected(path, data.name.as_str())
            }
        });

        let state_ = state.clone();
        backend.on_core_file_select_up(move || {
            let mut state = state_.borrow_mut();
            state.cores_handle_file_select_selected(PathBuf::new(), "..");
        });

        backend.on_core_file_select_cancel(move || {
            worker::send(worker::Message::CoreFileCancelled);
        });
    }

    pub fn cores_handle_run(&mut self, core_id: SharedString) {
        self.game_cartridge = false;
        worker::send(worker::Message::RunCore(core_id.to_string()));
    }

    pub fn cores_list(&mut self, list: Vec<crate::core::CoreListEntry>) {
        let list = ModelRc::from(Rc::new(VecModel::from(
            list.into_iter()
                .map(|item| crate::ui::slint::CoreListEntry {
                    core_id: item.id.as_str().into(),
                    core_name: item.name.as_str().into(),
                    core_author: item.author.as_str().into(),
                })
                .collect::<Vec<_>>(),
        )));
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();
        backend.set_core_list(list);
    }

    pub fn cores_file_select_begin(&mut self, label: String, path: PathBuf) {
        // `path` may be either a specific file (in which case
        // that file should be selected), or a directory.
        if path.is_file() {
            self.core_file_select_filename =
                path.file_name().unwrap().to_str().unwrap().to_string();
            self.core_file_select_directory = {
                let mut path = path;
                path.pop();
                path
            };
        } else {
            self.core_file_select_filename.clear();
            self.core_file_select_directory = path;
        }

        let root = self.root.unwrap();
        let backend = root.global::<Backend>();
        root.invoke_set_title(format!("Select {label}").into());
        backend.set_core_file_select_label(label.into());
        backend.set_core_file_select_list(slint::ModelRc::default());
        backend.set_core_file_select_index(-1);
        backend.set_core_file_select_is_loading(true);
        self.cores_file_select_update_path();
        backend.set_core_subscreen(CoreSubscreen::FileSelect);
    }

    pub fn cores_file_select_list(&mut self, files: Vec<(String, bool)>) {
        // TODO: add .. (or do that in worker)

        let selected = files
            .iter()
            .position(|(f, _)| *f == self.core_file_select_filename)
            .unwrap_or(0);

        let root = self.root.unwrap();
        let backend = root.global::<Backend>();
        let files = ModelRc::new(FileListModel { data: files });
        backend.set_core_file_select_list(files);

        {
            // Defer setting the index and unsetting loading, because
            // the FileListView component can't handle shifting view to
            // a newly selected element until the first time it renders.
            let root = self.root.clone();
            self.core_file_select_timer.start(
                slint::TimerMode::SingleShot,
                Duration::from_millis(1),
                move || {
                    let root = root.unwrap();
                    let backend = root.global::<Backend>();
                    backend.set_core_file_select_index(selected as i32);
                    backend.set_core_file_select_is_loading(false);
                },
            );
        }
    }

    pub fn cores_file_select_update_path(&self) {
        // Remove base directory from name before displaying.
        let path = &self.core_file_select_directory;
        let mut directory = path
            .strip_prefix(BASE_DIR)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        if !directory.starts_with("/") {
            directory.insert_str(0, "/");
        }

        let root = self.root.unwrap();
        let backend = root.global::<Backend>();
        backend.set_core_file_select_path(directory.into());
    }

    pub fn cores_handle_file_select_selected(&mut self, path: PathBuf, filename: &str) {
        if filename == ".." {
            if self.core_file_select_directory == Path::new(BASE_DIR) {
                log::warn!("No parent directory");
                return;
            }

            // Highlight the directory we're just leaving.
            self.core_file_select_filename = self
                .core_file_select_directory
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            self.core_file_select_directory.pop();
            worker::send(worker::Message::CoreFileSelected(
                self.core_file_select_directory.clone(),
            ));
        } else if path.is_dir() {
            log::info!("Entering subdirectory {}", filename);
            self.core_file_select_filename.clear();
            self.core_file_select_directory.push(filename);
            worker::send(worker::Message::CoreFileSelected(
                self.core_file_select_directory.clone(),
            ));
        } else {
            worker::send(worker::Message::CoreFileSelected(path));
        }

        self.root
            .unwrap()
            .global::<Backend>()
            .set_core_file_select_is_loading(true);
        self.cores_file_select_update_path();
    }

    pub fn cores_file_select_set_error(&mut self, error: String) {
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();
        backend.set_core_file_select_is_loading(false);
        backend.set_core_file_select_error(error.into());
        self.cores_file_select_update_path();
    }

    pub fn cores_set_error(&mut self, error: String) {
        let root = self.root.unwrap();
        let backend = root.global::<Backend>();
        backend.set_core_error(error.into());
        backend.set_core_subscreen(CoreSubscreen::Error);
        root.invoke_set_screen(ScreenId::Cores);
    }
}
