use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::SystemTime;

use aster_forge_webdav::{
    DavBackendError, DavBackendErrorKind, DavCancellationToken, DavDirectoryEntry,
    DavDirectoryEnumerator, DavDirectoryPage, DavDirectoryPageLimits, DavDirectoryPageRequest,
    DavMetaData, DavMultiStatusLimits, DavMutationCommand, DavMutationExecutorLimits,
    DavMutationOperation, DavMutationPort, DavMutationRequest, DavMutationStepError,
    DavMutationStepKind, DavMutationStop, DavNeverCancelled, DavPath, DavResourceKind,
    DavResponseBody, DavTraversalLimits, FsError, execute_recursive_mutation,
    mutation_outcome_response,
};
use http::StatusCode;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Meta {
    collection: bool,
}

impl DavMetaData for Meta {
    fn len(&self) -> u64 {
        0
    }

    fn modified(&self) -> Result<SystemTime, FsError> {
        Ok(SystemTime::UNIX_EPOCH)
    }

    fn is_dir(&self) -> bool {
        self.collection
    }

    fn etag(&self) -> Option<String> {
        None
    }

    fn created(&self) -> Result<SystemTime, FsError> {
        Ok(SystemTime::UNIX_EPOCH)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: Vec<u8>,
    stable_key: Vec<u8>,
    meta: Meta,
}

impl Entry {
    fn file(name: &str) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            stable_key: name.as_bytes().to_vec(),
            meta: Meta { collection: false },
        }
    }

    fn collection(name: &str) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            stable_key: name.as_bytes().to_vec(),
            meta: Meta { collection: true },
        }
    }

    fn with_stable_key(mut self, stable_key: &[u8]) -> Self {
        self.stable_key = stable_key.to_vec();
        self
    }
}

impl DavDirectoryEntry for Entry {
    type Metadata = Meta;

    fn name(&self) -> &[u8] {
        &self.name
    }

    fn metadata(&self) -> &Self::Metadata {
        &self.meta
    }

    fn stable_key(&self) -> &[u8] {
        &self.stable_key
    }
}

#[derive(Debug, Default)]
struct Enumerator {
    entries: BTreeMap<String, Vec<Entry>>,
    failures: BTreeSet<String>,
}

impl Enumerator {
    fn with(mut self, path: &str, mut entries: Vec<Entry>) -> Self {
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        self.entries.insert(path.to_owned(), entries);
        self
    }

    fn failing(mut self, path: &str) -> Self {
        self.failures.insert(path.to_owned());
        self
    }
}

impl DavDirectoryEnumerator for Enumerator {
    type Cursor = usize;
    type Entry = Entry;

    async fn read_directory_page<'a>(
        &'a self,
        request: DavDirectoryPageRequest<'a, Self::Cursor>,
    ) -> Result<DavDirectoryPage<Self::Entry, Self::Cursor>, DavBackendError> {
        if self.failures.contains(request.path.as_str()) {
            return Err(DavBackendError::new(DavBackendErrorKind::Internal));
        }
        let entries = self
            .entries
            .get(request.path.as_str())
            .cloned()
            .unwrap_or_default();
        let start = request.cursor.copied().unwrap_or(0);
        let end = start
            .saturating_add(request.maximum_entries)
            .min(entries.len());
        Ok(DavDirectoryPage {
            entries: entries[start..end].to_vec(),
            next_cursor: (end < entries.len()).then_some(end),
        })
    }
}

#[test]
fn directory_backend_failure_prunes_copy_subtree_and_continues_sibling() {
    futures::executor::block_on(async {
        let enumerator = Enumerator::default()
            .with(
                "/source/",
                vec![Entry::collection("failed"), Entry::file("sibling.txt")],
            )
            .with("/destination/", vec![])
            .with("/destination/failed/", vec![])
            .failing("/source/failed/");
        let port = Port::default();

        let outcome = execute_recursive_mutation(
            request(DavMutationOperation::Copy),
            &enumerator,
            &port,
            &DavNeverCancelled,
            limits(16, 16, 4, 4, 8),
        )
        .await;

        assert_eq!(outcome.stop, None);
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].path().as_str(), "/source/failed/");
        let commands = port.commands();
        assert!(commands.iter().any(|command| {
            command.step == DavMutationStepKind::CopyFile
                && command.source.as_str() == "/source/sibling.txt"
        }));
        assert!(!commands.iter().any(|command| {
            command.step == DavMutationStepKind::CopyFile
                && command.source.as_str().starts_with("/source/failed/")
        }));
    });
}

#[test]
fn directory_backend_failure_prunes_delete_subtree_and_preserves_ancestors() {
    futures::executor::block_on(async {
        let enumerator = Enumerator::default()
            .with(
                "/source/",
                vec![Entry::collection("failed"), Entry::file("sibling.txt")],
            )
            .failing("/source/failed/");
        let port = Port::default();

        let outcome = execute_recursive_mutation(
            request(DavMutationOperation::Delete),
            &enumerator,
            &port,
            &DavNeverCancelled,
            limits(16, 16, 4, 4, 8),
        )
        .await;

        assert_eq!(outcome.stop, None);
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].path().as_str(), "/source/failed/");
        let commands = port.commands();
        assert!(commands.iter().any(|command| {
            command.step == DavMutationStepKind::DeleteFile
                && command.source.as_str() == "/source/sibling.txt"
        }));
        assert!(!commands.iter().any(|command| {
            command.step == DavMutationStepKind::DeleteCollection
                && matches!(command.source.as_str(), "/source/failed/" | "/source/")
        }));
    });
}

#[derive(Debug, Default)]
struct Port {
    commands: Mutex<Vec<DavMutationCommand>>,
    failures: BTreeMap<String, DavMutationStepError>,
    cancellation: Option<DavCancellationToken>,
    cancel_after: Option<usize>,
}

impl Port {
    fn failing(mut self, path: &str, error: DavMutationStepError) -> Self {
        self.failures.insert(path.to_owned(), error);
        self
    }

    fn cancelling(token: DavCancellationToken, cancel_after: usize) -> Self {
        Self {
            cancellation: Some(token),
            cancel_after: Some(cancel_after),
            ..Self::default()
        }
    }

    fn commands(&self) -> Vec<DavMutationCommand> {
        self.commands.lock().unwrap().clone()
    }
}

impl DavMutationPort for Port {
    async fn execute(&self, command: DavMutationCommand) -> Result<(), DavMutationStepError> {
        let affected = command.affected_path().as_str().to_owned();
        let mut commands = self.commands.lock().unwrap();
        commands.push(command);
        if self.cancel_after == Some(commands.len())
            && let Some(cancellation) = &self.cancellation
        {
            cancellation.cancel();
        }
        drop(commands);
        self.failures.get(&affected).cloned().map_or(Ok(()), Err)
    }
}

#[test]
fn copy_matches_source_and_destination_children_by_name_not_backend_cursor_key() {
    futures::executor::block_on(async {
        let enumerator = Enumerator::default()
            .with(
                "/source/",
                vec![Entry::collection("subcoll").with_stable_key(&[0, 1])],
            )
            .with(
                "/destination/",
                vec![Entry::collection("subcoll").with_stable_key(&[0, 9])],
            )
            .with("/source/subcoll/", vec![])
            .with("/destination/subcoll/", vec![]);
        let port = Port::default();

        let outcome = execute_recursive_mutation(
            request(DavMutationOperation::Copy),
            &enumerator,
            &port,
            &DavNeverCancelled,
            limits(16, 16, 4, 4, 8),
        )
        .await;

        assert!(outcome.is_complete_success());
        let commands = port.commands();
        assert!(commands.iter().any(|command| {
            command.step == DavMutationStepKind::PrepareCollection
                && command.source.as_str() == "/source/subcoll/"
                && command
                    .destination
                    .as_ref()
                    .is_some_and(|path| path.as_str() == "/destination/subcoll/")
        }));
        assert!(!commands.iter().any(|command| {
            command.step == DavMutationStepKind::DeleteCollection
                && command.source.as_str() == "/destination/subcoll/"
        }));
    });
}

fn limits(
    visited: usize,
    queued: usize,
    failures: usize,
    depth: usize,
    directory_entries: usize,
) -> DavMutationExecutorLimits {
    DavMutationExecutorLimits::new(
        DavTraversalLimits::new(visited, queued, failures, Some(depth)),
        DavDirectoryPageLimits::new(2, 16).unwrap(),
        2,
        directory_entries,
    )
}

fn limits_with_pages(
    visited: usize,
    queued: usize,
    failures: usize,
    depth: usize,
    pages: usize,
    page_entries: usize,
    directory_entries: usize,
) -> DavMutationExecutorLimits {
    DavMutationExecutorLimits::new(
        DavTraversalLimits::new(visited, queued, failures, Some(depth)),
        DavDirectoryPageLimits::new(page_entries, pages).unwrap(),
        page_entries,
        directory_entries,
    )
}

fn request(operation: DavMutationOperation) -> DavMutationRequest {
    DavMutationRequest {
        operation,
        source: DavPath::new("/source/").unwrap(),
        source_kind: DavResourceKind::Collection,
        destination: (operation != DavMutationOperation::Delete)
            .then(|| DavPath::new("/destination/").unwrap()),
        destination_kind: None,
        destination_existed: false,
        recurse_collections: true,
    }
}

#[test]
fn delete_prunes_failed_collection_and_continues_sibling() {
    futures::executor::block_on(async {
        let enumerator = Enumerator::default()
            .with(
                "/source/",
                vec![Entry::collection("failed"), Entry::file("sibling.txt")],
            )
            .with("/source/failed/", vec![Entry::file("locked.txt")]);
        let locked_path = DavPath::new("/source/failed/locked.txt").unwrap();
        let port = Port::default().failing(
            locked_path.as_str(),
            DavMutationStepError::locked(
                locked_path.clone(),
                DavPath::new("/source/failed/").unwrap(),
            ),
        );

        let outcome = execute_recursive_mutation(
            request(DavMutationOperation::Delete),
            &enumerator,
            &port,
            &DavNeverCancelled,
            limits(16, 16, 4, 4, 8),
        )
        .await;

        assert_eq!(outcome.failures.len(), 1);
        let commands = port.commands();
        assert!(commands.iter().any(|command| {
            command.step == DavMutationStepKind::DeleteFile
                && command.source.as_str() == "/source/sibling.txt"
        }));
        assert!(!commands.iter().any(|command| {
            command.step == DavMutationStepKind::DeleteCollection
                && matches!(command.source.as_str(), "/source/failed/" | "/source/")
        }));

        let response =
            mutation_outcome_response("/webdav", &outcome, DavMultiStatusLimits::default())
                .unwrap();
        assert_eq!(response.status, StatusCode::MULTI_STATUS);
        let DavResponseBody::Bytes(body) = response.body else {
            panic!("partial mutation should return a buffered test response");
        };
        let xml = String::from_utf8(body.to_vec()).unwrap();
        assert!(xml.contains("/webdav/source/failed/locked.txt"), "{xml}");
        assert!(xml.contains("/webdav/source/failed/"), "{xml}");
    });
}

#[test]
fn move_is_postorder_and_copy_never_deletes_source() {
    futures::executor::block_on(async {
        let enumerator = Enumerator::default()
            .with(
                "/source/",
                vec![Entry::collection("nested"), Entry::file("root.txt")],
            )
            .with("/source/nested/", vec![Entry::file("leaf.txt")])
            .with("/destination/", vec![])
            .with("/destination/nested/", vec![]);

        let move_port = Port::default();
        let move_outcome = execute_recursive_mutation(
            request(DavMutationOperation::Move),
            &enumerator,
            &move_port,
            &DavNeverCancelled,
            limits(16, 16, 4, 4, 8),
        )
        .await;
        assert!(move_outcome.is_complete_success());
        let commands = move_port.commands();
        let nested_delete = commands
            .iter()
            .position(|command| {
                command.step == DavMutationStepKind::DeleteCollection
                    && command.source.as_str() == "/source/nested/"
            })
            .unwrap();
        let root_delete = commands
            .iter()
            .position(|command| {
                command.step == DavMutationStepKind::DeleteCollection
                    && command.source.as_str() == "/source/"
            })
            .unwrap();
        assert!(nested_delete < root_delete);

        let copy_port = Port::default();
        let copy_outcome = execute_recursive_mutation(
            request(DavMutationOperation::Copy),
            &enumerator,
            &copy_port,
            &DavNeverCancelled,
            limits(16, 16, 4, 4, 8),
        )
        .await;
        assert!(copy_outcome.is_complete_success());
        assert!(
            copy_port
                .commands()
                .iter()
                .all(|command| command.step != DavMutationStepKind::DeleteCollection)
        );
    });
}

#[test]
fn directory_entry_limit_accepts_exact_and_stops_at_plus_one() {
    futures::executor::block_on(async {
        let exact =
            Enumerator::default().with("/source/", vec![Entry::file("a"), Entry::file("b")]);
        let exact_outcome = execute_recursive_mutation(
            request(DavMutationOperation::Delete),
            &exact,
            &Port::default(),
            &DavNeverCancelled,
            limits(8, 8, 2, 2, 2),
        )
        .await;
        assert!(exact_outcome.is_complete_success());

        let plus_one = Enumerator::default().with(
            "/source/",
            vec![Entry::file("a"), Entry::file("b"), Entry::file("c")],
        );
        let outcome = execute_recursive_mutation(
            request(DavMutationOperation::Delete),
            &plus_one,
            &Port::default(),
            &DavNeverCancelled,
            limits(8, 8, 2, 2, 2),
        )
        .await;
        assert_eq!(outcome.stop, Some(DavMutationStop::InsufficientStorage));
        assert_eq!(outcome.progress.completed_mutations, 0);
    });
}

#[test]
fn failure_and_cancellation_boundaries_keep_typed_partial_progress() {
    futures::executor::block_on(async {
        let enumerator =
            Enumerator::default().with("/source/", vec![Entry::file("a"), Entry::file("b")]);
        let port = Port::default()
            .failing(
                "/source/a",
                DavMutationStepError::backend(DavPath::new("/source/a").unwrap()),
            )
            .failing(
                "/source/b",
                DavMutationStepError::backend(DavPath::new("/source/b").unwrap()),
            );
        let failure_outcome = execute_recursive_mutation(
            request(DavMutationOperation::Delete),
            &enumerator,
            &port,
            &DavNeverCancelled,
            limits(8, 8, 1, 2, 2),
        )
        .await;
        assert_eq!(failure_outcome.failures.len(), 1);
        assert_eq!(
            failure_outcome.stop,
            Some(DavMutationStop::InsufficientStorage)
        );

        let cancellation = DavCancellationToken::new();
        let cancelling_port = Port::cancelling(cancellation.clone(), 1);
        let cancellation_outcome = execute_recursive_mutation(
            request(DavMutationOperation::Delete),
            &enumerator,
            &cancelling_port,
            &cancellation,
            limits(8, 8, 2, 2, 2),
        )
        .await;
        assert_eq!(cancellation_outcome.stop, Some(DavMutationStop::Cancelled));
        assert_eq!(cancellation_outcome.progress.completed_mutations, 1);
    });
}

#[test]
fn file_request_uses_one_authoritative_step_and_composes_success() {
    futures::executor::block_on(async {
        let port = Port::default();
        let outcome = execute_recursive_mutation(
            DavMutationRequest {
                operation: DavMutationOperation::Move,
                source: DavPath::new("/from.txt").unwrap(),
                source_kind: DavResourceKind::File,
                destination: Some(DavPath::new("/to.txt").unwrap()),
                destination_kind: Some(DavResourceKind::File),
                destination_existed: true,
                recurse_collections: false,
            },
            &Enumerator::default(),
            &port,
            &DavNeverCancelled,
            limits(1, 1, 1, 0, 1),
        )
        .await;
        assert!(outcome.is_complete_success());
        assert_eq!(port.commands().len(), 1);
        assert_eq!(
            mutation_outcome_response("/webdav", &outcome, DavMultiStatusLimits::default())
                .unwrap()
                .status,
            StatusCode::NO_CONTENT
        );
    });
}

#[test]
fn file_replacement_deletes_destination_collection_postorder_before_transfer() {
    futures::executor::block_on(async {
        let enumerator = Enumerator::default()
            .with("/to/", vec![Entry::collection("nested")])
            .with("/to/nested/", vec![Entry::file("old.txt")]);
        let port = Port::default();
        let outcome = execute_recursive_mutation(
            DavMutationRequest {
                operation: DavMutationOperation::Copy,
                source: DavPath::new("/from.txt").unwrap(),
                source_kind: DavResourceKind::File,
                destination: Some(DavPath::new("/to/").unwrap()),
                destination_kind: Some(DavResourceKind::Collection),
                destination_existed: true,
                recurse_collections: false,
            },
            &enumerator,
            &port,
            &DavNeverCancelled,
            limits(8, 8, 2, 3, 8),
        )
        .await;

        assert!(outcome.is_complete_success());
        let commands = port.commands();
        let leaf = commands
            .iter()
            .position(|command| command.source.as_str() == "/to/nested/old.txt")
            .unwrap();
        let nested = commands
            .iter()
            .position(|command| command.source.as_str() == "/to/nested/")
            .unwrap();
        let root = commands
            .iter()
            .position(|command| command.source.as_str() == "/to/")
            .unwrap();
        let transfer = commands
            .iter()
            .position(|command| command.step == DavMutationStepKind::CopyFile)
            .unwrap();
        assert!(leaf < nested && nested < root && root < transfer);
    });
}

#[test]
fn failed_collection_replacement_does_not_transfer_file() {
    futures::executor::block_on(async {
        let enumerator = Enumerator::default().with("/to/", vec![Entry::file("locked.txt")]);
        let locked_path = "/to/locked.txt";
        let locked = DavPath::new(locked_path).unwrap();
        let port = Port::default().failing(
            locked_path,
            DavMutationStepError::locked(locked.clone(), locked),
        );
        let outcome = execute_recursive_mutation(
            DavMutationRequest {
                operation: DavMutationOperation::Move,
                source: DavPath::new("/from.txt").unwrap(),
                source_kind: DavResourceKind::File,
                destination: Some(DavPath::new("/to/").unwrap()),
                destination_kind: Some(DavResourceKind::Collection),
                destination_existed: true,
                recurse_collections: false,
            },
            &enumerator,
            &port,
            &DavNeverCancelled,
            limits(8, 8, 2, 2, 8),
        )
        .await;

        assert_eq!(outcome.failures.len(), 1);
        assert!(
            port.commands()
                .iter()
                .all(|command| command.step != DavMutationStepKind::MoveFile)
        );
        assert_eq!(
            mutation_outcome_response("/webdav", &outcome, DavMultiStatusLimits::default())
                .unwrap()
                .status,
            StatusCode::MULTI_STATUS
        );
    });
}

#[test]
fn matching_file_replaces_destination_collection_and_siblings_continue() {
    futures::executor::block_on(async {
        let enumerator = Enumerator::default()
            .with(
                "/source/",
                vec![Entry::file("same"), Entry::file("sibling.txt")],
            )
            .with("/destination/", vec![Entry::collection("same")])
            .with("/destination/same/", vec![Entry::file("old.txt")]);
        let port = Port::default();
        let outcome = execute_recursive_mutation(
            request(DavMutationOperation::Copy),
            &enumerator,
            &port,
            &DavNeverCancelled,
            limits(16, 16, 4, 4, 8),
        )
        .await;

        assert!(outcome.is_complete_success());
        let commands = port.commands();
        let collection_delete = commands
            .iter()
            .position(|command| {
                command.step == DavMutationStepKind::DeleteCollection
                    && command.source.as_str() == "/destination/same/"
            })
            .unwrap();
        let matching_transfer = commands
            .iter()
            .position(|command| {
                command.step == DavMutationStepKind::CopyFile
                    && command.source.as_str() == "/source/same"
            })
            .unwrap();
        assert!(collection_delete < matching_transfer);
        assert!(commands.iter().any(|command| {
            command.step == DavMutationStepKind::CopyFile
                && command.source.as_str() == "/source/sibling.txt"
        }));
    });
}

#[test]
fn depth_zero_collection_only_prepares_destination() {
    futures::executor::block_on(async {
        let enumerator = Enumerator::default().with("/source/", vec![Entry::file("ignored")]);
        let port = Port::default();
        let mut request = request(DavMutationOperation::Copy);
        request.recurse_collections = false;
        let outcome = execute_recursive_mutation(
            request,
            &enumerator,
            &port,
            &DavNeverCancelled,
            limits(1, 1, 1, 0, 1),
        )
        .await;

        assert!(outcome.is_complete_success());
        assert_eq!(port.commands().len(), 1);
        assert_eq!(
            port.commands()[0].step,
            DavMutationStepKind::PrepareCollection
        );
    });
}

#[test]
fn root_lock_failure_is_direct_423_with_lock_root() {
    futures::executor::block_on(async {
        let destination = DavPath::new("/to.txt").unwrap();
        let port = Port::default().failing(
            destination.as_str(),
            DavMutationStepError::locked(
                destination.clone(),
                DavPath::new("/locked-root/").unwrap(),
            ),
        );
        let outcome = execute_recursive_mutation(
            DavMutationRequest {
                operation: DavMutationOperation::Copy,
                source: DavPath::new("/from.txt").unwrap(),
                source_kind: DavResourceKind::File,
                destination: Some(destination),
                destination_kind: Some(DavResourceKind::File),
                destination_existed: true,
                recurse_collections: false,
            },
            &Enumerator::default(),
            &port,
            &DavNeverCancelled,
            limits(1, 1, 1, 0, 1),
        )
        .await;
        let response =
            mutation_outcome_response("/webdav", &outcome, DavMultiStatusLimits::default())
                .unwrap();
        assert_eq!(response.status, StatusCode::LOCKED);
        let DavResponseBody::Bytes(body) = response.body else {
            panic!("locked request failure should contain DAV:error XML");
        };
        let xml = String::from_utf8(body.to_vec()).unwrap();
        assert!(xml.contains("/webdav/locked-root/"), "{xml}");
        assert!(xml.contains("lock-token-submitted"), "{xml}");
    });
}

#[test]
fn traversal_limits_accept_exact_and_stop_at_plus_one() {
    futures::executor::block_on(async {
        let one_child = Enumerator::default().with("/source/", vec![Entry::file("one")]);
        let exact_visited = execute_recursive_mutation(
            request(DavMutationOperation::Delete),
            &one_child,
            &Port::default(),
            &DavNeverCancelled,
            limits(2, 2, 1, 1, 2),
        )
        .await;
        assert!(exact_visited.is_complete_success());

        let visited_plus_one = execute_recursive_mutation(
            request(DavMutationOperation::Delete),
            &one_child,
            &Port::default(),
            &DavNeverCancelled,
            limits(1, 2, 1, 1, 2),
        )
        .await;
        assert_eq!(
            visited_plus_one.stop,
            Some(DavMutationStop::InsufficientStorage)
        );

        let two_children =
            Enumerator::default().with("/source/", vec![Entry::file("one"), Entry::file("two")]);
        let queue_plus_one = execute_recursive_mutation(
            request(DavMutationOperation::Delete),
            &two_children,
            &Port::default(),
            &DavNeverCancelled,
            limits(3, 2, 1, 1, 2),
        )
        .await;
        assert_eq!(
            queue_plus_one.stop,
            Some(DavMutationStop::InsufficientStorage)
        );

        let nested = Enumerator::default()
            .with("/source/", vec![Entry::collection("nested")])
            .with("/source/nested/", vec![Entry::file("leaf")]);
        let depth_plus_one = execute_recursive_mutation(
            request(DavMutationOperation::Delete),
            &nested,
            &Port::default(),
            &DavNeverCancelled,
            limits(3, 3, 1, 1, 2),
        )
        .await;
        assert_eq!(
            depth_plus_one.stop,
            Some(DavMutationStop::InsufficientStorage)
        );
    });
}

#[test]
fn directory_page_limit_accepts_exact_and_stops_at_plus_one() {
    futures::executor::block_on(async {
        let enumerator = Enumerator::default().with(
            "/source/",
            vec![Entry::file("a"), Entry::file("b"), Entry::file("c")],
        );
        let exact = execute_recursive_mutation(
            request(DavMutationOperation::Delete),
            &enumerator,
            &Port::default(),
            &DavNeverCancelled,
            limits_with_pages(4, 4, 1, 1, 2, 2, 3),
        )
        .await;
        assert!(exact.is_complete_success());

        let plus_one = execute_recursive_mutation(
            request(DavMutationOperation::Delete),
            &enumerator,
            &Port::default(),
            &DavNeverCancelled,
            limits_with_pages(4, 4, 1, 1, 1, 2, 3),
        )
        .await;
        assert_eq!(plus_one.stop, Some(DavMutationStop::InsufficientStorage));
        assert_eq!(plus_one.progress.completed_mutations, 0);
    });
}
