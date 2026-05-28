use super::{DisplaySize, truncate_middle};

#[test]
fn truncate_middle_char_boundary() {
    assert_eq!(
        truncate_middle("굿걸 - 누가 방송국을 털었나 E06.mp4", 44),
        "굿걸 - 누가 방송국을[...]국을 털었나 E06.mp4",
    );
}

#[test]
fn display_size_formats_kilobytes() {
    assert_eq!(format!("{}", DisplaySize(2048.0)), "2.0K");
}
