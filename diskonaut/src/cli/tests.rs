use ::std::path::PathBuf;

use clap::{CommandFactory, Parser};

use super::Opt;
use crate::error::Error;

#[test]
fn parses_defaults() {
    let opt = Opt::parse_from(["diskonaut"]);
    assert_eq!(
        opt,
        Opt {
            folder: None,
            apparent_size: false,
            config: None,
        }
    );
}

#[test]
fn parses_apparent_size_and_folder() {
    let opt = Opt::parse_from(["diskonaut", "-a", "/tmp"]);
    assert_eq!(
        opt,
        Opt {
            folder: Some(PathBuf::from("/tmp")),
            apparent_size: true,
            config: None,
        }
    );
}

#[test]
fn parses_long_flags() {
    let opt = Opt::parse_from(["diskonaut", "--apparent-size", "/var"]);
    assert!(opt.apparent_size);
    assert_eq!(opt.folder, Some(PathBuf::from("/var")));
}

#[test]
fn resolve_folder_errors_for_missing_path() {
    let opt = Opt {
        folder: Some(PathBuf::from("/nonexistent_diskonaut_test_path_9f3c2a")),
        apparent_size: false,
        config: None,
    };
    let err = opt.resolve_folder().unwrap_err();
    assert!(matches!(err, Error::FolderNotFound(_)));
}

#[test]
fn cli_definition_is_valid() {
    Opt::command().debug_assert();
}
