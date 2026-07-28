use std::sync::{Arc, Mutex};

use aster_forge_webdav::{
    DavBackendError, DavBackendErrorKind, DavRandomWriteHandle, DavRandomWriteSystem,
    DavWriteHandle, DavWriteOptions, DavWriteSystem, FsError,
};
use bytes::Bytes;

#[test]
fn filesystem_errors_map_exhaustively_to_protocol_backend_categories() {
    let cases = [
        (FsError::NotFound, DavBackendErrorKind::NotFound),
        (FsError::Forbidden, DavBackendErrorKind::Forbidden),
        (FsError::GeneralFailure, DavBackendErrorKind::Internal),
        (FsError::Exists, DavBackendErrorKind::AlreadyExists),
        (
            FsError::InsufficientStorage,
            DavBackendErrorKind::InsufficientStorage,
        ),
        (FsError::TooLarge, DavBackendErrorKind::PayloadTooLarge),
        (FsError::BadRequest, DavBackendErrorKind::InvalidInput),
    ];
    for (error, expected) in cases {
        assert_eq!(DavBackendError::from(error).kind, expected);
    }
}

#[test]
fn write_options_are_transport_neutral_values() {
    assert_eq!(
        DavWriteOptions::default(),
        DavWriteOptions {
            ..DavWriteOptions::default()
        }
    );
}

#[derive(Clone, Default)]
struct RecordingWriteSystem {
    opened: Arc<Mutex<Vec<DavWriteOptions>>>,
    writes: Arc<Mutex<Vec<(u64, Bytes)>>>,
}

struct RecordingWriteHandle {
    writes: Arc<Mutex<Vec<(u64, Bytes)>>>,
    next_offset: u64,
}

impl DavWriteHandle for RecordingWriteHandle {
    async fn write_bytes(&mut self, buf: Bytes) -> Result<(), DavBackendError> {
        let offset = self.next_offset;
        self.next_offset = self
            .next_offset
            .saturating_add(u64::try_from(buf.len()).expect("test chunk length should fit in u64"));
        let writes = Arc::clone(&self.writes);
        writes
            .lock()
            .expect("write log should not be poisoned")
            .push((offset, buf));
        Ok(())
    }

    async fn finish(self) -> Result<(), DavBackendError> {
        Ok(())
    }

    async fn abort(self) -> Result<(), DavBackendError> {
        Ok(())
    }
}

impl DavWriteSystem for RecordingWriteSystem {
    type Handle = RecordingWriteHandle;

    async fn open_write<'a>(
        &'a self,
        _path: &'a aster_forge_webdav::DavPath,
        options: DavWriteOptions,
    ) -> Result<Self::Handle, DavBackendError> {
        self.opened
            .lock()
            .expect("open log should not be poisoned")
            .push(options);
        let writes = Arc::clone(&self.writes);
        Ok(RecordingWriteHandle {
            writes,
            next_offset: 0,
        })
    }
}

#[derive(Default)]
struct RecordingRandomSystem {
    writes: Arc<Mutex<Vec<(u64, Bytes)>>>,
}

struct RecordingRandomHandle {
    writes: Arc<Mutex<Vec<(u64, Bytes)>>>,
}

impl DavRandomWriteHandle for RecordingRandomHandle {
    async fn write_at(&mut self, offset: u64, buf: Bytes) -> Result<(), DavBackendError> {
        let writes = Arc::clone(&self.writes);
        writes
            .lock()
            .expect("random write log should not be poisoned")
            .push((offset, buf));
        Ok(())
    }

    async fn finish(self) -> Result<(), DavBackendError> {
        Ok(())
    }

    async fn abort(self) -> Result<(), DavBackendError> {
        Ok(())
    }
}

impl DavRandomWriteSystem for RecordingRandomSystem {
    type Handle = RecordingRandomHandle;

    async fn open_random_write<'a>(
        &'a self,
        _path: &'a aster_forge_webdav::DavPath,
        _options: DavWriteOptions,
    ) -> Result<Self::Handle, DavBackendError> {
        let writes = Arc::clone(&self.writes);
        Ok(RecordingRandomHandle { writes })
    }
}

#[test]
fn sequential_and_random_write_ports_have_distinct_operations() {
    futures::executor::block_on(async {
        let path = aster_forge_webdav::DavPath::new("/file").expect("path");
        let sequential = RecordingWriteSystem::default();
        let mut writer = sequential
            .open_write(
                &path,
                DavWriteOptions {
                    create: true,
                    expected_length: Some(3),
                    ..DavWriteOptions::default()
                },
            )
            .await
            .expect("writer should open");
        writer
            .write_bytes(Bytes::from_static(b"abc"))
            .await
            .expect("sequential write");
        writer.finish().await.expect("finish should commit");
        assert_eq!(sequential.writes.lock().expect("writes").len(), 1);
        sequential
            .open_write(&path, DavWriteOptions::default())
            .await
            .expect("second writer should open")
            .abort()
            .await
            .expect("abort should clean up");

        let random = RecordingRandomSystem::default();
        let mut writer = random
            .open_random_write(&path, DavWriteOptions::default())
            .await
            .expect("random writer should open");
        writer
            .write_at(4096, Bytes::from_static(b"range"))
            .await
            .expect("random write");
        writer.abort().await.expect("abort should clean up");
        random
            .open_random_write(&path, DavWriteOptions::default())
            .await
            .expect("second random writer should open")
            .finish()
            .await
            .expect("random finish should commit");
        assert_eq!(
            random.writes.lock().expect("writes").as_slice(),
            &[(4096, Bytes::from_static(b"range"))]
        );
    });
}
