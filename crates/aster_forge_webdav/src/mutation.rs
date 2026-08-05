//! Bounded recursive COPY, MOVE, and DELETE execution.

use http::StatusCode;

use crate::response::{no_store_empty_response, xml_document_response};
use crate::{
    DavBackendErrorKind, DavCancellation, DavDirectoryEntry, DavDirectoryEnumerator,
    DavDirectoryPageLimits, DavDirectoryPageState, DavDirectoryReadError, DavErrorCondition,
    DavMetaData, DavMultiStatusError, DavMutationFailure, DavPath, DavPathError, DavResourceKind,
    DavResponse, DavTraversalBudget, DavTraversalError, DavTraversalErrorKind, DavTraversalLimits,
    DavXmlError, dav_error_element, delete_success_response,
    mutation_multistatus_response_with_limits, mutation_success_response, read_next_directory_page,
};

/// Recursive mutation selected by the request method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavMutationOperation {
    Copy,
    Move,
    Delete,
}

/// Product mutation performed by one authoritative commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavMutationStepKind {
    CopyFile,
    MoveFile,
    PrepareCollection,
    DeleteFile,
    DeleteCollection,
}

/// Role of the resource modified by a mutation step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavMutationTargetRole {
    Source,
    Destination,
}

/// One product-authoritative mutation step.
///
/// The product port must perform the resource write, transactional lock revalidation, rooted-lock
/// cleanup or destination-lock rebinding, and derived lock-state synchronization in one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavMutationCommand {
    pub operation: DavMutationOperation,
    pub step: DavMutationStepKind,
    pub role: DavMutationTargetRole,
    pub source: DavPath,
    pub destination: Option<DavPath>,
}

impl DavMutationCommand {
    #[must_use]
    pub fn affected_path(&self) -> &DavPath {
        self.destination.as_ref().unwrap_or(&self.source)
    }
}

/// Typed failure returned by the authoritative product mutation port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DavMutationStepError {
    Locked {
        affected_path: DavPath,
        lock_root: DavPath,
    },
    Status {
        affected_path: DavPath,
        status: u16,
    },
}

impl DavMutationStepError {
    #[must_use]
    pub fn locked(affected_path: DavPath, lock_root: DavPath) -> Self {
        Self::Locked {
            affected_path,
            lock_root,
        }
    }

    #[must_use]
    pub fn status(affected_path: DavPath, status: u16) -> Self {
        Self::Status {
            affected_path,
            status,
        }
    }

    #[must_use]
    pub fn backend(affected_path: DavPath) -> Self {
        Self::status(affected_path, StatusCode::INTERNAL_SERVER_ERROR.as_u16())
    }

    /// Maps one typed backend failure to the canonical resource status.
    #[must_use]
    pub fn from_backend(affected_path: DavPath, error: &crate::DavBackendError) -> Self {
        let status = match error.kind {
            DavBackendErrorKind::NotFound => StatusCode::NOT_FOUND,
            DavBackendErrorKind::Forbidden => StatusCode::FORBIDDEN,
            DavBackendErrorKind::Conflict | DavBackendErrorKind::AlreadyExists => {
                StatusCode::CONFLICT
            }
            DavBackendErrorKind::InsufficientStorage => StatusCode::INSUFFICIENT_STORAGE,
            DavBackendErrorKind::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            DavBackendErrorKind::Locked => StatusCode::LOCKED,
            DavBackendErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
            DavBackendErrorKind::Unsupported => StatusCode::METHOD_NOT_ALLOWED,
            DavBackendErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self::status(affected_path, status.as_u16())
    }

    fn into_failure(self) -> DavMutationFailure {
        match self {
            Self::Locked {
                affected_path,
                lock_root,
            } => DavMutationFailure::locked(affected_path, lock_root),
            Self::Status {
                affected_path,
                status,
            } => DavMutationFailure::status(affected_path, status),
        }
    }
}

/// Product port for a single atomic mutation step.
///
/// Forge deliberately does not expose a transaction type. The implementation owns its writer
/// transaction and returns only after commit succeeds.
pub trait DavMutationPort: Send + Sync {
    fn execute(
        &self,
        command: DavMutationCommand,
    ) -> impl Future<Output = Result<(), DavMutationStepError>> + Send;
}

/// Hard limits for one recursive mutation execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavMutationExecutorLimits {
    pub traversal: DavTraversalLimits,
    pub directory_pages: DavDirectoryPageLimits,
    pub directory_page_entries: usize,
    pub maximum_directory_entries: usize,
}

impl DavMutationExecutorLimits {
    #[must_use]
    pub const fn new(
        traversal: DavTraversalLimits,
        directory_pages: DavDirectoryPageLimits,
        directory_page_entries: usize,
        maximum_directory_entries: usize,
    ) -> Self {
        Self {
            traversal,
            directory_pages,
            directory_page_entries,
            maximum_directory_entries,
        }
    }
}

/// Input for one recursive mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavMutationRequest {
    pub operation: DavMutationOperation,
    pub source: DavPath,
    pub source_kind: DavResourceKind,
    pub destination: Option<DavPath>,
    pub destination_kind: Option<DavResourceKind>,
    pub destination_existed: bool,
    pub recurse_collections: bool,
}

/// Terminal reason when bounded execution stops before draining its work queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavMutationStop {
    Cancelled,
    InsufficientStorage,
    Backend,
}

/// Complete or partial result of recursive mutation execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavMutationOutcome {
    pub operation: DavMutationOperation,
    pub request_target: DavPath,
    pub destination_existed: bool,
    pub failures: Vec<DavMutationFailure>,
    pub stop: Option<DavMutationStop>,
    pub progress: crate::DavTraversalProgress,
}

impl DavMutationOutcome {
    #[must_use]
    pub fn is_complete_success(&self) -> bool {
        self.failures.is_empty() && self.stop.is_none()
    }
}

#[derive(Debug, Clone)]
struct Child {
    name: bytes::Bytes,
    key: bytes::Bytes,
    path: DavPath,
    is_collection: bool,
}

impl Child {
    fn namespace_name(&self) -> &[u8] {
        &self.name
    }
}

#[derive(Debug, Clone)]
enum Work {
    TransferDirectory {
        source: DavPath,
        destination: DavPath,
        depth: usize,
        parent: Option<usize>,
    },
    TransferFile {
        source: DavPath,
        destination: DavPath,
        depth: usize,
        parent: Option<usize>,
    },
    ReplaceWithFile {
        source: DavPath,
        destination: DavPath,
        depth: usize,
        parent: Option<usize>,
    },
    DeleteNode {
        path: DavPath,
        is_collection: bool,
        role: DavMutationTargetRole,
        depth: usize,
        parent: Option<usize>,
    },
    FinalizeTransfer {
        source: DavPath,
        frame: usize,
    },
    FinalizeFileReplacement {
        source: DavPath,
        destination: DavPath,
        depth: usize,
        frame: usize,
    },
    FinalizeDelete {
        path: DavPath,
        role: DavMutationTargetRole,
        frame: usize,
    },
}

#[derive(Debug, Clone, Copy)]
struct Frame {
    parent: Option<usize>,
    failed: bool,
}

/// Executes a bounded recursive COPY, MOVE, or DELETE.
///
/// A collection failure prunes that subtree. Failures in one child are recorded while eligible
/// siblings continue. Collection removal is post-order and is skipped after any descendant
/// failure, preserving namespace consistency.
///
/// # Errors
///
/// Invalid request shapes return a typed backend-style stop outcome rather than invoking the port.
#[expect(
    clippy::too_many_lines,
    reason = "the iterative state machine keeps queue accounting, frame propagation, and cancellation order visible in one loop"
)]
pub async fn execute_recursive_mutation<E, P, C>(
    request: DavMutationRequest,
    enumerator: &E,
    port: &P,
    cancellation: &C,
    limits: DavMutationExecutorLimits,
) -> DavMutationOutcome
where
    E: DavDirectoryEnumerator,
    P: DavMutationPort,
    C: DavCancellation,
{
    let operation = request.operation;
    let destination_existed = request.destination_existed;
    let recurse_collections = request.recurse_collections;
    let request_target = request
        .destination
        .as_ref()
        .unwrap_or(&request.source)
        .clone();
    let mut failures = Vec::new();
    let mut stop = None;
    let mut budget = match DavTraversalBudget::new(limits.traversal) {
        Ok(budget)
            if limits.directory_page_entries != 0 && limits.maximum_directory_entries != 0 =>
        {
            budget
        }
        Ok(budget) => {
            return DavMutationOutcome {
                operation,
                request_target,
                destination_existed,
                failures,
                stop: Some(DavMutationStop::InsufficientStorage),
                progress: budget.progress(),
            };
        }
        Err(error) => {
            return DavMutationOutcome {
                operation,
                request_target,
                destination_existed,
                failures,
                stop: Some(stop_from_traversal(error)),
                progress: error.progress,
            };
        }
    };

    let root = match (
        operation,
        request.source_kind,
        request.destination,
        request.destination_kind,
    ) {
        (DavMutationOperation::Move, DavResourceKind::Collection, Some(_), _)
            if !recurse_collections =>
        {
            return DavMutationOutcome {
                operation,
                request_target,
                destination_existed,
                failures,
                stop: Some(DavMutationStop::Backend),
                progress: budget.progress(),
            };
        }
        (DavMutationOperation::Delete, kind, _, _) => Work::DeleteNode {
            path: request.source,
            is_collection: kind == DavResourceKind::Collection,
            role: DavMutationTargetRole::Source,
            depth: 0,
            parent: None,
        },
        (
            DavMutationOperation::Copy | DavMutationOperation::Move,
            DavResourceKind::File,
            Some(destination),
            Some(DavResourceKind::Collection),
        ) => Work::ReplaceWithFile {
            source: request.source,
            destination,
            depth: 0,
            parent: None,
        },
        (
            DavMutationOperation::Copy | DavMutationOperation::Move,
            DavResourceKind::File,
            Some(destination),
            _,
        ) => Work::TransferFile {
            source: request.source,
            destination,
            depth: 0,
            parent: None,
        },
        (
            DavMutationOperation::Copy | DavMutationOperation::Move,
            DavResourceKind::Collection,
            Some(destination),
            _,
        ) => Work::TransferDirectory {
            source: request.source,
            destination,
            depth: 0,
            parent: None,
        },
        _ => {
            return DavMutationOutcome {
                operation,
                request_target,
                destination_existed,
                failures,
                stop: Some(DavMutationStop::Backend),
                progress: budget.progress(),
            };
        }
    };

    let mut work = vec![root];
    if let Err(error) = budget.reserve_work(1) {
        stop = Some(stop_from_traversal(error));
    }
    let mut frames = Vec::new();

    while stop.is_none() {
        if let Err(error) = budget.checkpoint(cancellation) {
            stop = Some(stop_from_traversal(error));
            break;
        }
        let Some(item) = work.pop() else {
            break;
        };
        budget.complete_work();

        match item {
            Work::TransferFile {
                source,
                destination,
                depth,
                parent,
            } => {
                if let Err(error) = budget.visit(depth) {
                    stop = Some(stop_from_traversal(error));
                    continue;
                }
                let frame = parent.unwrap_or_else(|| {
                    let frame = frames.len();
                    frames.push(Frame {
                        parent: None,
                        failed: false,
                    });
                    frame
                });
                let step_kind = if operation == DavMutationOperation::Move {
                    DavMutationStepKind::MoveFile
                } else {
                    DavMutationStepKind::CopyFile
                };
                let command = DavMutationCommand {
                    operation,
                    step: step_kind,
                    role: DavMutationTargetRole::Destination,
                    source,
                    destination: Some(destination),
                };
                if let Err(error) = apply_step(
                    port,
                    command,
                    frame,
                    &mut frames,
                    &mut failures,
                    &mut budget,
                )
                .await
                {
                    stop = Some(stop_from_traversal(error));
                }
            }
            Work::ReplaceWithFile {
                source,
                destination,
                depth,
                parent,
            } => {
                let frame = frames.len();
                frames.push(Frame {
                    parent,
                    failed: false,
                });
                let next = [
                    Work::DeleteNode {
                        path: destination.clone(),
                        is_collection: true,
                        role: DavMutationTargetRole::Destination,
                        depth,
                        parent: Some(frame),
                    },
                    Work::FinalizeFileReplacement {
                        source,
                        destination,
                        depth,
                        frame,
                    },
                ];
                if let Err(error) = budget.reserve_work(next.len()) {
                    stop = Some(stop_from_traversal(error));
                    continue;
                }
                work.extend(next.into_iter().rev());
            }
            Work::TransferDirectory {
                source,
                destination,
                depth,
                parent,
            } => {
                if let Err(error) = budget.visit(depth) {
                    stop = Some(stop_from_traversal(error));
                    continue;
                }
                let frame = frames.len();
                frames.push(Frame {
                    parent,
                    failed: false,
                });
                let command = DavMutationCommand {
                    operation,
                    step: DavMutationStepKind::PrepareCollection,
                    role: DavMutationTargetRole::Destination,
                    source: source.clone(),
                    destination: Some(destination.clone()),
                };
                let applied = match apply_step(
                    port,
                    command,
                    frame,
                    &mut frames,
                    &mut failures,
                    &mut budget,
                )
                .await
                {
                    Ok(applied) => applied,
                    Err(error) => {
                        stop = Some(stop_from_traversal(error));
                        false
                    }
                };
                if !applied {
                    propagate_frame_failure(frame, &mut frames);
                    continue;
                }
                if !recurse_collections {
                    propagate_frame_failure(frame, &mut frames);
                    continue;
                }

                let source_children =
                    match read_children(enumerator, &source, cancellation, limits).await {
                        Ok(children) => children,
                        Err(error) => {
                            record_directory_read_failure(
                                error,
                                source,
                                frame,
                                &mut frames,
                                &mut failures,
                                &mut budget,
                                &mut stop,
                            );
                            propagate_frame_failure(frame, &mut frames);
                            continue;
                        }
                    };
                let destination_children =
                    match read_children(enumerator, &destination, cancellation, limits).await {
                        Ok(children) => children,
                        Err(error) => {
                            record_directory_read_failure(
                                error,
                                destination,
                                frame,
                                &mut frames,
                                &mut failures,
                                &mut budget,
                                &mut stop,
                            );
                            propagate_frame_failure(frame, &mut frames);
                            continue;
                        }
                    };
                let Some(child_depth) = depth.checked_add(1) else {
                    stop = Some(DavMutationStop::InsufficientStorage);
                    continue;
                };
                let Ok(mut next) = merge_transfer_children(
                    &destination,
                    &source_children,
                    &destination_children,
                    child_depth,
                    frame,
                ) else {
                    stop = Some(DavMutationStop::Backend);
                    propagate_frame_failure(frame, &mut frames);
                    continue;
                };
                next.push(Work::FinalizeTransfer { source, frame });
                if let Err(error) = budget.reserve_work(next.len()) {
                    stop = Some(stop_from_traversal(error));
                    continue;
                }
                work.extend(next.into_iter().rev());
            }
            Work::DeleteNode {
                path,
                is_collection: false,
                role,
                depth,
                parent,
            } => {
                if let Err(error) = budget.visit(depth) {
                    stop = Some(stop_from_traversal(error));
                    continue;
                }
                let frame = parent.unwrap_or_else(|| {
                    let frame = frames.len();
                    frames.push(Frame {
                        parent: None,
                        failed: false,
                    });
                    frame
                });
                let command = DavMutationCommand {
                    operation,
                    step: DavMutationStepKind::DeleteFile,
                    role,
                    source: path,
                    destination: None,
                };
                if let Err(error) = apply_step(
                    port,
                    command,
                    frame,
                    &mut frames,
                    &mut failures,
                    &mut budget,
                )
                .await
                {
                    stop = Some(stop_from_traversal(error));
                }
            }
            Work::DeleteNode {
                path,
                is_collection: true,
                role,
                depth,
                parent,
            } => {
                if let Err(error) = budget.visit(depth) {
                    stop = Some(stop_from_traversal(error));
                    continue;
                }
                let frame = frames.len();
                frames.push(Frame {
                    parent,
                    failed: false,
                });
                let children = match read_children(enumerator, &path, cancellation, limits).await {
                    Ok(children) => children,
                    Err(error) => {
                        record_directory_read_failure(
                            error,
                            path.clone(),
                            frame,
                            &mut frames,
                            &mut failures,
                            &mut budget,
                            &mut stop,
                        );
                        propagate_frame_failure(frame, &mut frames);
                        continue;
                    }
                };
                let Some(child_depth) = depth.checked_add(1) else {
                    stop = Some(DavMutationStop::InsufficientStorage);
                    continue;
                };
                let mut next = Vec::with_capacity(children.len().saturating_add(1));
                for child in children {
                    next.push(Work::DeleteNode {
                        path: child.path,
                        is_collection: child.is_collection,
                        role,
                        depth: child_depth,
                        parent: Some(frame),
                    });
                }
                next.push(Work::FinalizeDelete { path, role, frame });
                if let Err(error) = budget.reserve_work(next.len()) {
                    stop = Some(stop_from_traversal(error));
                    continue;
                }
                work.extend(next.into_iter().rev());
            }
            Work::FinalizeTransfer { source, frame } => {
                if operation != DavMutationOperation::Move || frames[frame].failed {
                    propagate_frame_failure(frame, &mut frames);
                    continue;
                }
                let command = DavMutationCommand {
                    operation,
                    step: DavMutationStepKind::DeleteCollection,
                    role: DavMutationTargetRole::Source,
                    source,
                    destination: None,
                };
                if let Err(error) = apply_step(
                    port,
                    command,
                    frame,
                    &mut frames,
                    &mut failures,
                    &mut budget,
                )
                .await
                {
                    stop = Some(stop_from_traversal(error));
                }
                propagate_frame_failure(frame, &mut frames);
            }
            Work::FinalizeFileReplacement {
                source,
                destination,
                depth,
                frame,
            } => {
                if frames[frame].failed {
                    propagate_frame_failure(frame, &mut frames);
                    continue;
                }
                if let Err(error) = budget.visit(depth) {
                    stop = Some(stop_from_traversal(error));
                    continue;
                }
                let step_kind = if operation == DavMutationOperation::Move {
                    DavMutationStepKind::MoveFile
                } else {
                    DavMutationStepKind::CopyFile
                };
                let command = DavMutationCommand {
                    operation,
                    step: step_kind,
                    role: DavMutationTargetRole::Destination,
                    source,
                    destination: Some(destination),
                };
                if let Err(error) = apply_step(
                    port,
                    command,
                    frame,
                    &mut frames,
                    &mut failures,
                    &mut budget,
                )
                .await
                {
                    stop = Some(stop_from_traversal(error));
                }
                propagate_frame_failure(frame, &mut frames);
            }
            Work::FinalizeDelete { path, role, frame } => {
                if frames[frame].failed {
                    propagate_frame_failure(frame, &mut frames);
                    continue;
                }
                let command = DavMutationCommand {
                    operation,
                    step: DavMutationStepKind::DeleteCollection,
                    role,
                    source: path,
                    destination: None,
                };
                if let Err(error) = apply_step(
                    port,
                    command,
                    frame,
                    &mut frames,
                    &mut failures,
                    &mut budget,
                )
                .await
                {
                    stop = Some(stop_from_traversal(error));
                }
                propagate_frame_failure(frame, &mut frames);
            }
        }
    }

    DavMutationOutcome {
        operation,
        request_target,
        destination_existed,
        failures,
        stop,
        progress: budget.progress(),
    }
}

async fn apply_step<P: DavMutationPort>(
    port: &P,
    command: DavMutationCommand,
    frame: usize,
    frames: &mut [Frame],
    failures: &mut Vec<DavMutationFailure>,
    budget: &mut DavTraversalBudget,
) -> Result<bool, DavTraversalError> {
    match port.execute(command).await {
        Ok(()) => {
            budget.record_completed_mutation();
            Ok(true)
        }
        Err(error) => {
            frames[frame].failed = true;
            budget.record_failure()?;
            failures.push(error.into_failure());
            Ok(false)
        }
    }
}

fn propagate_frame_failure(frame: usize, frames: &mut [Frame]) {
    if !frames[frame].failed {
        return;
    }
    if let Some(parent) = frames[frame].parent {
        frames[parent].failed = true;
    }
}

async fn read_children<E: DavDirectoryEnumerator, C: DavCancellation>(
    enumerator: &E,
    path: &DavPath,
    cancellation: &C,
    limits: DavMutationExecutorLimits,
) -> Result<Vec<Child>, DavDirectoryReadError> {
    let mut state = DavDirectoryPageState::new();
    let mut children = Vec::new();
    while let Some(page) = read_next_directory_page(
        enumerator,
        path,
        &mut state,
        limits.directory_page_entries,
        limits.directory_pages,
        cancellation,
    )
    .await?
    {
        if children
            .len()
            .checked_add(page.entries.len())
            .is_none_or(|count| count > limits.maximum_directory_entries)
        {
            return Err(DavDirectoryReadError::Backend(crate::DavBackendError::new(
                DavBackendErrorKind::InsufficientStorage,
            )));
        }
        for entry in page.entries {
            let is_collection = entry.metadata().is_dir();
            let name = bytes::Bytes::copy_from_slice(entry.name());
            let child_path = path.join_child(&name, is_collection).map_err(|_| {
                DavDirectoryReadError::Backend(crate::DavBackendError::new(
                    DavBackendErrorKind::Internal,
                ))
            })?;
            children.push(Child {
                name,
                key: bytes::Bytes::copy_from_slice(entry.stable_key()),
                path: child_path,
                is_collection,
            });
        }
    }
    Ok(children)
}

fn merge_transfer_children(
    destination_root: &DavPath,
    source: &[Child],
    destination: &[Child],
    depth: usize,
    frame: usize,
) -> Result<Vec<Work>, DavPathError> {
    let mut work = Vec::with_capacity(source.len().saturating_add(destination.len()));
    let mut source_order = (0..source.len()).collect::<Vec<_>>();
    source_order.sort_unstable_by(|left, right| {
        source[*left]
            .namespace_name()
            .cmp(source[*right].namespace_name())
            .then_with(|| source[*left].key.cmp(&source[*right].key))
    });
    let mut destination_order = (0..destination.len()).collect::<Vec<_>>();
    destination_order.sort_unstable_by(|left, right| {
        destination[*left]
            .namespace_name()
            .cmp(destination[*right].namespace_name())
            .then_with(|| destination[*left].key.cmp(&destination[*right].key))
    });
    let mut source_index = 0;
    let mut destination_index = 0;
    while source_index < source.len() || destination_index < destination.len() {
        match (
            source_order
                .get(source_index)
                .and_then(|index| source.get(*index)),
            destination_order
                .get(destination_index)
                .and_then(|index| destination.get(*index)),
        ) {
            (Some(source_child), Some(destination_child)) => {
                match source_child
                    .namespace_name()
                    .cmp(destination_child.namespace_name())
                {
                    std::cmp::Ordering::Less => {
                        push_source_child(&mut work, destination_root, source_child, depth, frame)?;
                        source_index += 1;
                    }
                    std::cmp::Ordering::Equal => {
                        push_matching_children(
                            &mut work,
                            destination_root,
                            source_child,
                            destination_child,
                            depth,
                            frame,
                        )?;
                        source_index += 1;
                        destination_index += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        work.push(destination_delete_work(destination_child, depth, frame));
                        destination_index += 1;
                    }
                }
            }
            (Some(source_child), None) => {
                push_source_child(&mut work, destination_root, source_child, depth, frame)?;
                source_index += 1;
            }
            (None, Some(destination_child)) => {
                work.push(destination_delete_work(destination_child, depth, frame));
                destination_index += 1;
            }
            (None, None) => break,
        }
    }
    Ok(work)
}

fn push_source_child(
    work: &mut Vec<Work>,
    destination_root: &DavPath,
    child: &Child,
    depth: usize,
    frame: usize,
) -> Result<(), DavPathError> {
    let destination = destination_root.join_child(&child.name, child.is_collection)?;
    if child.is_collection {
        work.push(Work::TransferDirectory {
            source: child.path.clone(),
            destination,
            depth,
            parent: Some(frame),
        });
    } else {
        work.push(Work::TransferFile {
            source: child.path.clone(),
            destination,
            depth,
            parent: Some(frame),
        });
    }
    Ok(())
}

fn push_matching_children(
    work: &mut Vec<Work>,
    destination_root: &DavPath,
    source: &Child,
    destination: &Child,
    depth: usize,
    frame: usize,
) -> Result<(), DavPathError> {
    let destination_path = destination_root.join_child(&source.name, source.is_collection)?;
    if source.is_collection {
        work.push(Work::TransferDirectory {
            source: source.path.clone(),
            destination: destination_path,
            depth,
            parent: Some(frame),
        });
    } else if destination.is_collection {
        work.push(Work::ReplaceWithFile {
            source: source.path.clone(),
            destination: destination.path.clone(),
            depth,
            parent: Some(frame),
        });
    } else {
        work.push(Work::TransferFile {
            source: source.path.clone(),
            destination: destination_path,
            depth,
            parent: Some(frame),
        });
    }
    Ok(())
}

fn destination_delete_work(child: &Child, depth: usize, frame: usize) -> Work {
    Work::DeleteNode {
        path: child.path.clone(),
        is_collection: child.is_collection,
        role: DavMutationTargetRole::Destination,
        depth,
        parent: Some(frame),
    }
}

fn record_directory_read_failure(
    error: DavDirectoryReadError,
    path: DavPath,
    frame: usize,
    frames: &mut [Frame],
    failures: &mut Vec<DavMutationFailure>,
    budget: &mut DavTraversalBudget,
    stop: &mut Option<DavMutationStop>,
) {
    match error {
        DavDirectoryReadError::Cancelled => *stop = Some(DavMutationStop::Cancelled),
        DavDirectoryReadError::Backend(error)
            if error.kind == DavBackendErrorKind::InsufficientStorage =>
        {
            *stop = Some(DavMutationStop::InsufficientStorage);
        }
        DavDirectoryReadError::PageLimitExceeded
        | DavDirectoryReadError::InvalidLimit
        | DavDirectoryReadError::InvalidPage(_) => {
            *stop = Some(DavMutationStop::InsufficientStorage);
        }
        DavDirectoryReadError::Backend(error) => {
            frames[frame].failed = true;
            if let Err(error) = budget.record_failure() {
                *stop = Some(stop_from_traversal(error));
                return;
            }
            failures.push(DavMutationStepError::from_backend(path, &error).into_failure());
        }
    }
}

fn stop_from_traversal(error: DavTraversalError) -> DavMutationStop {
    match error.kind {
        DavTraversalErrorKind::Cancelled => DavMutationStop::Cancelled,
        DavTraversalErrorKind::InvalidLimits
        | DavTraversalErrorKind::VisitedResourceLimitExceeded
        | DavTraversalErrorKind::QueuedWorkLimitExceeded
        | DavTraversalErrorKind::FailureLimitExceeded
        | DavTraversalErrorKind::DepthLimitExceeded => DavMutationStop::InsufficientStorage,
    }
}

/// Composes the canonical response for a recursive mutation outcome.
///
/// # Errors
///
/// Returns a Multi-Status serialization error when accumulated failures exceed response limits.
pub fn mutation_outcome_response(
    prefix: &str,
    outcome: &DavMutationOutcome,
    limits: crate::DavMultiStatusLimits,
) -> Result<DavResponse, DavMutationComposeError> {
    let stop_status = outcome.stop.map(stop_status);
    let partial_execution = outcome.progress.completed_mutations != 0;
    let request_failure = !partial_execution
        && outcome.stop.is_none()
        && outcome.failures.len() == 1
        && outcome.failures[0].path() == &outcome.request_target;
    if request_failure {
        return direct_failure_response(prefix, &outcome.failures[0]);
    }
    if !outcome.failures.is_empty() || stop_status.is_some_and(|_| partial_execution) {
        let mut failures = outcome.failures.clone();
        if let Some(status) = stop_status
            && !failures.iter().any(|failure| {
                failure.path() == &outcome.request_target
                    && failure.status_code() == status.as_u16()
            })
        {
            failures.push(DavMutationFailure::status(
                outcome.request_target.clone(),
                status.as_u16(),
            ));
        }
        return mutation_multistatus_response_with_limits(prefix, &failures, limits)
            .map_err(Into::into);
    }
    if let Some(status) = stop_status {
        return Ok(DavResponse::empty(status));
    }
    Ok(match outcome.operation {
        DavMutationOperation::Delete => delete_success_response(),
        DavMutationOperation::Copy | DavMutationOperation::Move => {
            mutation_success_response(outcome.destination_existed)
        }
    })
}

fn stop_status(stop: DavMutationStop) -> StatusCode {
    match stop {
        DavMutationStop::Cancelled => StatusCode::SERVICE_UNAVAILABLE,
        DavMutationStop::Backend => StatusCode::INTERNAL_SERVER_ERROR,
        DavMutationStop::InsufficientStorage => StatusCode::INSUFFICIENT_STORAGE,
    }
}

fn direct_failure_response(
    prefix: &str,
    failure: &DavMutationFailure,
) -> Result<DavResponse, DavMutationComposeError> {
    let status =
        StatusCode::from_u16(failure.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if let Some(lock_path) = failure.lock_path() {
        return xml_document_response(
            status,
            &dav_error_element(&DavErrorCondition::LockTokenSubmitted {
                href: crate::href_for_dav_path(prefix, lock_path),
            }),
        )
        .map_err(Into::into);
    }
    Ok(no_store_empty_response(status))
}

/// Failure while composing the final recursive mutation response.
#[derive(Debug, thiserror::Error)]
pub enum DavMutationComposeError {
    #[error("failed to compose mutation Multi-Status response")]
    MultiStatus(#[from] DavMultiStatusError),
    #[error("failed to compose mutation DAV error response")]
    Xml(#[from] DavXmlError),
}
