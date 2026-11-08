use std::{
    fs::File,
    io::{BufWriter, Read, Write},
    ops::DerefMut as _,
    path::PathBuf,
    time::{Duration, Instant},
};

use esp_idf_svc::hal::units::Hertz;

use crate::{
    device::{
        drivers::fpga::{SpiCommand, MAX_SPI_READ_CLOCK},
        Device,
    },
    ui,
};

use super::{
    command,
    CoreError::{self, *},
    CoreFile, CoreManager, Stage, DIR_SETTINGS, NOTIFY_TIMEOUT, PROGRESS_UPDATE_INTERVAL,
    SETUP_TIMEOUT,
};

/// Only perform background I/O when reading files at least this large.
const BG_IO_THRESHOLD: usize = 512 * 1024;

impl CoreManager {
    pub(super) fn load_files(&mut self) -> Result<(), CoreError> {
        // TODO: Sum of size of files to load (rather than doing it file-by-file).
        let mut overall_transferred = 0u64;
        let mut overall_total = 0u64;
        let mut last_progress_update = Instant::now();

        let mut scratch = crate::bitstream::SCRATCH.take().expect("scratch buffer");

        let core = self.core_info.as_ref().unwrap();
        let file_0_index = core.files.iter().position(|f| f.id == 0);
        for (i, info) in core.files.iter().enumerate() {
            if self.run_cartridge && (info.id == 0 || info.dependent_on_0) {
                continue;
            }

            // Get the file path
            let mut path = self.selected_files[i].clone();

            if let Some(filename) = info.filename.as_ref() {
                // Construct path based on core directory
                path = Some(core.core_dir.join(filename));
            } else if info.dependent_on_0 {
                // Construct a new path based on file 0's path
                let extension = info.extensions[0].as_str();
                let file_0_index = file_0_index.unwrap();
                path = self.selected_files[file_0_index]
                    .as_ref()
                    .map(|p| p.with_extension(extension));
            }

            // Possibly override the path
            if let Some(core_handler) = self.core_handler.get_mut() {
                path = path.or(core_handler.get_file_path_override(info.id));
            }

            let path = match path {
                Some(path) => path,
                // Use an empty path. Open will fail and we'll clear it.
                None if info.optional => PathBuf::new(),
                // It shouldn't be possible to have no path for a required file.
                None => panic!("Missing path for required file {}", info.label),
            };

            log::info!("Load file {} from {}", info.label, path.display());

            let file = File::open(&path);
            self.selected_files[i] = Some(path);
            let mut file = match file {
                Ok(file) => file,
                Err(_) if info.optional && info.initialize => {
                    log::info!("Failed to open file, clearing");
                    self.clear_file_slot(info, &mut scratch)?;
                    continue;
                }
                Err(_) if info.optional => {
                    log::info!("Failed to open file, skipping");
                    continue;
                }
                Err(_) => {
                    return Err(CannotOpenFile(info.label.to_string()));
                }
            };

            self.run_core_command(
                &[command::FILE_WRITE_START, info.id as u32],
                &mut [],
                SETUP_TIMEOUT,
            )?;

            if let Some(core_handler) = self.core_handler.get_mut() {
                core_handler
                    .on_before_file_load(info.id, &mut file)
                    .map_err(|e| FailedLoadFile(info.label.to_string(), e))?;
            }
            let file_size = file.metadata().unwrap().len();
            overall_total += file_size;

            if info.exact_size != 0 && file_size != (info.exact_size as u64) {
                return Err(FileWrongSize(
                    info.label.to_string(),
                    info.exact_size,
                    file_size as u32,
                ));
            }
            if info.max_size != 0 && file_size > (info.max_size as u64) {
                return Err(FileTooBig(
                    info.label.to_string(),
                    info.max_size,
                    file_size as u32,
                ));
            }

            let start_time = Instant::now();
            let mut transfer_duration = Duration::ZERO;
            let mut handler_duration = Duration::ZERO;
            let mut transferred = 0;
            let mut on_chunk_read = |chunk: &[u8]| {
                let transfer_start = Instant::now();
                let max_clock = Some(Hertz(info.max_transfer_speed * 1000 * 2));
                let command = SpiCommand::new(info.transfer_word_size);
                Device::lock()
                    .fpga
                    .spi_write(max_clock, command, info.address + transferred, chunk)
                    .unwrap();
                transferred += chunk.len() as u32;
                overall_transferred += chunk.len() as u64;
                transfer_duration += transfer_start.elapsed();

                let handler_start = Instant::now();

                if let Some(core_handler) = self.core_handler.get_mut() {
                    core_handler.on_during_file_load(info.id, chunk);
                }
                handler_duration += handler_start.elapsed();

                // Update UI progress bar.
                if last_progress_update.elapsed() > PROGRESS_UPDATE_INTERVAL {
                    let progress = (overall_transferred as f32) / (overall_total as f32);
                    ui::send(ui::Message::CoreLoadProgress(progress));
                    last_progress_update = Instant::now();
                }
            };

            // Only do background I/O for large files.
            let result = if file_size >= BG_IO_THRESHOLD as u64 {
                crate::util::background_io::iter_chunks(file, &mut scratch, on_chunk_read)
            } else {
                let mut duration = Duration::ZERO;
                loop {
                    let read_start = Instant::now();
                    let result = file.read(&mut scratch);
                    duration += read_start.elapsed();

                    match result {
                        Ok(0) => break Ok(duration),
                        Ok(n) => on_chunk_read(&scratch[..n]),
                        Err(err) => break Err(err),
                    }
                }
            };

            let read_duration = result
                .map_err(|_| FailedLoadFile(info.label.to_string(), "I/O error".to_string()))?;

            let duration = start_time.elapsed();
            self.run_core_command(
                &[
                    // Command
                    command::FILE_WRITE_END,
                    // Arg 1: file ID
                    info.id as u32,
                    // Arg 2: file size (bytes)
                    transferred,
                    // Arg 3: reserved (0)
                    0,
                ],
                &mut [],
                NOTIFY_TIMEOUT,
            )?;
            if let Some(core_handler) = self.core_handler.get_mut() {
                core_handler.on_after_file_load(info.id);
            }

            log::info!(
                "Loaded {} bytes in {} ms ({}/{}/{} ms read/transfer/handler)",
                transferred,
                duration.as_millis(),
                read_duration.as_millis(),
                transfer_duration.as_millis(),
                handler_duration.as_millis(),
            );
        }

        Ok(())
    }

    pub(super) fn persist_files(&mut self) -> Result<(), CoreError> {
        assert!(self.stage == Stage::Running);
        let core = self.core_info.as_ref().unwrap();
        for (i, info) in core.files.iter().enumerate() {
            if info.read_only {
                continue;
            }
            if self.run_cartridge && (info.id == 0 || info.dependent_on_0) {
                continue;
            }

            let path = self.selected_files[i].clone().unwrap();
            log::info!("Saving file {} to {}", info.label, path.display());

            let mut file_size = 0u32;
            self.run_core_command(
                &[command::FILE_READ_START, info.id as u32],
                std::slice::from_mut(&mut file_size),
                SETUP_TIMEOUT,
            )?;
            if let Some(override_size) = self
                .core_handler
                .get_mut()
                .and_then(|x| x.get_file_size(info.id))
            {
                file_size = override_size;
            }

            let mut file = File::create(path)
                .map_err(|_| FailedSaveFile(info.label.to_string(), "Open failed".to_string()))?;
            let mut scratch = crate::bitstream::SCRATCH.take().expect("scratch buffer");
            let buf = scratch.deref_mut();
            let mut address: u32 = info.address;
            let mut bytes_left = file_size as usize;

            let start_time = Instant::now();
            while bytes_left > 0 {
                let to_read = bytes_left.min(buf.len());
                let data = &mut buf[0..to_read];

                let max_clock =
                    Some(Hertz(info.max_transfer_speed * 1000 * 2).min(MAX_SPI_READ_CLOCK));
                let command = SpiCommand::new(info.transfer_word_size);
                let _ = Device::lock()
                    .fpga
                    .spi_read(max_clock, command, address, data);
                file.write(data).map_err(|_| {
                    FailedSaveFile(info.label.to_string(), "Write failed".to_string())
                })?;
                address += to_read as u32;
                bytes_left -= to_read;
            }
            log::info!(
                "Saved {} bytes in {}ms",
                file_size,
                start_time.elapsed().as_millis() as u32
            );

            self.run_core_command(
                &[command::FILE_READ_END, info.id as u32],
                &mut [],
                NOTIFY_TIMEOUT,
            )?;
            if let Some(core_handler) = self.core_handler.get_mut() {
                core_handler
                    .on_after_file_save(info.id, &mut file)
                    .map_err(|e| FailedSaveFile(info.label.to_string(), e))?;
            }
        }
        Ok(())
    }

    /// Called to persist core settings to the core settings JSON file
    pub fn persist_settings(&mut self) -> Result<(), std::io::Error> {
        assert!(self.stage == Stage::Running);
        let core = self.core_info.as_ref().unwrap();
        let settings = self.core_settings.as_mut().unwrap();
        if core.is_built_in {
            // Built-in cores don't use these settings, avoid confusion by not saving them.
            settings.settings.clear();
        }

        if !std::fs::exists(DIR_SETTINGS)? {
            std::fs::create_dir(DIR_SETTINGS)?;
        }
        let file = File::create(core.get_settings_path())?;
        let writer = BufWriter::with_capacity(256, file);
        serde_json::to_writer(writer, settings).map_err(std::io::Error::other)
    }

    /// Clear a file slot to 0xFF
    pub(super) fn clear_file_slot(&self, info: &CoreFile, buf: &mut [u8]) -> Result<(), CoreError> {
        self.run_core_command(
            &[command::FILE_WRITE_START, info.id as u32],
            &mut [],
            SETUP_TIMEOUT,
        )?;

        buf.fill(0xFF);
        let mut pos = 0u32;
        let len = info.max_size.max(info.exact_size);
        while pos < len {
            let n = ((len - pos) as usize).min(buf.len());
            let max_clock = Some(Hertz(info.max_transfer_speed * 1000 * 2));
            let command = SpiCommand::new(info.transfer_word_size);
            let _ =
                Device::lock()
                    .fpga
                    .spi_write(max_clock, command, info.address + pos, &buf[..n]);
            pos += n as u32;
        }

        self.run_core_command(
            &[
                // Command
                command::FILE_WRITE_END,
                // Arg 1: file ID
                info.id as u32,
                // Arg 2: file size (bytes)
                len,
                // Arg 3: reserved (0)
                0,
            ],
            &mut [],
            NOTIFY_TIMEOUT,
        )?;

        Ok(())
    }
}
