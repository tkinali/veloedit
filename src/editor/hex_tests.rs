use std::{fs, io::Write, path::PathBuf};

use super::*;

fn doc(bytes: &[u8], tag: &str) -> (HexDoc, PathBuf) {
    let path = std::env::temp_dir().join(format!("veloedit-hex-{}-{tag}", std::process::id()));
    fs::File::create(&path).unwrap().write_all(bytes).unwrap();
    (HexDoc::open(&path).unwrap(), path)
}

fn digits(d: &mut HexDoc, s: &str) {
    for c in s.chars() {
        d.apply(HexAction::Digit(c.to_digit(16).unwrap() as u8));
    }
}

#[test]
fn typing_hex_digits_overwrites_nibble_by_nibble_and_advances() {
    let (mut d, path) = doc(&[0x00, 0x11, 0x22, 0x33], "digits");
    digits(&mut d, "ab");
    assert_eq!(d.byte(0), Some(0xAB));
    assert_eq!(d.cursor(), 1, "moves on after the second nibble");
    assert!(d.dirty() && d.is_edited(0));
    digits(&mut d, "f");
    assert_eq!(d.byte(1), Some(0xF1), "the high nibble is replaced, the low one kept");
    assert!(!d.high_next(), "waiting for the low nibble");
    digits(&mut d, "e");
    assert_eq!((d.byte(1), d.cursor()), (Some(0xFE), 2));
    // Moving the cursor resets the nibble state.
    digits(&mut d, "9");
    d.apply(HexAction::Move(Move::Left));
    assert!(d.high_next());
    // The last byte: the cursor stays on it.
    d.apply(HexAction::Move(Move::End));
    digits(&mut d, "77");
    assert_eq!((d.byte(3), d.cursor()), (Some(0x77), 3));
    assert_eq!(d.edits().len(), 4);
    fs::remove_file(path).ok();
}

#[test]
fn writing_the_original_value_removes_the_edit_and_undo_redo_are_exact() {
    let (mut d, path) = doc(&[0x10, 0x20, 0x30], "undo");
    digits(&mut d, "ff"); // byte 0 -> FF, cursor 1
    digits(&mut d, "11"); // byte 1 -> 11
    assert_eq!(d.edits().len(), 2);
    d.apply(HexAction::Undo);
    assert_eq!((d.byte(1), d.cursor()), (Some(0x20), 1), "undo restores the byte and the cursor");
    d.apply(HexAction::Undo);
    assert_eq!((d.byte(0), d.cursor(), d.dirty()), (Some(0x10), 0, false));
    assert!(!d.apply(HexAction::Undo), "nothing left to undo");
    d.apply(HexAction::Redo);
    d.apply(HexAction::Redo);
    assert_eq!((d.byte(0), d.byte(1)), (Some(0xFF), Some(0x11)));

    // Typing the original value back (high then low nibble) clears the overlay.
    let (mut d2, p2) = doc(&[0x10], "same");
    digits(&mut d2, "ff");
    d2.apply(HexAction::Move(Move::Left));
    digits(&mut d2, "10");
    assert!(!d2.dirty(), "equal to the file -> not an edit");
    fs::remove_file(path).ok();
    fs::remove_file(p2).ok();
}

#[test]
fn ascii_area_types_characters_and_tab_switches_areas() {
    let (mut d, path) = doc(b"abcd", "ascii");
    d.apply(HexAction::ToggleArea);
    assert_eq!(d.area(), Area::Ascii);
    d.apply(HexAction::Byte(b'X'));
    d.apply(HexAction::Byte(b'Y'));
    assert_eq!((d.byte(0), d.byte(1), d.cursor()), (Some(b'X'), Some(b'Y'), 2));
    d.apply(HexAction::Click { offset: 3, area: Area::Hex });
    assert_eq!((d.cursor(), d.area()), (3, Area::Hex));
    d.apply(HexAction::Click { offset: 99, area: Area::Hex });
    assert_eq!(d.cursor(), 3, "clicks past the end are ignored");
    fs::remove_file(path).ok();
}

#[test]
fn save_patches_the_file_in_place_without_changing_its_size() {
    let (mut d, path) = doc(&(0..=255u8).collect::<Vec<_>>(), "save");
    d.apply(HexAction::GoTo(10));
    digits(&mut d, "aabb"); // bytes 10, 11
    d.apply(HexAction::GoTo(200));
    digits(&mut d, "01");
    let saved: Vec<(u64, u8)> = d.edits().iter().map(|(&o, &b)| (o, b)).collect();
    assert_eq!(saved, vec![(10, 0xAA), (11, 0xBB), (200, 0x01)]);
    write_patches(&path, d.edits()).unwrap();
    d.finish_save(&saved);
    assert!(!d.dirty());
    assert!(!d.apply(HexAction::Undo), "history starts over after a save");

    let disk = fs::read(&path).unwrap();
    assert_eq!(disk.len(), 256);
    assert_eq!((disk[9], disk[10], disk[11], disk[12], disk[200], disk[201]), (9, 0xAA, 0xBB, 12, 0x01, 201));
    assert_eq!(d.byte(10), Some(0xAA), "the view re-reads the patched file");

    // An edit made *while* the save ran survives it.
    d.apply(HexAction::GoTo(0));
    digits(&mut d, "55");
    let first: Vec<(u64, u8)> = vec![(0, 0x55)];
    digits(&mut d, "66"); // byte 1
    d.finish_save(&first);
    assert_eq!(d.edits().len(), 1);
    assert!(d.is_edited(1) && !d.is_edited(0));
    fs::remove_file(path).ok();
}

#[test]
fn cursor_moves_clamp_and_scrolling_follows() {
    let (mut d, path) = doc(&vec![7u8; 16 * 100 + 5], "moves");
    d.set_view_rows(10);
    d.apply(HexAction::Move(Move::Down));
    assert_eq!(d.cursor(), 16);
    d.apply(HexAction::Move(Move::PageDown));
    assert_eq!(d.cursor(), 16 + 9 * 16, "a page is the view minus one row");
    assert!(d.top_row() > 0, "the cursor row stays visible");
    d.apply(HexAction::Move(Move::End));
    assert_eq!(d.cursor(), 16 * 100 + 4);
    d.apply(HexAction::Move(Move::Right));
    assert_eq!(d.cursor(), 16 * 100 + 4, "clamped at the last byte");
    d.apply(HexAction::Move(Move::Start));
    assert_eq!((d.cursor(), d.top_row()), (0, 0));
    d.apply(HexAction::Move(Move::RowEnd));
    assert_eq!(d.cursor(), 15);
    d.apply(HexAction::Scroll { rows: -50.0 });
    assert_eq!(d.top_row(), 0);
    d.apply(HexAction::Scroll { rows: 1e9 });
    assert!(d.top_row() < d.rows, "scroll is clamped to the file");
    fs::remove_file(path).ok();
}

#[test]
fn empty_and_tiny_files_are_safe() {
    let (mut d, path) = doc(&[], "empty");
    for a in [HexAction::Digit(1), HexAction::Byte(b'a'), HexAction::Move(Move::Down), HexAction::Undo,
        HexAction::GoTo(5), HexAction::Click { offset: 0, area: Area::Hex }, HexAction::Scroll { rows: 3.0 }] {
        d.apply(a);
    }
    assert!(!d.dirty() && d.byte(0).is_none());
    fs::remove_file(path).ok();
}

#[test]
fn a_gigabyte_file_is_windowed_and_goto_is_exact() {
    let path = std::env::temp_dir().join(format!("veloedit-hex-{}-gig", std::process::id()));
    let f = fs::File::create(&path).unwrap();
    f.set_len(1 << 30).unwrap();
    f.write_all_at(b"ABCDEFGHIJKLMNOP", 1 << 29).unwrap();
    let mut d = HexDoc::open(&path).unwrap();
    d.set_view_rows(30);
    assert_eq!((d.len, d.rows), (1 << 30, 1 << 26));
    d.apply(HexAction::GoTo(1 << 29));
    assert_eq!(d.cursor(), 1 << 29);
    assert_eq!(d.byte(1 << 29), Some(b'A'));
    assert_eq!(d.byte((1 << 29) + 15), Some(b'P'));
    assert!(d.top_row() <= (1 << 25) && d.top_row() + 30 >= (1 << 25), "cursor row is in view");
    // Far from the window nothing is loaded (no hidden full read).
    assert_eq!(d.byte(5), None);
    // Editing far away works through a one-byte read of the original.
    d.apply(HexAction::GoTo(900_000_000));
    digits(&mut d, "00"); // equal to the file's zero byte -> no edit
    assert!(!d.dirty());
    digits(&mut d, "7e");
    assert_eq!(d.edits().len(), 1);
    fs::remove_file(path).ok();
}

#[test]
fn restored_edits_outside_the_file_are_dropped() {
    let (mut d, path) = doc(&[1, 2, 3], "restore");
    d.set_edits([(1, 9), (50, 1)]);
    assert_eq!(d.edits().len(), 1);
    assert_eq!(d.byte(1), Some(9));
    fs::remove_file(path).ok();
}
