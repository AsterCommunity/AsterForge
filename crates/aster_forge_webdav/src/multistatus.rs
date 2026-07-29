//! Bounded incremental RFC 4918 Multi-Status response writing.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};
use std::pin::Pin;
use std::task::{Context, Poll};

use aster_forge_xml::{Error as ForgeXmlError, XmlSafetyError, XmlStreamWriter, XmlWriteOptions};
use bytes::{Bytes, BytesMut};
use futures::Stream;
use http::header::CONTENT_TYPE;
use http::{HeaderValue, StatusCode};

use crate::xml::write_element;
use crate::xml_response::error_condition_parts;
use crate::{
    DavBackendError, DavErrorCondition, DavMultiStatusItem, DavPropStat, DavResponse,
    DavResponseBody,
};

const DAV_NAMESPACE: &str = "DAV:";
const DEFAULT_MAXIMUM_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_MAXIMUM_ITEMS: usize = 100_000;
const DEFAULT_MAXIMUM_PROPERTIES_PER_ITEM: usize = 4_096;
const DEFAULT_CHUNK_BYTES: usize = 16 * 1024;

/// Product-configured resource limits for a Multi-Status response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavMultiStatusLimits {
    pub maximum_output_bytes: usize,
    pub maximum_items: usize,
    pub maximum_properties_per_item: usize,
    pub chunk_bytes: usize,
}

impl DavMultiStatusLimits {
    #[must_use]
    pub const fn new(
        maximum_output_bytes: usize,
        maximum_items: usize,
        maximum_properties_per_item: usize,
        chunk_bytes: usize,
    ) -> Self {
        Self {
            maximum_output_bytes,
            maximum_items,
            maximum_properties_per_item,
            chunk_bytes,
        }
    }

    fn validate(self) -> Result<(), DavMultiStatusError> {
        if self.maximum_output_bytes == 0
            || self.maximum_items == 0
            || self.maximum_properties_per_item == 0
            || self.chunk_bytes == 0
        {
            Err(DavMultiStatusError::new(
                DavMultiStatusErrorKind::InvalidLimits,
                DavMultiStatusProgress::default(),
            ))
        } else {
            Ok(())
        }
    }
}

impl Default for DavMultiStatusLimits {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAXIMUM_OUTPUT_BYTES,
            DEFAULT_MAXIMUM_ITEMS,
            DEFAULT_MAXIMUM_PROPERTIES_PER_ITEM,
            DEFAULT_CHUNK_BYTES,
        )
    }
}

/// Progress retained when bounded response generation stops.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DavMultiStatusProgress {
    pub response_started: bool,
    pub emitted_items: usize,
    pub emitted_bytes: usize,
}

/// Stable Multi-Status failure classification.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavMultiStatusErrorKind {
    #[error("invalid Multi-Status resource limits")]
    InvalidLimits,
    #[error("Multi-Status item limit exceeded")]
    ItemLimitExceeded,
    #[error("Multi-Status property limit exceeded")]
    PropertyLimitExceeded,
    #[error("invalid Multi-Status response item")]
    InvalidItem,
    #[error("Multi-Status output byte limit exceeded")]
    OutputLimitExceeded,
    #[error("Multi-Status source was cancelled")]
    Cancelled,
    #[error(transparent)]
    Backend(#[from] DavBackendError),
    #[error("Multi-Status XML is malformed")]
    Xml,
    #[error("Multi-Status output sink failed")]
    Write,
}

/// Multi-Status failure plus the exact response progress at the failure boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{kind}")]
pub struct DavMultiStatusError {
    pub kind: DavMultiStatusErrorKind,
    pub progress: DavMultiStatusProgress,
}

impl DavMultiStatusError {
    const fn new(kind: DavMultiStatusErrorKind, progress: DavMultiStatusProgress) -> Self {
        Self { kind, progress }
    }
}

/// Failure produced by the product-owned item source before XML composition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavMultiStatusSourceError {
    #[error(transparent)]
    Backend(#[from] DavBackendError),
    #[error("Multi-Status source was cancelled")]
    Cancelled,
}

/// Type-erased incremental Multi-Status response stream.
pub type DavMultiStatusStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, DavMultiStatusError>> + Send + 'static>>;

/// Stateful writer for one RFC 4918 `<D:multistatus>` document.
pub struct DavMultiStatusWriter<W: Write> {
    writer: XmlStreamWriter<TrackingWriter<W>>,
    inherited_namespaces: BTreeMap<String, String>,
    limits: DavMultiStatusLimits,
    emitted_items: usize,
}

impl<W: Write> DavMultiStatusWriter<W> {
    pub fn new(inner: W, limits: DavMultiStatusLimits) -> Result<Self, DavMultiStatusError> {
        limits.validate()?;
        let options = XmlWriteOptions::new().max_output_bytes(limits.maximum_output_bytes);
        let tracking = TrackingWriter::new(inner);
        let mut writer = XmlStreamWriter::with_options(tracking, options)
            .map_err(|error| map_writer_error(error, DavMultiStatusProgress::default()))?;
        if let Err(error) = writer.start_element("D:multistatus", [("xmlns:D", DAV_NAMESPACE)]) {
            return Err(map_writer_error(error, writer_progress(&writer, 0)));
        }
        let mut inherited_namespaces = BTreeMap::new();
        inherited_namespaces.insert("D".to_owned(), DAV_NAMESPACE.to_owned());
        Ok(Self {
            writer,
            inherited_namespaces,
            limits,
            emitted_items: 0,
        })
    }

    pub fn append(&mut self, item: DavMultiStatusItem) -> Result<(), DavMultiStatusError> {
        let next_items = self
            .emitted_items
            .checked_add(1)
            .ok_or_else(|| self.error(DavMultiStatusErrorKind::ItemLimitExceeded))?;
        if next_items > self.limits.maximum_items {
            return Err(self.error(DavMultiStatusErrorKind::ItemLimitExceeded));
        }
        validate_item(&item, self.limits.maximum_properties_per_item)
            .map_err(|kind| self.error(kind))?;

        if let Err(error) = write_response_item(&mut self.writer, &self.inherited_namespaces, item)
        {
            return Err(map_writer_error(error, self.progress()));
        }
        self.emitted_items = next_items;
        Ok(())
    }

    /// Returns the number of bytes successfully written to the underlying sink.
    #[must_use]
    pub fn written_bytes(&self) -> usize {
        self.writer.get_ref().written
    }

    /// Returns a mutable reference to the underlying sink without finishing the document.
    ///
    /// Writing to the sink directly bypasses XML state and byte accounting and can corrupt the
    /// document. This access is intended only for draining sink-managed completed chunks.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.writer.get_mut().inner
    }

    pub fn finish(mut self) -> Result<W, DavMultiStatusError> {
        if let Err(error) = self.writer.end_element() {
            return Err(map_writer_error(error, self.progress()));
        }
        let progress = self.progress();
        self.writer
            .finish()
            .map(|tracking| tracking.inner)
            .map_err(|error| map_writer_error(error, progress))
    }

    fn progress(&self) -> DavMultiStatusProgress {
        writer_progress(&self.writer, self.emitted_items)
    }

    fn error(&self, kind: DavMultiStatusErrorKind) -> DavMultiStatusError {
        DavMultiStatusError::new(kind, self.progress())
    }
}

/// Serializes a complete bounded Multi-Status document through the incremental writer contract.
pub fn dav_multistatus_bytes(
    items: impl IntoIterator<Item = DavMultiStatusItem>,
    limits: DavMultiStatusLimits,
) -> Result<Vec<u8>, DavMultiStatusError> {
    let mut writer = DavMultiStatusWriter::new(Vec::new(), limits)?;
    for item in items {
        writer.append(item)?;
    }
    writer.finish()
}

/// Creates a transport-neutral streaming 207 response from a product-owned item stream.
pub fn multistatus_stream_response<S>(
    source: S,
    limits: DavMultiStatusLimits,
) -> Result<DavResponse, DavMultiStatusError>
where
    S: Stream<Item = Result<DavMultiStatusItem, DavMultiStatusSourceError>> + Send + 'static,
{
    limits.validate()?;
    let stream = StreamingMultiStatus::new(Box::pin(source), limits);
    let mut response = DavResponse {
        status: StatusCode::MULTI_STATUS,
        headers: http::HeaderMap::new(),
        body: DavResponseBody::MultiStatus(Box::pin(stream)),
    };
    response.headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    Ok(response)
}

struct StreamingMultiStatus {
    source: Pin<
        Box<
            dyn Stream<Item = Result<DavMultiStatusItem, DavMultiStatusSourceError>>
                + Send
                + 'static,
        >,
    >,
    writer: Option<DavMultiStatusWriter<ChunkBuffer>>,
    pending: VecDeque<Bytes>,
    limits: DavMultiStatusLimits,
    progress: DavMultiStatusProgress,
    done: bool,
}

impl StreamingMultiStatus {
    fn new(
        source: Pin<
            Box<
                dyn Stream<Item = Result<DavMultiStatusItem, DavMultiStatusSourceError>>
                    + Send
                    + 'static,
            >,
        >,
        limits: DavMultiStatusLimits,
    ) -> Self {
        Self {
            source,
            writer: None,
            pending: VecDeque::new(),
            limits,
            progress: DavMultiStatusProgress::default(),
            done: false,
        }
    }

    fn new_writer(&self) -> Result<DavMultiStatusWriter<ChunkBuffer>, DavMultiStatusError> {
        let buffer = ChunkBuffer::new(
            self.limits
                .chunk_bytes
                .min(self.limits.maximum_output_bytes),
        );
        DavMultiStatusWriter::new(buffer, self.limits)
    }

    fn fail(
        &mut self,
        kind: DavMultiStatusErrorKind,
    ) -> Poll<Option<Result<Bytes, DavMultiStatusError>>> {
        self.done = true;
        self.pending.clear();
        self.writer = None;
        Poll::Ready(Some(Err(DavMultiStatusError::new(kind, self.progress))))
    }
}

impl Stream for StreamingMultiStatus {
    type Item = Result<Bytes, DavMultiStatusError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(chunk) = self.pending.pop_front() {
                self.progress.response_started = true;
                self.progress.emitted_bytes =
                    self.progress.emitted_bytes.saturating_add(chunk.len());
                return Poll::Ready(Some(Ok(chunk)));
            }
            if self.done {
                return Poll::Ready(None);
            }

            match self.source.as_mut().poll_next(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Err(DavMultiStatusSourceError::Backend(error)))) => {
                    return self.fail(DavMultiStatusErrorKind::Backend(error));
                }
                Poll::Ready(Some(Err(DavMultiStatusSourceError::Cancelled))) => {
                    return self.fail(DavMultiStatusErrorKind::Cancelled);
                }
                Poll::Ready(Some(Ok(item))) => {
                    let mut writer = match self.writer.take() {
                        Some(writer) => writer,
                        None => match self.new_writer() {
                            Ok(writer) => writer,
                            Err(mut error) => {
                                error.progress = self.progress;
                                self.done = true;
                                return Poll::Ready(Some(Err(error)));
                            }
                        },
                    };
                    let result = writer.append(item);
                    if let Err(mut error) = result {
                        error.progress.response_started = self.progress.response_started;
                        error.progress.emitted_bytes = self.progress.emitted_bytes;
                        self.done = true;
                        self.writer = None;
                        return Poll::Ready(Some(Err(error)));
                    }
                    self.progress.emitted_items = self.progress.emitted_items.saturating_add(1);
                    self.pending.append(&mut writer.get_mut().take_chunks());
                    self.writer = Some(writer);
                }
                Poll::Ready(None) => {
                    let writer = match self.writer.take() {
                        Some(writer) => writer,
                        None => match self.new_writer() {
                            Ok(writer) => writer,
                            Err(mut error) => {
                                error.progress = self.progress;
                                self.done = true;
                                return Poll::Ready(Some(Err(error)));
                            }
                        },
                    };
                    match writer.finish() {
                        Ok(mut buffer) => {
                            self.pending.append(&mut buffer.take_chunks());
                            self.done = true;
                        }
                        Err(mut error) => {
                            error.progress.response_started = self.progress.response_started;
                            error.progress.emitted_bytes = self.progress.emitted_bytes;
                            self.done = true;
                            return Poll::Ready(Some(Err(error)));
                        }
                    }
                }
            }
        }
    }
}

fn validate_item(
    item: &DavMultiStatusItem,
    maximum_properties: usize,
) -> Result<(), DavMultiStatusErrorKind> {
    let property_count = item.propstats.iter().try_fold(0usize, |count, propstat| {
        count.checked_add(propstat.properties.len())
    });
    if property_count.is_none_or(|count| count > maximum_properties) {
        return Err(DavMultiStatusErrorKind::PropertyLimitExceeded);
    }
    if item
        .status
        .is_some_and(|status| StatusCode::from_u16(status).is_err())
        || item
            .propstats
            .iter()
            .any(|propstat| StatusCode::from_u16(propstat.status).is_err())
    {
        return Err(DavMultiStatusErrorKind::InvalidItem);
    }
    let property_form = item.status.is_none() && !item.propstats.is_empty();
    let status_form = item.status.is_some() && item.propstats.is_empty();
    if item.href.is_empty() || !(property_form || status_form) {
        return Err(DavMultiStatusErrorKind::InvalidItem);
    }
    Ok(())
}

fn write_response_item<W: Write>(
    writer: &mut XmlStreamWriter<TrackingWriter<W>>,
    inherited_namespaces: &BTreeMap<String, String>,
    item: DavMultiStatusItem,
) -> Result<(), ForgeXmlError> {
    writer.start("D:response")?;
    write_text_element(writer, "D:href", &item.href)?;
    for propstat in item.propstats {
        write_propstat(writer, inherited_namespaces, propstat)?;
    }
    if let Some(status) = item.status {
        write_status(writer, status)?;
    }
    if let Some(error) = item.error {
        write_error(writer, &error)?;
    }
    writer.end_element()
}

fn write_propstat<W: Write>(
    writer: &mut XmlStreamWriter<TrackingWriter<W>>,
    inherited_namespaces: &BTreeMap<String, String>,
    propstat: DavPropStat,
) -> Result<(), ForgeXmlError> {
    writer.start("D:propstat")?;
    writer.start("D:prop")?;
    for property in &propstat.properties {
        write_element(writer, property, inherited_namespaces)?;
    }
    writer.end_element()?;
    write_status(writer, propstat.status)?;
    writer.end_element()
}

fn write_status<W: Write>(
    writer: &mut XmlStreamWriter<TrackingWriter<W>>,
    status: u16,
) -> Result<(), ForgeXmlError> {
    let status = StatusCode::from_u16(status)
        .map_err(|_| ForgeXmlError::InvalidData("invalid HTTP status code".to_owned()))?;
    let line = format!(
        "HTTP/1.1 {} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("Unknown"),
    );
    write_text_element(writer, "D:status", &line)
}

fn write_error<W: Write>(
    writer: &mut XmlStreamWriter<TrackingWriter<W>>,
    error: &DavErrorCondition,
) -> Result<(), ForgeXmlError> {
    let (name, href) = error_condition_parts(error);
    writer.start("D:error")?;
    if let Some(href) = href {
        writer.start(&format!("D:{name}"))?;
        write_text_element(writer, "D:href", href)?;
        writer.end_element()?;
    } else {
        writer.empty(&format!("D:{name}"))?;
    }
    writer.end_element()
}

fn write_text_element<W: Write>(
    writer: &mut XmlStreamWriter<TrackingWriter<W>>,
    name: &str,
    text: &str,
) -> Result<(), ForgeXmlError> {
    writer.start(name)?;
    writer.text(text)?;
    writer.end_element()
}

fn writer_progress<W: Write>(
    writer: &XmlStreamWriter<TrackingWriter<W>>,
    emitted_items: usize,
) -> DavMultiStatusProgress {
    let emitted_bytes = writer.get_ref().written;
    DavMultiStatusProgress {
        response_started: emitted_bytes != 0,
        emitted_items,
        emitted_bytes,
    }
}

fn map_writer_error(error: ForgeXmlError, progress: DavMultiStatusProgress) -> DavMultiStatusError {
    let kind = match error {
        ForgeXmlError::Safety(XmlSafetyError::OutputTooLarge) => {
            DavMultiStatusErrorKind::OutputLimitExceeded
        }
        ForgeXmlError::Safety(_) => DavMultiStatusErrorKind::Xml,
        ForgeXmlError::InvalidXml(_) | ForgeXmlError::InvalidData(_) => {
            DavMultiStatusErrorKind::Xml
        }
        ForgeXmlError::Io(_) => DavMultiStatusErrorKind::Write,
    };
    DavMultiStatusError::new(kind, progress)
}

struct TrackingWriter<W> {
    inner: W,
    written: usize,
}

impl<W> TrackingWriter<W> {
    const fn new(inner: W) -> Self {
        Self { inner, written: 0 }
    }
}

impl<W: Write> Write for TrackingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.written = self
            .written
            .checked_add(written)
            .ok_or_else(|| io::Error::other("Multi-Status byte count overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct ChunkBuffer {
    chunk_bytes: usize,
    current: BytesMut,
    ready: VecDeque<Bytes>,
}

impl ChunkBuffer {
    fn new(chunk_bytes: usize) -> Self {
        Self {
            chunk_bytes,
            current: BytesMut::with_capacity(chunk_bytes.min(DEFAULT_CHUNK_BYTES)),
            ready: VecDeque::new(),
        }
    }

    fn take_chunks(&mut self) -> VecDeque<Bytes> {
        if !self.current.is_empty() {
            self.ready.push_back(self.current.split().freeze());
        }
        std::mem::take(&mut self.ready)
    }
}

impl Write for ChunkBuffer {
    fn write(&mut self, mut buffer: &[u8]) -> io::Result<usize> {
        let input_len = buffer.len();
        while !buffer.is_empty() {
            let remaining = self.chunk_bytes - self.current.len();
            let take = remaining.min(buffer.len());
            self.current.extend_from_slice(&buffer[..take]);
            buffer = &buffer[take..];
            if self.current.len() == self.chunk_bytes {
                self.ready.push_back(self.current.split().freeze());
            }
        }
        Ok(input_len)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_writer_rejects_an_unvalidated_invalid_code() {
        let tracking = TrackingWriter::new(Vec::new());
        let mut writer = XmlStreamWriter::new(tracking).expect("writer");
        writer.start("root").expect("root");
        assert!(matches!(
            write_status(&mut writer, 99),
            Err(ForgeXmlError::InvalidData(_))
        ));
    }
}
