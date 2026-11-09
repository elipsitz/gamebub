use std::path::{Path, PathBuf};

use crate::ui;

use super::{CoreManager, Stage, DIR_SDCARD};

/// Maximum number of files to show at once.
/// TODO: save memory and increase this limit (or avoid altogether)
const FILE_LIST_MAX: usize = 100;

impl CoreManager {
    pub(super) fn next_file_select(&mut self) {
        let core = self.core_info.as_ref().unwrap();
        let index = loop {
            // Find the index of the next file to load.
            let index = match self.stage {
                Stage::LoadInit => 0,
                Stage::LoadSelectFile(i) => i + 1,
                _ => panic!(),
            };
            if index >= core.files.len() {
                log::info!("File selection complete");
                self.stage = Stage::LoadBitstream;
                self.finish_loading();
                return;
            }
            self.stage = Stage::LoadSelectFile(index);
            let file = &core.files[index];
            if !file.user_selected {
                continue;
            }
            if self.run_cartridge && (file.id == 0 || file.dependent_on_0) {
                // TODO: validate and make sure there's a file 0
                continue;
            }
            break index;
        };
        let file = &core.files[index];

        // Get the starting path for the file browser.
        let last_path = self
            .core_settings
            .as_ref()
            .and_then(|x| x.file_paths.iter().find(|(id, _)| *id == file.id))
            .map(|(_, path)| path.as_path());
        let last_dir = last_path.and_then(|p| p.parent());

        // Use the last path, or the last dir, or finally the root path.
        let (initial_path, initial_dir) = match (last_path, last_dir) {
            (Some(file), Some(parent)) if file.is_file() => (file, parent),
            (_, Some(parent)) if parent.is_dir() => (parent, parent),
            _ => (Path::new(DIR_SDCARD), Path::new(DIR_SDCARD)),
        };

        ui::send(ui::Message::CoreFileSelectBegin {
            label: file.label.to_string(),
            path: initial_path.to_path_buf(),
        });
        self.send_core_file_list(initial_dir);
    }

    /// Called when a file has been selected for the core.
    /// This could be a file or a directory.
    pub fn handle_file_selected(&mut self, path: PathBuf) {
        let is_none = path.as_os_str().is_empty();
        if is_none || path.is_file() {
            log::info!("File selected: {}", path.display());
            let Stage::LoadSelectFile(file_index) = self.stage else {
                panic!()
            };

            // Update settings
            let settings = self.core_settings.as_mut().unwrap();
            let file_id = self.core_info.as_ref().unwrap().files[file_index].id;
            settings.file_paths.retain(|x| x.0 != file_id);

            if is_none {
                self.selected_files[file_index] = None;
            } else {
                settings.file_paths.push((file_id, path.clone()));
                self.selected_files[file_index] = Some(path);
            }

            self.next_file_select();
        } else if path.is_dir() {
            self.send_core_file_list(&path);
        }
    }

    /// Called if a file selection is cancelled.
    pub fn cancel_file_select(&mut self) {
        // TODO support multiple file select (go back to previous file)
        log::info!("File select cancelled");
        self.core_info = None;
        self.stage = Stage::Idle;
    }

    fn send_core_file_list(&self, path: &Path) {
        let files = match self.list_core_files(path) {
            Ok(files) => files,
            Err(e) => {
                log::warn!("Error listing directory: {:?}", e);
                ui::send(ui::Message::CoreFileSelectError(format!(
                    "Error listing directory:\n{}",
                    e,
                )));
                Vec::new()
            }
        };
        let truncated = files.len() >= FILE_LIST_MAX;
        ui::send(ui::Message::CoreFileSelectList(files));
        if truncated {
            let message =
                format!("More than {FILE_LIST_MAX} items in directory:\nsome items will be hidden");
            ui::send(ui::Message::CoreFileSelectError(message));
        }
    }

    /// Get the list of eligible files for the file select menu at the given directory
    fn list_core_files(&self, path: &Path) -> std::io::Result<Vec<(String, bool)>> {
        // Assumes we're in a valid file selection stage.
        let file_index = match self.stage {
            Stage::LoadSelectFile(i) => i,
            _ => panic!(),
        };

        // Prepend . to extensions to make matching easier
        let extensions = self.core_info.as_ref().unwrap().files[file_index]
            .extensions
            .iter()
            .map(|e| {
                let mut extension = arrayvec::ArrayString::<9>::new();
                extension.push('.');
                extension.push_str(e.as_str());
                extension
            })
            .collect::<arrayvec::ArrayVec<_, 4>>();

        let mut files = path
            .read_dir()?
            .filter_map(|e| {
                let e = e.ok()?;
                let name = e.file_name();
                let name = name.to_str()?;
                let kind = e.metadata().ok()?.file_type();
                if name.starts_with(".") {
                    return None;
                }
                if kind.is_file() && !extensions.iter().any(|&e| name.ends_with(e.as_str())) {
                    return None;
                }

                Some((name.to_string(), kind))
            })
            .take(FILE_LIST_MAX)
            .collect::<Vec<_>>();
        files.sort_unstable_by(|f1, f2| {
            // 1) Directories first
            let directory_order = f1.1.is_file().cmp(&f2.1.is_file());
            // 2) then naturally by filename
            directory_order.then_with(|| natord::compare_ignore_case(&f1.0, &f2.0))
        });

        // If this file is optional, make a "nope" entry.
        let no_file = if self.core_info.as_ref().unwrap().files[file_index].optional {
            Some((String::new(), false))
        } else {
            None
        };
        let files =
            std::iter::chain(no_file, files.into_iter().map(|f| (f.0, f.1.is_dir()))).collect();
        Ok(files)
    }
}
