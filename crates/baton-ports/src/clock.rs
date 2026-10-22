use baton_domain::Timestamp;

/// 時間來源；測試以 fake 控制，不使用真實 sleep。
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}
