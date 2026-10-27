//! 只快取成功結果的一次性非同步初始化：桌面殼用來連回或啟動 Runtime。

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::Mutex;

/// 成功後永久沿用結果；失敗不保留，下次呼叫重新初始化。
/// 初始化期間持有鎖：並行呼叫者等待同一次初始化，不重複執行。
pub struct StartupCell<T, E> {
    state: Mutex<State<T, E>>,
    /// 已完成的失敗次數；呼叫者藉此判斷等待期間是否已有一次初始化失敗
    failures: AtomicU64,
}

struct State<T, E> {
    value: Option<T>,
    last_failure: Option<E>,
}

impl<T: Clone, E: Clone> StartupCell<T, E> {
    pub fn new() -> Self {
        Self { state: Mutex::new(State { value: None, last_failure: None }), failures: AtomicU64::new(0) }
    }

    /// 已成功過就直接回傳；否則執行 `init`，成功才保留結果。
    /// 等待鎖的期間若已有一次初始化失敗，直接回傳該失敗，不讓同一波呼叫者各自再等一次逾時。
    pub async fn get_or_try_init<F, Fut>(&self, init: F) -> Result<T, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let seen = self.failures.load(Ordering::Acquire);
        let mut state = self.state.lock().await;
        if let Some(value) = &state.value {
            return Ok(value.clone());
        }
        if self.failures.load(Ordering::Acquire) != seen {
            if let Some(failure) = &state.last_failure {
                return Err(failure.clone());
            }
        }
        match init().await {
            Ok(value) => {
                state.value = Some(value.clone());
                Ok(value)
            }
            Err(failure) => {
                state.last_failure = Some(failure.clone());
                self.failures.fetch_add(1, Ordering::Release);
                Err(failure)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn failure_is_not_cached_and_next_call_retries() {
        let cell: StartupCell<u32, String> = StartupCell::new();
        let calls = AtomicUsize::new(0);
        let attempt = || async {
            // 第一次失敗（例如啟動逾時），第二次成功（Runtime 之後正常監聽）
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Err("逾時".to_string())
            } else {
                Ok(7)
            }
        };
        assert_eq!(cell.get_or_try_init(attempt).await, Err("逾時".to_string()));
        assert_eq!(cell.get_or_try_init(attempt).await, Ok(7));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn success_is_cached_and_init_is_not_called_again() {
        let cell: StartupCell<u32, String> = StartupCell::new();
        let calls = AtomicUsize::new(0);
        let attempt = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(7)
        };
        assert_eq!(cell.get_or_try_init(attempt).await, Ok(7));
        assert_eq!(cell.get_or_try_init(attempt).await, Ok(7));
        assert_eq!(cell.get_or_try_init(|| async { Err("不該被呼叫".to_string()) }).await, Ok(7));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_callers_share_one_successful_start() {
        let cell: Arc<StartupCell<u32, String>> = Arc::new(StartupCell::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let callers: Vec<_> = (0..5)
            .map(|_| {
                let cell = cell.clone();
                let calls = calls.clone();
                tokio::spawn(async move {
                    cell.get_or_try_init(|| async {
                        calls.fetch_add(1, Ordering::SeqCst);
                        // 啟動需要一段時間，其他呼叫者在這期間到達
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        Ok(7)
                    })
                    .await
                })
            })
            .collect();
        for caller in callers {
            assert_eq!(caller.await.unwrap(), Ok(7));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_callers_waiting_on_a_failed_start_get_that_failure_without_restarting() {
        let cell: Arc<StartupCell<u32, String>> = Arc::new(StartupCell::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let callers: Vec<_> = (0..5)
            .map(|_| {
                let cell = cell.clone();
                let calls = calls.clone();
                tokio::spawn(async move {
                    cell.get_or_try_init(|| async {
                        calls.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_secs(20)).await;
                        Err("逾時".to_string())
                    })
                    .await
                })
            })
            .collect();
        for caller in callers {
            assert_eq!(caller.await.unwrap(), Err("逾時".to_string()));
        }
        // 同一波等待中的呼叫者沿用這次失敗，不各自排隊再等一次逾時
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // 之後的呼叫重新嘗試
        assert_eq!(cell.get_or_try_init(|| async { Ok(9) }).await, Ok(9));
    }
}
