use sea_orm::{Database, DatabaseConnection};
use std::time::Duration;

pub(crate) async fn connect_with_retry(database_url: &str, service: &str) -> DatabaseConnection {
    let deadline = tokio::time::Instant::now() + Duration::from_mins(1);
    loop {
        match Database::connect(database_url).await {
            Ok(database) => return database,
            Err(error) if tokio::time::Instant::now() >= deadline => {
                panic!("{service} test database did not become ready: {error}")
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
}
