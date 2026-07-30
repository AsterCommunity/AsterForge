//! Owned File Provider enumeration requests and backend page validation.

use std::collections::HashSet;

use aster_forge_cloud_files_core::{CloudItemKey, CloudItemPage, PageCursor};

use crate::{
    MacosBridgeError, MacosFileProviderIdentifier, MacosFileProviderItem,
    MacosFileProviderSystemContainer, Result,
};

/// Maximum number of items accepted from one backend page.
pub const MAX_ENUMERATION_PAGE_ITEMS: usize = 4_096;
/// Maximum number of items retained for cross-page duplicate detection by one enumerator.
pub const MAX_ENUMERATION_ITEMS: usize = 100_000;
/// Maximum cumulative UTF-8 bytes retained for identifiers, filenames, and cursors.
pub const MAX_ENUMERATION_STATE_BYTES: usize = 16 * 1024 * 1024;

/// One File Provider enumeration request with a backend page cursor kept opaque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosEnumerationRequest {
    container: MacosFileProviderIdentifier,
    page: Option<PageCursor>,
}

impl MacosEnumerationRequest {
    /// Creates a request for a product directory or the root system container.
    pub const fn new(container: MacosFileProviderIdentifier, page: Option<PageCursor>) -> Self {
        Self { container, page }
    }

    /// Returns the native-facing container identifier.
    pub const fn container(&self) -> &MacosFileProviderIdentifier {
        &self.container
    }

    /// Returns the opaque backend page cursor.
    pub const fn page(&self) -> Option<&PageCursor> {
        self.page.as_ref()
    }

    /// Resolves the backend parent key, using `root_key` only for Apple's root container.
    pub fn backend_parent<'a>(&'a self, root_key: &'a CloudItemKey) -> Result<&'a CloudItemKey> {
        match &self.container {
            MacosFileProviderIdentifier::Item { key, .. } => Ok(key),
            MacosFileProviderIdentifier::System(MacosFileProviderSystemContainer::Root) => {
                Ok(root_key)
            }
            MacosFileProviderIdentifier::System(
                MacosFileProviderSystemContainer::WorkingSet
                | MacosFileProviderSystemContainer::Trash,
            ) => Err(MacosBridgeError::UnsupportedSystemContainer),
        }
    }
}

/// Owned, validated page returned to the Swift enumerator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosEnumerationPage {
    items: Vec<MacosFileProviderItem>,
    next_page: Option<PageCursor>,
}

/// Enumerator-scoped state that rejects duplicate items and repeated continuation cursors across
/// multiple backend pages.
#[derive(Debug, Clone)]
pub struct MacosEnumerationState {
    container: MacosFileProviderIdentifier,
    next_page: Option<PageCursor>,
    seen_identifiers: HashSet<String>,
    seen_filenames: HashSet<String>,
    seen_cursors: HashSet<PageCursor>,
    retained_bytes: usize,
    finished: bool,
}

impl MacosEnumerationState {
    /// Creates a fresh enumerator for one immutable container identity.
    pub fn new(container: MacosFileProviderIdentifier) -> Self {
        Self {
            container,
            next_page: None,
            seen_identifiers: HashSet::new(),
            seen_filenames: HashSet::new(),
            seen_cursors: HashSet::new(),
            retained_bytes: 0,
            finished: false,
        }
    }

    /// Returns the next backend request. Calling after the terminal page is a contract error.
    pub fn request(&self) -> Result<MacosEnumerationRequest> {
        if self.finished {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "enumeration requested another page after terminal completion",
            });
        }
        Ok(MacosEnumerationRequest::new(
            self.container.clone(),
            self.next_page.clone(),
        ))
    }

    /// Accepts one page and advances the handle-scoped continuation state.
    pub fn accept_page(&mut self, page: MacosEnumerationPage) -> Result<MacosEnumerationPage> {
        if self.finished {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "enumeration returned a page after terminal completion",
            });
        }
        let new_item_count = self
            .seen_identifiers
            .len()
            .checked_add(page.items().len())
            .ok_or(MacosBridgeError::InvalidBackendResponse {
                reason: "enumeration item count overflowed",
            })?;
        if new_item_count > MAX_ENUMERATION_ITEMS {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "enumeration exceeded the retained item limit",
            });
        }
        let mut added_bytes = 0usize;
        for item in page.items() {
            if self.seen_identifiers.contains(item.identifier().as_str()) {
                return Err(MacosBridgeError::InvalidBackendResponse {
                    reason: "enumeration repeated a persistent identifier across pages",
                });
            }
            if self.seen_filenames.contains(item.filename()) {
                return Err(MacosBridgeError::InvalidBackendResponse {
                    reason: "enumeration repeated a filename across pages",
                });
            }
            added_bytes = added_bytes
                .checked_add(item.identifier().as_str().len())
                .and_then(|value| value.checked_add(item.filename().len()))
                .ok_or(MacosBridgeError::InvalidBackendResponse {
                    reason: "enumeration retained byte count overflowed",
                })?;
        }
        let next_page = match page.next_page() {
            Some(cursor) => {
                if self.seen_cursors.contains(cursor) {
                    return Err(MacosBridgeError::InvalidBackendResponse {
                        reason: "enumeration repeated a continuation cursor",
                    });
                }
                added_bytes = added_bytes.checked_add(cursor.as_bytes().len()).ok_or(
                    MacosBridgeError::InvalidBackendResponse {
                        reason: "enumeration retained byte count overflowed",
                    },
                )?;
                Some(cursor.clone())
            }
            None => None,
        };
        let retained_bytes = self.retained_bytes.checked_add(added_bytes).ok_or(
            MacosBridgeError::InvalidBackendResponse {
                reason: "enumeration retained byte count overflowed",
            },
        )?;
        if retained_bytes > MAX_ENUMERATION_STATE_BYTES {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "enumeration exceeded the retained byte limit",
            });
        }
        self.seen_identifiers.extend(
            page.items()
                .iter()
                .map(|item| item.identifier().as_str().to_owned()),
        );
        self.seen_filenames
            .extend(page.items().iter().map(|item| item.filename().to_owned()));
        if let Some(cursor) = &next_page {
            self.seen_cursors.insert(cursor.clone());
        } else {
            self.finished = true;
        }
        self.next_page = next_page;
        self.retained_bytes = retained_bytes;
        Ok(page)
    }

    /// Returns whether a terminal page has been accepted.
    pub const fn is_finished(&self) -> bool {
        self.finished
    }
}

impl MacosEnumerationPage {
    /// Validates a backend page against the exact domain root and requested parent before exposing
    /// it to File Provider.
    pub fn from_backend(
        root_key: &CloudItemKey,
        parent: &CloudItemKey,
        page: CloudItemPage,
    ) -> Result<Self> {
        let (items, next_page) = page.into_parts();
        if items.len() > MAX_ENUMERATION_PAGE_ITEMS {
            return Err(MacosBridgeError::InvalidBackendResponse {
                reason: "enumeration page exceeded the item limit",
            });
        }
        let mut identifiers = HashSet::new();
        let mut filenames = HashSet::new();
        let mut converted = Vec::with_capacity(items.len());
        for item in items {
            if item.key().scope() != parent.scope() || item.parent_id() != Some(parent.item_id()) {
                return Err(MacosBridgeError::InvalidBackendResponse {
                    reason: "enumerated item escaped the requested parent scope",
                });
            }
            let converted_item = MacosFileProviderItem::from_cloud_item(&item, root_key)?;
            if !identifiers.insert(converted_item.identifier().as_str().to_owned()) {
                return Err(MacosBridgeError::InvalidBackendResponse {
                    reason: "enumeration returned duplicate persistent identifiers",
                });
            }
            if !filenames.insert(converted_item.filename().to_owned()) {
                return Err(MacosBridgeError::InvalidBackendResponse {
                    reason: "enumeration returned duplicate filenames",
                });
            }
            converted.push(converted_item);
        }
        Ok(Self {
            items: converted,
            next_page,
        })
    }

    /// Returns items in backend enumeration order.
    pub fn items(&self) -> &[MacosFileProviderItem] {
        &self.items
    }

    /// Returns the next opaque page cursor.
    pub const fn next_page(&self) -> Option<&PageCursor> {
        self.next_page.as_ref()
    }

    /// Consumes the page into owned items and continuation cursor.
    pub fn into_parts(self) -> (Vec<MacosFileProviderItem>, Option<PageCursor>) {
        (self.items, self.next_page)
    }
}
