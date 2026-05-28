use thiserror::Error;

/// Errors surfaced at the `diskonaut` binary boundary.
#[derive(Debug, Error)]
pub enum Error {
    #[error("Folder '{0}' does not exist")]
    FolderNotFound(String),

    #[error("Failed to get stdout: are you trying to pipe 'diskonaut'?")]
    NoStdout,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Terminal(#[from] crossterm::ErrorKind),
}
