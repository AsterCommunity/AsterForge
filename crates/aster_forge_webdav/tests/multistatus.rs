use std::io::{self, Write};
use std::task::Poll;

use aster_forge_webdav::{
    DavBackendError, DavBackendErrorKind, DavErrorCondition, DavMultiStatusErrorKind,
    DavMultiStatusItem, DavMultiStatusLimits, DavMultiStatusSourceError, DavMultiStatusWriter,
    DavPropStat, DavResponseBody, DavXmlElement, DavXmlNode, dav_element, dav_multistatus_bytes,
    multistatus_stream_response,
};
use bytes::Bytes;
use futures::{StreamExt, stream};

fn property_item(href: &str, property_count: usize) -> DavMultiStatusItem {
    DavMultiStatusItem::properties(
        href,
        vec![DavPropStat {
            status: 200,
            properties: (0..property_count)
                .map(|index| dav_element(&format!("property-{index}")))
                .collect(),
        }],
    )
}

fn status_item(href: &str, status: u16) -> DavMultiStatusItem {
    DavMultiStatusItem::status(href, status)
}

#[test]
fn complete_writer_preserves_item_and_property_order_with_one_grammar() {
    let bytes = dav_multistatus_bytes(
        [
            property_item("/dav/a", 2).with_error(DavErrorCondition::PropfindFiniteDepth),
            status_item("/dav/b", 404),
        ],
        DavMultiStatusLimits::default(),
    )
    .expect("complete Multi-Status");
    let xml = String::from_utf8(bytes).expect("UTF-8 XML");
    assert!(xml.starts_with("<D:multistatus xmlns:D=\"DAV:\">"), "{xml}");
    assert!(xml.ends_with("</D:multistatus>"), "{xml}");
    assert_eq!(xml.matches("<D:response>").count(), 2, "{xml}");
    assert!(xml.contains("propfind-finite-depth"), "{xml}");
    assert!(
        xml.find("property-0").expect("first property")
            < xml.find("property-1").expect("second property"),
        "{xml}"
    );
    assert!(
        xml.find("/dav/a").expect("first href") < xml.find("/dav/b").expect("second href"),
        "{xml}"
    );
    DavXmlElement::parse(xml.as_bytes()).expect("generated XML should reparse");
}

#[test]
fn writer_exposes_exact_progress_and_sink_access() {
    let mut writer =
        DavMultiStatusWriter::new(Vec::new(), DavMultiStatusLimits::default()).expect("writer");
    assert_eq!(writer.written_bytes(), writer.get_mut().len());
    assert!(writer.written_bytes() > 0);
    writer
        .append(status_item("/dav/a", 404))
        .expect("response item");
    assert_eq!(writer.written_bytes(), writer.get_mut().len());
}

#[test]
fn complete_writer_enforces_exact_item_property_and_byte_limits() {
    let items = [property_item("/dav/a", 2), status_item("/dav/b", 404)];
    let baseline = dav_multistatus_bytes(items.clone(), DavMultiStatusLimits::default())
        .expect("baseline bytes");
    let exact = DavMultiStatusLimits::new(baseline.len(), 2, 2, 16);
    assert_eq!(
        dav_multistatus_bytes(items.clone(), exact).expect("exact limits"),
        baseline
    );

    let item_error = dav_multistatus_bytes(
        items.clone(),
        DavMultiStatusLimits::new(baseline.len(), 1, 2, 16),
    )
    .expect_err("second item should exceed the limit");
    assert_eq!(item_error.kind, DavMultiStatusErrorKind::ItemLimitExceeded);
    assert_eq!(item_error.progress.emitted_items, 1);
    assert!(item_error.progress.response_started);

    let property_error = dav_multistatus_bytes(
        items.clone(),
        DavMultiStatusLimits::new(baseline.len(), 2, 1, 16),
    )
    .expect_err("second property should exceed the limit");
    assert_eq!(
        property_error.kind,
        DavMultiStatusErrorKind::PropertyLimitExceeded
    );
    assert_eq!(property_error.progress.emitted_items, 0);

    let byte_error = dav_multistatus_bytes(
        items,
        DavMultiStatusLimits::new(baseline.len() - 1, 2, 2, 16),
    )
    .expect_err("one byte below the exact document size should fail");
    assert_eq!(
        byte_error.kind,
        DavMultiStatusErrorKind::OutputLimitExceeded
    );
}

#[test]
fn complete_writer_rejects_zero_limits_and_invalid_response_forms() {
    for limits in [
        DavMultiStatusLimits::new(0, 1, 1, 1),
        DavMultiStatusLimits::new(1, 0, 1, 1),
        DavMultiStatusLimits::new(1, 1, 0, 1),
        DavMultiStatusLimits::new(1, 1, 1, 0),
    ] {
        assert_eq!(
            dav_multistatus_bytes([], limits)
                .expect_err("zero limit should be rejected")
                .kind,
            DavMultiStatusErrorKind::InvalidLimits
        );
    }

    let invalid_items = [
        DavMultiStatusItem::properties(
            "",
            vec![DavPropStat {
                status: 200,
                properties: vec![dav_element("displayname")],
            }],
        ),
        DavMultiStatusItem::properties("/dav/empty", Vec::new()),
        DavMultiStatusItem {
            href: "/dav/mixed".to_owned(),
            status: Some(409),
            propstats: vec![DavPropStat {
                status: 200,
                properties: vec![dav_element("displayname")],
            }],
            error: None,
        },
    ];
    for item in invalid_items {
        assert_eq!(
            dav_multistatus_bytes([item], DavMultiStatusLimits::default())
                .expect_err("invalid response form")
                .kind,
            DavMultiStatusErrorKind::InvalidItem
        );
    }
}

#[test]
fn complete_writer_maps_real_xml_data_and_depth_failures() {
    let invalid_text = dav_multistatus_bytes(
        [status_item("/dav/\0", 404)],
        DavMultiStatusLimits::default(),
    )
    .expect_err("XML 1.0 forbids NUL text");
    assert_eq!(invalid_text.kind, DavMultiStatusErrorKind::Xml);

    let mut property = dav_element("leaf");
    for _ in 0..130 {
        let mut parent = dav_element("nested");
        parent.children.push(DavXmlNode::Element(property));
        property = parent;
    }
    let too_deep = dav_multistatus_bytes(
        [DavMultiStatusItem::properties(
            "/dav/deep",
            vec![DavPropStat {
                status: 200,
                properties: vec![property],
            }],
        )],
        DavMultiStatusLimits::default(),
    )
    .expect_err("nested property should cross the XML writer depth limit");
    assert_eq!(too_deep.kind, DavMultiStatusErrorKind::Xml);
}

#[derive(Debug)]
struct FailAfter {
    remaining: usize,
    bytes: Vec<u8>,
    fail_flush: bool,
}

impl FailAfter {
    const fn new(remaining: usize) -> Self {
        Self {
            remaining,
            bytes: Vec::new(),
            fail_flush: false,
        }
    }
}

impl Write for FailAfter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::other("injected write failure"));
        }
        let written = self.remaining.min(buffer.len());
        self.bytes.extend_from_slice(&buffer[..written]);
        self.remaining -= written;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.fail_flush {
            Err(io::Error::other("injected flush failure"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn writer_reports_sink_failure_before_and_after_response_start() {
    let before = DavMultiStatusWriter::new(FailAfter::new(0), DavMultiStatusLimits::default())
        .err()
        .expect("root write should fail");
    assert_eq!(before.kind, DavMultiStatusErrorKind::Write);
    assert!(!before.progress.response_started);
    assert_eq!(before.progress.emitted_bytes, 0);

    let root_bytes = b"<D:multistatus xmlns:D=\"DAV:\">".len();
    let mut writer = DavMultiStatusWriter::new(
        FailAfter::new(root_bytes + 8),
        DavMultiStatusLimits::default(),
    )
    .expect("root should fit");
    let after = writer
        .append(status_item("/dav/a", 404))
        .expect_err("response write should fail after the root");
    assert_eq!(after.kind, DavMultiStatusErrorKind::Write);
    assert!(after.progress.response_started);
    assert!(after.progress.emitted_bytes >= root_bytes);

    let mut sink = FailAfter::new(usize::MAX);
    sink.fail_flush = true;
    let writer = DavMultiStatusWriter::new(sink, DavMultiStatusLimits::default())
        .expect("root should write");
    let flush = writer.finish().expect_err("flush should fail");
    assert_eq!(flush.kind, DavMultiStatusErrorKind::Write);
    assert!(flush.progress.response_started);
}

#[test]
fn stream_emits_bounded_chunks_and_valid_empty_or_populated_documents() {
    futures::executor::block_on(async {
        for items in [Vec::new(), vec![property_item("/dav/a", 2)]] {
            let response = multistatus_stream_response(
                stream::iter(items.into_iter().map(Ok)),
                DavMultiStatusLimits::new(4096, 4, 4, 7),
            )
            .expect("stream response");
            let DavResponseBody::MultiStatus(mut body) = response.body else {
                panic!("expected Multi-Status stream");
            };
            let mut chunks = Vec::new();
            while let Some(chunk) = body.next().await {
                let chunk = chunk.expect("stream chunk");
                assert!(!chunk.is_empty());
                assert!(chunk.len() <= 7, "chunk was {} bytes", chunk.len());
                chunks.push(chunk);
            }
            let bytes = chunks.concat();
            let xml = String::from_utf8(bytes).expect("UTF-8 XML");
            assert!(xml.starts_with("<D:multistatus"), "{xml}");
            assert!(xml.ends_with("</D:multistatus>"), "{xml}");
            DavXmlElement::parse(xml.as_bytes()).expect("streamed XML should reparse");
        }
    });
}

#[test]
fn stream_preserves_pending_and_output_limit_boundaries() {
    futures::executor::block_on(async {
        let mut first_poll = true;
        let pending_once = stream::poll_fn(move |_| {
            if std::mem::take(&mut first_poll) {
                Poll::Pending
            } else {
                Poll::Ready(None)
            }
        });
        let response =
            multistatus_stream_response(pending_once, DavMultiStatusLimits::new(4096, 1, 1, 64))
                .expect("response shell");
        let DavResponseBody::MultiStatus(mut body) = response.body else {
            panic!("expected Multi-Status stream");
        };
        assert!(matches!(futures::poll!(body.next()), Poll::Pending));
        assert!(body.next().await.expect("root chunk").is_ok());

        let root_bytes = b"<D:multistatus xmlns:D=\"DAV:\">".len();
        for source in [
            stream::iter(Vec::<Result<DavMultiStatusItem, DavMultiStatusSourceError>>::new()),
            stream::iter(vec![Ok(status_item("/dav/a", 404))]),
        ] {
            let response = multistatus_stream_response(
                source,
                DavMultiStatusLimits::new(root_bytes - 1, 1, 1, 64),
            )
            .expect("response shell");
            let DavResponseBody::MultiStatus(mut body) = response.body else {
                panic!("expected Multi-Status stream");
            };
            let error = body
                .next()
                .await
                .expect("output limit error")
                .expect_err("root should exceed output limit");
            assert_eq!(error.kind, DavMultiStatusErrorKind::OutputLimitExceeded);
            assert!(!error.progress.response_started);
            assert_eq!(error.progress.emitted_bytes, 0);
        }

        let response = multistatus_stream_response(
            stream::empty::<Result<DavMultiStatusItem, DavMultiStatusSourceError>>(),
            DavMultiStatusLimits::new(root_bytes - 1, 1, 1, usize::MAX),
        )
        .expect("response shell");
        let DavResponseBody::MultiStatus(mut body) = response.body else {
            panic!("expected Multi-Status stream");
        };
        assert_eq!(
            body.next()
                .await
                .expect("output limit error")
                .expect_err("root should exceed output limit")
                .kind,
            DavMultiStatusErrorKind::OutputLimitExceeded
        );

        let response = multistatus_stream_response(
            stream::empty(),
            DavMultiStatusLimits::new(root_bytes, 1, 1, 64),
        )
        .expect("response shell");
        let DavResponseBody::MultiStatus(mut body) = response.body else {
            panic!("expected Multi-Status stream");
        };
        let before_start = body
            .next()
            .await
            .expect("closing-tag limit error")
            .expect_err("closing tag should cross the limit");
        assert_eq!(
            before_start.kind,
            DavMultiStatusErrorKind::OutputLimitExceeded
        );
        assert!(!before_start.progress.response_started);

        let item = status_item("/dav/a", 404);
        let complete = dav_multistatus_bytes([item.clone()], DavMultiStatusLimits::default())
            .expect("complete baseline");
        let maximum_without_closing = complete.len() - b"</D:multistatus>".len();
        let response = multistatus_stream_response(
            stream::iter([Ok(item)]),
            DavMultiStatusLimits::new(maximum_without_closing, 1, 1, 16),
        )
        .expect("response shell");
        let DavResponseBody::MultiStatus(mut body) = response.body else {
            panic!("expected Multi-Status stream");
        };
        let after_start = loop {
            match body.next().await.expect("chunk or output limit error") {
                Ok(chunk) => assert!(!chunk.is_empty()),
                Err(error) => break error,
            }
        };
        assert_eq!(
            after_start.kind,
            DavMultiStatusErrorKind::OutputLimitExceeded
        );
        assert!(after_start.progress.response_started);
        assert_eq!(after_start.progress.emitted_items, 1);
        assert!(after_start.progress.emitted_bytes > 0);
    });
}

#[test]
fn stream_classifies_source_failure_and_cancellation_before_or_after_start() {
    futures::executor::block_on(async {
        let before_source = stream::iter([Err(DavMultiStatusSourceError::Backend(
            DavBackendError::new(DavBackendErrorKind::Forbidden),
        ))]);
        let response = multistatus_stream_response(before_source, DavMultiStatusLimits::default())
            .expect("response shell");
        let DavResponseBody::MultiStatus(mut body) = response.body else {
            panic!("expected Multi-Status stream");
        };
        let before = body
            .next()
            .await
            .expect("source error")
            .expect_err("backend failure");
        assert_eq!(
            before.kind,
            DavMultiStatusErrorKind::Backend(DavBackendError::new(DavBackendErrorKind::Forbidden))
        );
        assert!(!before.progress.response_started);
        assert!(body.next().await.is_none());

        let after_source = stream::iter([
            Ok(property_item("/dav/a", 1)),
            Err(DavMultiStatusSourceError::Cancelled),
        ]);
        let response =
            multistatus_stream_response(after_source, DavMultiStatusLimits::new(4096, 4, 4, 32))
                .expect("response shell");
        let DavResponseBody::MultiStatus(mut body) = response.body else {
            panic!("expected Multi-Status stream");
        };
        let first = body.next().await.expect("first chunk").expect("first item");
        assert!(!first.is_empty());
        let cancelled = loop {
            match body.next().await.expect("cancellation item") {
                Ok(_) => {}
                Err(error) => break error,
            }
        };
        assert_eq!(cancelled.kind, DavMultiStatusErrorKind::Cancelled);
        assert!(cancelled.progress.response_started);
        assert_eq!(cancelled.progress.emitted_items, 1);
        assert!(body.next().await.is_none());
    });
}

#[test]
fn stream_does_not_emit_partial_current_item_when_a_limit_fails() {
    futures::executor::block_on(async {
        let source = stream::iter([
            Ok(status_item("/dav/a", 404)),
            Ok(property_item("/dav/b", 2)),
        ]);
        let response =
            multistatus_stream_response(source, DavMultiStatusLimits::new(4096, 2, 1, 4096))
                .expect("response shell");
        let DavResponseBody::MultiStatus(mut body) = response.body else {
            panic!("expected Multi-Status stream");
        };
        let first = body.next().await.expect("first chunk").expect("first item");
        let first = String::from_utf8(first.to_vec()).expect("UTF-8 chunk");
        assert!(first.contains("/dav/a"), "{first}");
        assert!(!first.contains("/dav/b"), "{first}");
        let error = body
            .next()
            .await
            .expect("limit error")
            .expect_err("property limit should stop the stream");
        assert_eq!(error.kind, DavMultiStatusErrorKind::PropertyLimitExceeded);
        assert!(error.progress.response_started);
        assert_eq!(error.progress.emitted_items, 1);
    });
}

#[test]
fn response_stream_accepts_static_byte_values_without_extra_copy_contracts() {
    futures::executor::block_on(async {
        let source = stream::iter([Ok(status_item("/dav/a", 404))]);
        let response = multistatus_stream_response(source, DavMultiStatusLimits::default())
            .expect("response shell");
        let DavResponseBody::MultiStatus(mut body) = response.body else {
            panic!("expected Multi-Status stream");
        };
        let chunks = body.by_ref().collect::<Vec<_>>().await;
        let bytes = chunks
            .into_iter()
            .collect::<Result<Vec<Bytes>, _>>()
            .expect("all chunks")
            .concat();
        assert!(
            bytes
                .windows(b"/dav/a".len())
                .any(|window| window == b"/dav/a")
        );
    });
}
