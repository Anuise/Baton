//! 可取消的後備計時器：系統匣「完全退出」在 Web UI 沒反應時的後備。

use std::sync::Mutex;

use tauri::async_runtime::JoinHandle;

/// 同時最多一個等待中的計時器；可取消。
pub struct FallbackTimer {
    pending: Mutex<Option<JoinHandle<()>>>,
}

impl FallbackTimer {
    pub fn new() -> Self {
        Self { pending: Mutex::new(None) }
    }

    /// 沒有等待中的計時器時，以 `spawn` 排定一個並回傳 true；已有則不另起，回傳 false。
    pub fn arm(&self, spawn: impl FnOnce() -> JoinHandle<()>) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.as_ref().is_some_and(|handle| !handle.inner().is_finished()) {
            return false;
        }
        *pending = Some(spawn());
        true
    }

    /// 取消等待中的計時器（沒有就不做事）。
    pub fn cancel(&self) {
        if let Some(handle) = self.pending.lock().unwrap_or_else(|e| e.into_inner()).take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::oneshot;

    use super::*;

    const DELAY: Duration = Duration::from_secs(60);

    /// 排定一個 `DELAY` 後送出訊號的計時器；回傳的接收端在計時器觸發時收到值，被取消時收到關閉。
    fn timer_task() -> (impl FnOnce() -> JoinHandle<()>, oneshot::Receiver<()>) {
        let (fired, receiver) = oneshot::channel();
        let spawn = move || {
            JoinHandle::Tokio(tokio::spawn(async move {
                tokio::time::sleep(DELAY).await;
                let _ = fired.send(());
            }))
        };
        (spawn, receiver)
    }

    #[tokio::test(start_paused = true)]
    async fn armed_timer_fires_after_delay() {
        let timer = FallbackTimer::new();
        let (spawn, fired) = timer_task();
        assert!(timer.arm(spawn));
        assert!(fired.await.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_timer_never_fires() {
        let timer = FallbackTimer::new();
        let (spawn, fired) = timer_task();
        assert!(timer.arm(spawn));
        tokio::time::sleep(DELAY / 2).await;
        timer.cancel();
        // 被取消的任務丟棄送出端：接收端得到關閉而不是觸發（時間暫停時會自動推進，未取消就會觸發）
        assert!(fired.await.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn pending_timer_is_not_armed_twice() {
        let timer = FallbackTimer::new();
        let (first, first_fired) = timer_task();
        let (second, second_fired) = timer_task();
        assert!(timer.arm(first));
        assert!(!timer.arm(second));
        // 第二個沒有被排定：送出端隨閉包丟棄
        assert!(second_fired.await.is_err());
        assert!(first_fired.await.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn timer_can_be_armed_again_after_cancel_or_fire() {
        let timer = FallbackTimer::new();
        let (first, _first_fired) = timer_task();
        assert!(timer.arm(first));
        timer.cancel();
        let (second, second_fired) = timer_task();
        assert!(timer.arm(second));
        assert!(second_fired.await.is_ok());
        // 觸發後的任務已結束（送出訊號與結束在同一次 poll 完成），可以再排定新的
        let (third, third_fired) = timer_task();
        assert!(timer.arm(third));
        assert!(third_fired.await.is_ok());
    }

    #[test]
    fn cancel_without_pending_timer_is_harmless() {
        FallbackTimer::new().cancel();
    }
}
