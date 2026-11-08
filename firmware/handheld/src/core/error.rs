use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Cannot open file {0}")]
    CannotOpenFile(String),
    #[error("Failed to load file {0}:\n{1}")]
    FailedLoadFile(String, String),
    #[error("Failed to save file {0}:\n{1}")]
    FailedSaveFile(String, String),
    #[error("File {0} wrong size:\nExpected {1} bytes\nActually {2} bytes")]
    FileWrongSize(String, u32, u32),
    #[error("File {0} too big:\nMaximum {1} bytes\nActually {2} bytes")]
    FileTooBig(String, u32, u32),
    #[error("Core command {0:04X} timed out")]
    CommandTimeout(u32),
    #[error("Core command {0:04X} failed")]
    CommandFailed(u32),
    #[error("Timed out waiting for core to be ready")]
    CoreReadyTimeout,
    #[error("{0}")]
    Other(String),
}
