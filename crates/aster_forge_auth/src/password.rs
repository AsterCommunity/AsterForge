use std::sync::Arc;

use aster_forge_crypto::{
    PasswordHashPolicy, PasswordHashVerification, hash_password_with_policy,
    verify_password_with_policy,
};
use tokio::sync::Semaphore;

use aster_forge_crypto::CryptoError;

/// Redacted async password-work errors. Products map these at their own boundary.
#[derive(Debug, thiserror::Error)]
pub enum PasswordWorkError {
    /// A zero-sized executor budget was requested.
    #[error("password work concurrency must be positive")]
    InvalidConcurrency,
    /// No immediate execution permit is available.
    #[error("password work budget exhausted")]
    Busy,
    /// The execution semaphore was closed.
    #[error("password worker is closed")]
    Closed,
    /// The blocking job failed to join.
    #[error("password worker failed")]
    Worker,
    /// The synchronous cryptographic primitive rejected the operation.
    #[error("password cryptographic operation failed")]
    Crypto(#[from] CryptoError),
}
type Result<T> = std::result::Result<T, PasswordWorkError>;

/// Default number of concurrent memory-hard password jobs.
pub const DEFAULT_PASSWORD_HASH_MAX_CONCURRENCY: usize = 2;

/// Instance-owned blocking executor with a memory/CPU concurrency budget.
#[derive(Clone, Debug)]
pub struct PasswordHashRuntime {
    policy: PasswordHashPolicy,
    semaphore: Arc<Semaphore>,
}

impl PasswordHashRuntime {
    /// Creates an executor using the Forge default password policy.
    ///
    /// # Errors
    /// Returns `InvalidConcurrency` if the requested concurrency is zero.
    pub fn new(max_concurrency: usize) -> Result<Self> {
        Self::with_policy(max_concurrency, PasswordHashPolicy::default())
    }

    /// Creates an executor with an explicit policy and positive concurrency limit.
    ///
    /// # Errors
    /// Returns `InvalidConcurrency` if the requested concurrency is zero.
    pub fn with_policy(max_concurrency: usize, policy: PasswordHashPolicy) -> Result<Self> {
        if max_concurrency == 0 {
            return Err(PasswordWorkError::InvalidConcurrency);
        }

        Ok(Self {
            policy,
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
        })
    }

    /// Hashes a password on the blocking pool after acquiring an execution permit.
    ///
    /// # Errors
    /// Returns an error when the worker is closed, fails, or cannot hash the input.
    pub async fn hash_password(&self, password: &str) -> Result<String> {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| PasswordWorkError::Closed)?;
        let password = password.to_owned();
        let policy = self.policy;

        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            hash_password_with_policy(&password, &policy)
        })
        .await
        .map_err(|_| PasswordWorkError::Worker)?
        .map_err(PasswordWorkError::from)
    }

    /// Fail immediately when the CPU/memory budget is occupied. The blocking job
    /// owns the permit, so cancellation cannot release capacity prematurely.
    ///
    /// # Errors
    /// Returns `Busy` on saturation, or a worker/cryptographic error.
    pub async fn try_verify_password(
        &self,
        password: &str,
        stored_hash: &str,
    ) -> Result<PasswordHashVerification> {
        let permit = self
            .semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| PasswordWorkError::Busy)?;
        self.verify_with_permit(password, stored_hash, permit).await
    }

    /// Verifies a password on the blocking pool after acquiring an execution permit.
    ///
    /// # Errors
    /// Returns an error when the worker fails or the stored verifier is invalid.
    pub async fn verify_password(
        &self,
        password: &str,
        stored_hash: &str,
    ) -> Result<PasswordHashVerification> {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| PasswordWorkError::Closed)?;
        self.verify_with_permit(password, stored_hash, permit).await
    }

    async fn verify_with_permit(
        &self,
        password: &str,
        stored_hash: &str,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<PasswordHashVerification> {
        let password = password.to_owned();
        let stored_hash = stored_hash.to_owned();
        let policy = self.policy;

        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            verify_password_with_policy(&password, &stored_hash, &policy)
        })
        .await
        .map_err(|_| PasswordWorkError::Worker)?
        .map_err(PasswordWorkError::from)
    }
}

impl Default for PasswordHashRuntime {
    fn default() -> Self {
        Self {
            policy: PasswordHashPolicy::default(),
            semaphore: Arc::new(Semaphore::new(DEFAULT_PASSWORD_HASH_MAX_CONCURRENCY)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aster_forge_crypto::{
        CryptoError, PasswordHashVerificationLimits, PasswordHashWorkFactor,
        hash_password_with_policy,
    };

    use super::*;

    fn policy(memory_kib: u32) -> PasswordHashPolicy {
        PasswordHashPolicy::new(
            PasswordHashWorkFactor::new(memory_kib, 1, 1, 32).unwrap(),
            PasswordHashVerificationLimits::new(memory_kib, 1, 1, 32).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn immediate_verification_rejects_saturation_and_instances_are_independent() {
        let runtime = PasswordHashRuntime::with_policy(1, policy(8)).unwrap();
        let independent = PasswordHashRuntime::with_policy(1, policy(8)).unwrap();
        let hash = hash_password_with_policy("secret", &policy(8)).unwrap();
        let permit = runtime.semaphore.clone().acquire_owned().await.unwrap();
        assert!(matches!(
            runtime.try_verify_password("secret", &hash).await,
            Err(PasswordWorkError::Busy)
        ));
        assert!(
            independent
                .try_verify_password("secret", &hash)
                .await
                .unwrap()
                .is_valid
        );
        drop(permit);
        assert!(
            runtime
                .try_verify_password("secret", &hash)
                .await
                .unwrap()
                .is_valid
        );
    }

    #[test]
    fn cancelling_a_request_does_not_release_a_queued_blocking_job_permit() {
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        executor.block_on(async {
            let (entered, wait_entered) = tokio::sync::oneshot::channel();
            let (release, gate) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                entered.send(()).unwrap();
                gate.recv().unwrap();
            });
            wait_entered.await.unwrap();
            let runtime = PasswordHashRuntime::with_policy(1, policy(8)).unwrap();
            let hash = hash_password_with_policy("secret", &policy(8)).unwrap();
            let task_runtime = runtime.clone();
            let task_hash = hash.clone();
            let task = tokio::spawn(async move {
                task_runtime.try_verify_password("secret", &task_hash).await
            });
            tokio::time::timeout(Duration::from_secs(1), async {
                while runtime.semaphore.available_permits() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            assert_eq!(runtime.semaphore.available_permits(), 0);
            assert!(matches!(
                runtime.try_verify_password("secret", &hash).await,
                Err(PasswordWorkError::Busy)
            ));
            release.send(()).unwrap();
            blocker.await.unwrap();
            tokio::time::timeout(Duration::from_secs(1), async {
                while runtime.semaphore.available_permits() != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(
                runtime
                    .try_verify_password("secret", &hash)
                    .await
                    .unwrap()
                    .is_valid
            );
        });
    }

    #[test]
    fn rejects_zero_concurrency() {
        let error = PasswordHashRuntime::new(0).unwrap_err();
        assert!(matches!(error, PasswordWorkError::InvalidConcurrency));
    }

    #[test]
    fn production_policy_uses_forge_default_profile() {
        let runtime = PasswordHashRuntime::new(1).unwrap();
        let work_factor = runtime.policy.work_factor();

        assert_eq!(work_factor.memory_kib(), 64 * 1024);
        assert_eq!(work_factor.iterations(), 3);
        assert_eq!(work_factor.parallelism(), 4);
        assert_eq!(work_factor.output_length(), 32);
    }

    #[tokio::test]
    async fn hash_work_waits_for_the_concurrency_permit() {
        let runtime = PasswordHashRuntime::with_policy(1, policy(8)).unwrap();
        let permit = runtime.semaphore.clone().acquire_owned().await.unwrap();
        let runtime_for_task = runtime.clone();
        let mut task =
            tokio::spawn(async move { runtime_for_task.hash_password("bounded-password").await });

        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut task)
                .await
                .is_err()
        );

        drop(permit);
        let hash = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(hash.starts_with("$argon2id$v=19$m=8,t=1,p=1$"));
    }

    #[tokio::test]
    async fn verification_reports_legacy_rehash_and_wrong_password() {
        let current_policy = policy(16);
        let legacy_policy = policy(8);
        let runtime = PasswordHashRuntime::with_policy(1, current_policy).unwrap();
        let legacy_hash = hash_password_with_policy("legacy-password", &legacy_policy).unwrap();

        let verified = runtime
            .verify_password("legacy-password", &legacy_hash)
            .await
            .unwrap();
        assert!(verified.is_valid);
        assert!(verified.needs_rehash);

        let rejected = runtime
            .verify_password("wrong-password", &legacy_hash)
            .await
            .unwrap();
        assert!(!rejected.is_valid);
        assert!(!rejected.needs_rehash);
    }

    #[tokio::test]
    async fn malformed_hash_error_releases_the_concurrency_permit() {
        let runtime = PasswordHashRuntime::with_policy(1, policy(8)).unwrap();
        let error = runtime
            .verify_password("password", "not-a-phc-string")
            .await
            .unwrap_err();
        assert!(matches!(error, PasswordWorkError::Crypto(_)));

        let hash = runtime.hash_password("next-password").await.unwrap();
        assert!(
            runtime
                .verify_password("next-password", &hash)
                .await
                .unwrap()
                .is_valid
        );
    }

    #[test]
    fn over_budget_hash_is_classified_before_verification() {
        let runtime = PasswordHashRuntime::with_policy(1, policy(8)).unwrap();
        let hash = hash_password_with_policy("password", &policy(8)).unwrap();
        let over_budget = hash.replacen("m=8", "m=9", 1);
        let error =
            verify_password_with_policy("password", &over_budget, &runtime.policy).unwrap_err();

        assert!(matches!(
            error,
            CryptoError::PasswordHashVerificationLimit {
                parameter: "m",
                actual: 9,
                maximum: 8,
            }
        ));
    }
}
