//! Strict outbox log assertions run in their own test binary so unrelated
//! subscriber registration and callsite-interest rebuilds cannot affect capture.
use aster_forge_mail::{
    DEFAULT_ERROR_MAX_LEN, MailOutboxDispatchConfig, MailOutboxDispatchRow, MailOutboxRetryPolicy,
    dispatch_mail_outbox,
};

struct StoredRow {
    id: i64,
    attempt_count: i32,
    template_code: String,
    to_address: String,
    to_name: Option<String>,
}

impl MailOutboxDispatchRow for StoredRow {
    fn id(&self) -> i64 {
        self.id
    }
    fn attempt_count(&self) -> i32 {
        self.attempt_count
    }
    fn template_code(&self) -> &str {
        &self.template_code
    }
    fn to_address(&self) -> &str {
        &self.to_address
    }
    fn to_name(&self) -> Option<&str> {
        self.to_name.as_deref()
    }
}

#[tokio::test]
async fn historical_rows_cannot_inject_retry_or_permanent_failure_log_records() {
    for attempt_count in [0, 2] {
        let logs = aster_forge_test::logs::CapturedLogs::default();
        let stats = logs
            .capture_future(dispatch_mail_outbox(
                &MailOutboxDispatchConfig::new(
                    20,
                    60,
                    1,
                    MailOutboxRetryPolicy::new(3, DEFAULT_ERROR_MAX_LEN),
                ),
                move |_, _| async move {
                    Ok::<_, String>(vec![StoredRow {
                        id: 7,
                        attempt_count,
                        template_code: "template\nforged".to_string(),
                        to_address: "a\r\nforged@example.com".to_string(),
                        to_name: None,
                    }])
                },
                |_| async { Ok::<_, String>(true) },
                |_| async { Err::<String, _>("transport\nforged\u{1b}[31m".to_string()) },
                |_, _| async { Ok::<_, String>(true) },
                |_, _, _, _| async { Ok::<_, String>(true) },
                |_, _, _| async { Ok::<_, String>(true) },
                |_, _, _| async {},
                |_, _, _| async {},
            ))
            .await
            .unwrap();
        assert_eq!(stats.claimed, 1);
        assert_eq!(stats.sent, 0);
        assert_eq!(stats.retried, usize::from(attempt_count == 0));
        assert_eq!(stats.failed, usize::from(attempt_count == 2));
        let text = logs.text();
        assert_eq!(text.lines().count(), 4, "{text}");
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with(" WARN "))
                .count(),
            1,
            "{text}"
        );
        assert!(text.contains("dispatching due mail outbox rows"), "{text}");
        assert!(text.contains("claimed mail outbox row"), "{text}");
        let expected_failure = if attempt_count == 0 {
            "mail outbox delivery failed; scheduled retry"
        } else {
            "mail outbox delivery permanently failed"
        };
        assert!(text.contains(expected_failure), "{text}");
        assert!(text.contains("a\\r\\nforged@example.com"), "{text}");
        assert!(text.contains("transport\\nforged"), "{text}");
        assert!(text.contains("template\\nforged"), "{text}");
        assert!(
            text.contains("finished dispatching due mail outbox rows"),
            "{text}"
        );
        assert!(!text.contains("template\nforged"), "{text}");
        assert!(!text.contains("a\r\nforged@example.com"), "{text}");
        assert!(!text.contains("transport\nforged"), "{text}");
        assert!(!text.contains('\u{1b}'));
    }
}
