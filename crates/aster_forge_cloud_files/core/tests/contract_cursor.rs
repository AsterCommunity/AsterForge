use std::any::TypeId;

use aster_forge_cloud_files_core::{ChangeCursor, DirectoryCookie, PageCursor};

#[test]
fn pagination_change_tracking_and_directory_streams_use_distinct_types() {
    assert_ne!(TypeId::of::<PageCursor>(), TypeId::of::<ChangeCursor>());
    assert_ne!(TypeId::of::<PageCursor>(), TypeId::of::<DirectoryCookie>());
    assert_ne!(
        TypeId::of::<ChangeCursor>(),
        TypeId::of::<DirectoryCookie>()
    );
}

#[test]
fn opaque_cursor_bytes_are_preserved_exactly() {
    let bytes = vec![0, 0xff, b'/', b'?', 0];

    let page = PageCursor::new(bytes.clone()).expect("page cursor fixture should be valid");
    let changes = ChangeCursor::new(bytes.clone()).expect("change cursor fixture should be valid");
    let directory =
        DirectoryCookie::new(bytes.clone()).expect("directory cookie fixture should be valid");

    assert_eq!(page.as_bytes(), bytes);
    assert_eq!(changes.as_bytes(), bytes);
    assert_eq!(directory.as_bytes(), bytes);
}

#[test]
fn initial_positions_are_represented_by_none_instead_of_empty_tokens() {
    assert!(PageCursor::new(Vec::new()).is_err());
    assert!(ChangeCursor::new(Vec::new()).is_err());
    assert!(DirectoryCookie::new(Vec::new()).is_err());

    let initial_page: Option<PageCursor> = None;
    let initial_change_position: Option<ChangeCursor> = None;
    let initial_directory_position: Option<DirectoryCookie> = None;

    assert!(initial_page.is_none());
    assert!(initial_change_position.is_none());
    assert!(initial_directory_position.is_none());
}
