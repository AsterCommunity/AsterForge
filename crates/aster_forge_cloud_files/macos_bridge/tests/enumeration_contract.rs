use aster_forge_cloud_files_core::{
    CloudContentMetadata, CloudItem, CloudItemId, CloudItemKey, CloudItemPage, CloudNamespaceId,
    CloudRootId, CloudScope, ContentRevision, MetadataRevision, PageCursor,
};
use aster_forge_cloud_files_macos_bridge::{
    FILE_PROVIDER_CURRENT_WORKING_SET_IDENTIFIER, FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER,
    FILE_PROVIDER_TRASH_CONTAINER_IDENTIFIER, MAX_ENUMERATION_PAGE_ITEMS, MacosBridgeError,
    MacosEnumerationPage, MacosEnumerationRequest, MacosEnumerationState,
    MacosFileProviderIdentifier,
};

#[test]
fn request_resolves_only_item_and_root_containers_and_preserves_cursor() {
    let fixture = Fixture::new();
    let cursor = PageCursor::from_slice(b"opaque-page").expect("cursor should be valid");
    let item_identifier = MacosFileProviderIdentifier::encode(&fixture.docs_key)
        .expect("identifier fixture should fit");
    let request = MacosEnumerationRequest::new(item_identifier.clone(), Some(cursor.clone()));
    assert_eq!(request.container(), &item_identifier);
    assert_eq!(request.page(), Some(&cursor));
    assert_eq!(
        request
            .backend_parent(&fixture.root_key)
            .expect("item parent should resolve"),
        &fixture.docs_key
    );

    let root = parse(FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER);
    assert_eq!(
        MacosEnumerationRequest::new(root, None)
            .backend_parent(&fixture.root_key)
            .expect("root should resolve"),
        &fixture.root_key
    );

    for raw in [
        FILE_PROVIDER_CURRENT_WORKING_SET_IDENTIFIER,
        FILE_PROVIDER_TRASH_CONTAINER_IDENTIFIER,
    ] {
        assert!(matches!(
            MacosEnumerationRequest::new(parse(raw), None).backend_parent(&fixture.root_key),
            Err(MacosBridgeError::UnsupportedSystemContainer)
        ));
    }
}

#[test]
fn page_rejects_duplicate_identifiers_and_returns_owned_parts_in_order() {
    let fixture = Fixture::new();
    let duplicate_key = CloudItem::directory(
        fixture.docs_key.clone(),
        Some(fixture.root_key.item_id().clone()),
        "different-name",
        metadata(),
    )
    .expect("duplicate key fixture should remain a valid item");
    assert!(matches!(
        MacosEnumerationPage::from_backend(
            &fixture.root_key,
            &fixture.root_key,
            CloudItemPage::new(vec![fixture.docs.clone(), duplicate_key], None),
        ),
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumeration returned duplicate persistent identifiers"
        })
    ));

    let cursor = PageCursor::from_slice(b"next").expect("cursor should be valid");
    let page = MacosEnumerationPage::from_backend(
        &fixture.root_key,
        &fixture.root_key,
        CloudItemPage::new(
            vec![fixture.docs.clone(), fixture.file.clone()],
            Some(cursor.clone()),
        ),
    )
    .expect("unique page should convert");
    assert_eq!(page.items().len(), 2);
    assert_eq!(page.items()[0].filename(), "docs");
    assert_eq!(page.items()[1].filename(), "hello.txt");
    assert_eq!(page.next_page(), Some(&cursor));
    let (items, next) = page.into_parts();
    assert_eq!(items.len(), 2);
    assert_eq!(next, Some(cursor));
}

#[test]
fn page_rejects_scope_and_parent_escape_independently() {
    let fixture = Fixture::new();
    let other_scope = scope("enumeration", "other-root");
    let escaped_scope = CloudItem::directory(
        key(&other_scope, "escaped"),
        Some(fixture.root_key.item_id().clone()),
        "escaped",
        metadata(),
    )
    .expect("scope fixture should be valid");
    let escaped_parent = CloudItem::directory(
        key(&fixture.scope, "escaped-parent"),
        Some(fixture.docs_key.item_id().clone()),
        "escaped-parent",
        metadata(),
    )
    .expect("parent fixture should be valid");

    for item in [escaped_scope, escaped_parent] {
        assert!(matches!(
            MacosEnumerationPage::from_backend(
                &fixture.root_key,
                &fixture.root_key,
                CloudItemPage::new(vec![item], None),
            ),
            Err(MacosBridgeError::InvalidBackendResponse {
                reason: "enumerated item escaped the requested parent scope"
            })
        ));
    }
}

#[test]
fn failed_page_does_not_pollute_enumerator_state_and_terminal_state_is_final() {
    let fixture = Fixture::new();
    let root = parse(FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER);
    let cursor_a = PageCursor::from_slice(b"cursor-a").expect("cursor should be valid");
    let cursor_b = PageCursor::from_slice(b"cursor-b").expect("cursor should be valid");
    let mut state = MacosEnumerationState::new(root.clone());

    let first = page(&fixture, Vec::new(), Some(cursor_a.clone()));
    state.accept_page(first).expect("first page should advance");
    assert_eq!(
        state.request().expect("next request should exist").page(),
        Some(&cursor_a)
    );

    let repeated_cursor = page(&fixture, vec![fixture.docs.clone()], Some(cursor_a.clone()));
    assert!(matches!(
        state.accept_page(repeated_cursor),
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumeration repeated a continuation cursor"
        })
    ));
    assert_eq!(
        state
            .request()
            .expect("failed page must not advance")
            .page(),
        Some(&cursor_a)
    );

    let retry = page(&fixture, vec![fixture.docs.clone()], Some(cursor_b.clone()));
    state
        .accept_page(retry)
        .expect("same item should remain acceptable after failed page");
    assert_eq!(
        state.request().expect("third request should exist").page(),
        Some(&cursor_b)
    );

    state
        .accept_page(page(&fixture, vec![fixture.file.clone()], None))
        .expect("terminal page should finish");
    assert!(state.is_finished());
    assert!(matches!(
        state.accept_page(page(&fixture, Vec::new(), None)),
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumeration returned a page after terminal completion"
        })
    ));
    assert!(matches!(
        state.request(),
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumeration requested another page after terminal completion"
        })
    ));
}

#[test]
fn page_item_limit_accepts_boundary_and_rejects_boundary_plus_one() {
    let fixture = Fixture::new();
    let items = (0..MAX_ENUMERATION_PAGE_ITEMS)
        .map(|index| {
            CloudItem::directory(
                key(&fixture.scope, &format!("item-{index}")),
                Some(fixture.root_key.item_id().clone()),
                format!("item-{index}"),
                metadata(),
            )
            .expect("boundary item should be valid")
        })
        .collect::<Vec<_>>();
    MacosEnumerationPage::from_backend(
        &fixture.root_key,
        &fixture.root_key,
        CloudItemPage::new(items.clone(), None),
    )
    .expect("page at the item limit should convert");

    let mut oversized = items;
    oversized.push(
        CloudItem::directory(
            key(&fixture.scope, "overflow"),
            Some(fixture.root_key.item_id().clone()),
            "overflow",
            metadata(),
        )
        .expect("overflow fixture should be valid"),
    );
    assert!(matches!(
        MacosEnumerationPage::from_backend(
            &fixture.root_key,
            &fixture.root_key,
            CloudItemPage::new(oversized, None),
        ),
        Err(MacosBridgeError::InvalidBackendResponse {
            reason: "enumeration page exceeded the item limit"
        })
    ));
}

struct Fixture {
    scope: CloudScope,
    root_key: CloudItemKey,
    docs_key: CloudItemKey,
    docs: CloudItem,
    file: CloudItem,
}

impl Fixture {
    fn new() -> Self {
        let scope = scope("enumeration", "default");
        let root_key = key(&scope, "root");
        let docs_key = key(&scope, "docs");
        let docs = CloudItem::directory(
            docs_key.clone(),
            Some(root_key.item_id().clone()),
            "docs",
            metadata(),
        )
        .expect("directory should be valid");
        let file = CloudItem::file(
            key(&scope, "hello"),
            root_key.item_id().clone(),
            "hello.txt",
            metadata(),
            CloudContentMetadata::new(content(), None, 5),
        )
        .expect("file should be valid");
        Self {
            scope,
            root_key,
            docs_key,
            docs,
            file,
        }
    }
}

fn page(
    fixture: &Fixture,
    items: Vec<CloudItem>,
    next: Option<PageCursor>,
) -> MacosEnumerationPage {
    MacosEnumerationPage::from_backend(
        &fixture.root_key,
        &fixture.root_key,
        CloudItemPage::new(items, next),
    )
    .expect("page fixture should be valid")
}

fn parse(raw: &str) -> MacosFileProviderIdentifier {
    MacosFileProviderIdentifier::parse(raw).expect("system identifier should parse")
}

fn scope(namespace: &str, root: &str) -> CloudScope {
    CloudScope::new(
        CloudNamespaceId::new(namespace).expect("namespace should be valid"),
        CloudRootId::new(root).expect("root should be valid"),
    )
}

fn key(scope: &CloudScope, item: &str) -> CloudItemKey {
    CloudItemKey::new(
        scope.clone(),
        CloudItemId::new(item).expect("item should be valid"),
    )
}

fn metadata() -> MetadataRevision {
    MetadataRevision::from_slice(b"metadata").expect("metadata should be valid")
}

fn content() -> ContentRevision {
    ContentRevision::from_slice(b"content").expect("content should be valid")
}
