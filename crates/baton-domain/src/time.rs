//! UTC 時間戳（資料庫一律存 UTC，spec D-33）。

use serde::{Deserialize, Serialize};

/// 自 Unix epoch 起算的毫秒數。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    pub const fn as_millis(self) -> i64 {
        self.0
    }

    /// 加上指定毫秒數，回傳新的時間戳。
    pub const fn plus_millis(self, millis: i64) -> Self {
        Self(self.0 + millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_order_and_offset_by_millis() {
        let base = Timestamp::from_millis(1_000);
        assert!(base < base.plus_millis(1));
        assert_eq!(base.plus_millis(500).as_millis(), 1_500);
    }

    #[test]
    fn timestamp_serializes_as_plain_number() {
        let json = serde_json::to_string(&Timestamp::from_millis(42)).unwrap();
        assert_eq!(json, "42");
    }
}
