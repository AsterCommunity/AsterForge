//! Native pool sampling without acquisition, ping, or fictitious readers.
use aster_forge_db::{DatabaseConfig, DbHandles, connect, connect_reader_for_writer};
use aster_forge_metrics::{DbMetricsRecorder, DbQueryMetric, MetricsRecorder};
use aster_forge_test::temp::SqliteTestDatabase;
use sea_orm::{DatabaseConnection, TransactionTrait};
use std::sync::Mutex;

#[derive(Debug, PartialEq)]
struct Sample(String, u32, u32, Option<u32>);

#[derive(Default)]
struct Capture(Mutex<Vec<Sample>>);
impl DbMetricsRecorder for Capture {
    fn enabled(&self) -> bool {
        true
    }
    fn record_db_query(&self, _: &DbQueryMetric) {
        panic!("pool sampling must not execute a query");
    }
}
impl MetricsRecorder for Capture {
    fn db_pool(&self, name: &str, idle: u32, used: u32, max: Option<u32>) {
        self.0
            .lock()
            .unwrap()
            .push(Sample(name.to_owned(), idle, used, max));
    }
}

#[test]
fn disconnected_handles_emit_no_fictitious_zero_pool() {
    let handles = DbHandles::single(DatabaseConnection::default());
    let recorder = Capture::default();
    handles.record_pool_metrics("main", &recorder);
    assert!(recorder.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn independent_handle_groups_keep_distinct_names_on_one_recorder() {
    let main = DbHandles::single(sea_orm::Database::connect("sqlite::memory:").await.unwrap());
    let audit = DbHandles::single(sea_orm::Database::connect("sqlite::memory:").await.unwrap());
    let recorder = Capture::default();
    main.record_pool_metrics("main", &recorder);
    audit.record_pool_metrics("audit", &recorder);
    {
        let samples = recorder.0.lock().unwrap();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].0, "main.writer");
        assert_eq!(samples[1].0, "audit.writer");
    }
    main.close().await.unwrap();
    audit.close().await.unwrap();
}

#[tokio::test]
async fn lazy_postgres_and_mysql_pools_are_sampled_without_connecting() {
    // Port zero is deliberately unusable. min_connections=0 keeps these real
    // native pools disconnected; metrics must read their local state only.
    let postgres = sea_orm::sqlx::postgres::PgPoolOptions::new()
        .min_connections(0)
        .max_connections(7)
        .connect_lazy("postgres://unused@127.0.0.1:0/unused")
        .unwrap();
    let mysql = sea_orm::sqlx::mysql::MySqlPoolOptions::new()
        .min_connections(0)
        .max_connections(9)
        .connect_lazy("mysql://unused@127.0.0.1:0/unused")
        .unwrap();
    for (db, maximum) in [
        (DatabaseConnection::from(postgres), 7),
        (DatabaseConnection::from(mysql), 9),
    ] {
        let handles = DbHandles::single(db);
        let recorder = Capture::default();
        handles.record_pool_metrics("main", &recorder);
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![Sample("main.writer".into(), 0, 0, Some(maximum))]
        );
        handles.close().await.unwrap();
    }
}

#[tokio::test]
async fn split_sqlite_pools_report_held_writer_and_independent_reader_limits() {
    let database = SqliteTestDatabase::new("metrics-split-pools");
    let config = DatabaseConfig {
        url: database.url().into(),
        pool_size: 3,
        retry_count: 0,
    };
    let writer = connect(&config).await.unwrap();
    let handles = connect_reader_for_writer(&config, writer).await.unwrap();
    assert!(handles.sqlite_read_write_split());
    let transaction = handles.writer().begin().await.unwrap();
    // SQLx may return from pool construction while a background connection is still being
    // returned. Establish the fixture's idle reader state before asserting its sampled usage.
    // This preparation reads native counters only; it never acquires or pings a connection.
    let reader = handles.reader().get_sqlite_connection_pool();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while reader.size() == 0 || u32::try_from(reader.num_idle()).unwrap() != reader.size() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("reader fixture should finish returning its initial connection");
    let recorder = Capture::default();
    // With the only writer held, any acquisition by the sampler would block.
    handles.record_pool_metrics("main", &recorder);
    {
        let samples = recorder.0.lock().unwrap();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0], Sample("main.writer".into(), 0, 1, Some(1)));
        assert_eq!(samples[1].0, "main.reader");
        assert_eq!(samples[1].3, Some(3));
        assert_eq!(samples[1].2, 0);
        assert!(samples[1].1 <= 3);
    }
    transaction.rollback().await.unwrap();
    handles.close().await.unwrap();
}
