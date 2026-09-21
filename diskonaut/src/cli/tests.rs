use ::std::path::PathBuf;

use clap::{CommandFactory, Parser};

use super::Opt;
use crate::error::Error;

#[test]
fn parses_defaults() {
    let opt = Opt::parse_from(["diskonaut"]);
    assert_eq!(opt.folder, None);
    assert!(!opt.apparent_size);
    assert_eq!(opt.config, None);
    assert!(!opt.benchmark);
    assert_eq!(opt.max_depth, None);
    assert_eq!(opt.threads, None);
}

#[test]
fn parses_apparent_size_and_folder() {
    let opt = Opt::parse_from(["diskonaut", "-a", "/tmp"]);
    assert_eq!(opt.folder, Some(PathBuf::from("/tmp")));
    assert!(opt.apparent_size);
    assert_eq!(opt.config, None);
}

#[test]
fn parses_long_flags() {
    let opt = Opt::parse_from(["diskonaut", "--apparent-size", "/var"]);
    assert!(opt.apparent_size);
    assert_eq!(opt.folder, Some(PathBuf::from("/var")));
}

#[test]
fn resolve_folder_errors_for_missing_path() {
    let opt = Opt::parse_from(["diskonaut", "/nonexistent_diskonaut_test_path_9f3c2a"]);
    let err = opt.resolve_folder().unwrap_err();
    assert!(matches!(err, Error::FolderNotFound(_)));
}

#[test]
fn cli_definition_is_valid() {
    Opt::command().debug_assert();
}
