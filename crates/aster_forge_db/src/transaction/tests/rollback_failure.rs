//! Inject a definite rollback error through `SeaORM`'s transaction boundary.
//! `SQLx` 0.9.1 now recognizes SQL ROLLBACK statements and treats a later rollback
//! as success, so raw SQL is no longer a reliable failure injection mechanism.
use sea_orm::{
    AccessMode, ConnectionTrait, DbBackend, DbErr, ExecResult, IsolationLevel, QueryResult,
    Statement, TransactionError, TransactionOptions, TransactionSession, TransactionTrait,
};
use std::{future::Future, pin::Pin};

pub(super) struct FailingRollback;

#[async_trait::async_trait]
impl ConnectionTrait for FailingRollback {
    fn get_database_backend(&self) -> DbBackend {
        DbBackend::Sqlite
    }
    async fn execute_raw(&self, _: Statement) -> Result<ExecResult, DbErr> {
        Err(DbErr::Custom("unexpected execute".into()))
    }
    async fn execute_unprepared(&self, _: &str) -> Result<ExecResult, DbErr> {
        Err(DbErr::Custom("unexpected execute".into()))
    }
    async fn query_one_raw(&self, _: Statement) -> Result<Option<QueryResult>, DbErr> {
        Err(DbErr::Custom("unexpected query".into()))
    }
    async fn query_all_raw(&self, _: Statement) -> Result<Vec<QueryResult>, DbErr> {
        Err(DbErr::Custom("unexpected query".into()))
    }
}

#[async_trait::async_trait]
impl TransactionSession for FailingRollback {
    async fn commit(self) -> Result<(), DbErr> {
        Err(DbErr::Custom("unexpected commit".into()))
    }
    async fn rollback(self) -> Result<(), DbErr> {
        Err(DbErr::Custom("secret-rollback-canary".into()))
    }
}

#[async_trait::async_trait]
impl TransactionTrait for FailingRollback {
    type Transaction = Self;
    async fn begin(&self) -> Result<Self, DbErr> {
        Ok(Self)
    }
    async fn begin_with_config(
        &self,
        _: Option<IsolationLevel>,
        _: Option<AccessMode>,
    ) -> Result<Self, DbErr> {
        Ok(Self)
    }
    async fn begin_with_options(&self, _: TransactionOptions) -> Result<Self, DbErr> {
        Ok(Self)
    }
    async fn transaction<F, T, E>(&self, _: F) -> Result<T, TransactionError<E>>
    where
        F: for<'c> FnOnce(&'c Self) -> Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'c>>
            + Send,
        T: Send,
        E: std::fmt::Display + std::fmt::Debug + Send,
    {
        Err(TransactionError::Connection(DbErr::Custom(
            "unexpected nested transaction".into(),
        )))
    }
    async fn transaction_with_config<F, T, E>(
        &self,
        _: F,
        _: Option<IsolationLevel>,
        _: Option<AccessMode>,
    ) -> Result<T, TransactionError<E>>
    where
        F: for<'c> FnOnce(&'c Self) -> Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'c>>
            + Send,
        T: Send,
        E: std::fmt::Display + std::fmt::Debug + Send,
    {
        Err(TransactionError::Connection(DbErr::Custom(
            "unexpected nested transaction".into(),
        )))
    }
}
