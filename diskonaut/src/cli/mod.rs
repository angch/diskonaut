use ::std::path::PathBuf;

use clap::Parser;

use crate::error::Error;

/// Command-line options for `diskonaut`.
#[derive(Parser, Debug, PartialEq, Eq)]
#[command(name = "diskonaut")]
pub struct Opt {
    /// The folder to scan
    pub folder: Option<PathBuf>,
    /// Show file sizes rather than their block usage on disk
    #[arg(short, long)]
    pub apparent_size: bool,
    /// Path to config file (default: `~/.config/diskonaut/config.toml`)
    #[arg(short = 'c', long, value_name = "FILE")]
    pub config: Option<PathBuf>,
}

impl Opt {
    /// Resolves the scan root: explicit `--folder` or the current working directory.
    pub fn resolve_folder(&self) -> Result<PathBuf, Error> {
        let folder = match &self.folder {
            Some(folder) => folder.clone(),
            None => std::env::current_dir()?,
        };
        if !folder.as_path().is_dir() {
            return Err(Error::FolderNotFound(folder.to_string_lossy().into_owned()));
        }
        Ok(folder)
    }
}

#[cfg(test)]
mod tests;
