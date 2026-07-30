use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicUsize, Ordering},
};

use aster_forge_cloud_files_core::{
    BackendResult, CloudBackendError, CloudBackendErrorKind, CloudContentBackend, ContentReadRange,
    ContentReadRequest, ContentReadResponse, ContentRevision,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{FutureExt, channel::oneshot, future::Shared};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Clone)]
pub struct RevisionContent {
    pub revision: ContentRevision,
    pub bytes: Bytes,
}

pub struct RecordingContentBackend {
    contents: Vec<RevisionContent>,
    response_revision: Option<ContentRevision>,
    requests: Mutex<Vec<ContentReadRequest>>,
}

impl RecordingContentBackend {
    pub fn new(contents: Vec<RevisionContent>) -> Self {
        Self {
            contents,
            response_revision: None,
            requests: Mutex::new(Vec::new()),
        }
    }

    pub fn with_response_revision(mut self, revision: ContentRevision) -> Self {
        self.response_revision = Some(revision);
        self
    }

    pub fn requests(&self) -> Vec<ContentReadRequest> {
        lock(&self.requests).clone()
    }
}

#[async_trait]
impl CloudContentBackend for RecordingContentBackend {
    async fn read_content(
        &self,
        request: &ContentReadRequest,
    ) -> BackendResult<ContentReadResponse> {
        lock(&self.requests).push(request.clone());
        let Some(content) = self
            .contents
            .iter()
            .find(|content| &content.revision == request.revision())
        else {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::PreconditionFailed,
            ));
        };
        response_for(
            request,
            self.response_revision
                .clone()
                .unwrap_or_else(|| content.revision.clone()),
            &content.bytes,
        )
    }
}

type SharedGate = Shared<futures::future::BoxFuture<'static, ()>>;

pub struct BlockingContentBackend {
    content: RevisionContent,
    requests: Mutex<Vec<ContentReadRequest>>,
    started: AtomicUsize,
    cancelled: Arc<AtomicUsize>,
    completed: Arc<AtomicUsize>,
    release_tx: Mutex<Option<oneshot::Sender<()>>>,
    gate: SharedGate,
}

impl BlockingContentBackend {
    pub fn new(content: RevisionContent) -> Self {
        let (release_tx, release_rx) = oneshot::channel();
        let gate = async move {
            let _ = release_rx.await;
        }
        .boxed()
        .shared();
        Self {
            content,
            requests: Mutex::new(Vec::new()),
            started: AtomicUsize::new(0),
            cancelled: Arc::new(AtomicUsize::new(0)),
            completed: Arc::new(AtomicUsize::new(0)),
            release_tx: Mutex::new(Some(release_tx)),
            gate,
        }
    }

    pub fn started(&self) -> usize {
        self.started.load(Ordering::Acquire)
    }

    pub fn cancelled(&self) -> usize {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn completed(&self) -> usize {
        self.completed.load(Ordering::Acquire)
    }

    pub fn requests(&self) -> Vec<ContentReadRequest> {
        lock(&self.requests).clone()
    }

    pub fn release(&self) {
        if let Some(sender) = lock(&self.release_tx).take() {
            let _ = sender.send(());
        }
    }
}

struct ReadDropGuard {
    completed: bool,
    cancelled_count: Arc<AtomicUsize>,
    completed_count: Arc<AtomicUsize>,
}

impl ReadDropGuard {
    fn complete(&mut self) {
        self.completed = true;
        self.completed_count.fetch_add(1, Ordering::AcqRel);
    }
}

impl Drop for ReadDropGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.cancelled_count.fetch_add(1, Ordering::AcqRel);
        }
    }
}

#[async_trait]
impl CloudContentBackend for BlockingContentBackend {
    async fn read_content(
        &self,
        request: &ContentReadRequest,
    ) -> BackendResult<ContentReadResponse> {
        lock(&self.requests).push(request.clone());
        self.started.fetch_add(1, Ordering::AcqRel);
        let mut guard = ReadDropGuard {
            completed: false,
            cancelled_count: Arc::clone(&self.cancelled),
            completed_count: Arc::clone(&self.completed),
        };
        self.gate.clone().await;
        let response = response_for(request, self.content.revision.clone(), &self.content.bytes)?;
        guard.complete();
        Ok(response)
    }
}

fn response_for(
    request: &ContentReadRequest,
    response_revision: ContentRevision,
    content: &Bytes,
) -> BackendResult<ContentReadResponse> {
    let total_size = u64::try_from(content.len())
        .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
    let (offset, bytes) = match request.read_range() {
        ContentReadRange::Whole => (0, content.clone()),
        ContentReadRange::Range(range) => {
            if range.offset() > total_size {
                return Err(CloudBackendError::new(
                    CloudBackendErrorKind::InvalidRequest,
                ));
            }
            let start = usize::try_from(range.offset())
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
            let end = usize::try_from(range.end_exclusive().min(total_size))
                .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
            (range.offset(), content.slice(start..end))
        }
    };
    ContentReadResponse::new(response_revision, offset, bytes, total_size)
        .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
}

pub async fn wait_until_started(backend: &BlockingContentBackend, expected: usize) {
    for _ in 0..100 {
        if backend.started() >= expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("blocking backend did not start the expected number of reads");
}

pub fn revision(value: &str) -> ContentRevision {
    ContentRevision::from_slice(value.as_bytes()).expect("revision fixture should be valid")
}
