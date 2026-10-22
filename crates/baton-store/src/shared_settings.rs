//! 共同設定版本與 Run 設定快照綁定的持久化。
//!
//! 規則都在 Domain（`baton_domain::shared_settings`）；這裡只做「在同一個交易中取得目前最新版本 →
//! 交給 Domain 採用 → 追加新版本與事件」。版本表與綁定表由觸發器禁止更新與刪除，所以舊版本內容永遠不變。

use baton_domain::shared_settings::{self, ImportCandidate, RunSettingsPin, SettingsBundle, SettingsVersion};

use super::*;

impl Store {
    /// 採用候選內容：每次呼叫都建立新版本（編號接在最新版本之後），並在同一個交易追加事件。
    pub async fn adopt_settings(&self, candidate: ImportCandidate, now: Timestamp) -> StoreResult<SettingsVersion> {
        self.write(move |tx| {
            let latest: Option<i64> = tx.query_row("SELECT MAX(version) FROM shared_settings_version", [], |r| r.get(0))?;
            let adoption = shared_settings::adopt(latest, candidate, now);
            let version = &adoption.version;
            tx.execute(
                "INSERT INTO shared_settings_version (version, content_hash, bundle, sources, exclusions, adopted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    version.version,
                    version.content_hash,
                    to_json(&version.bundle),
                    to_json(&version.sources),
                    to_json(&version.exclusions),
                    version.adopted_at.as_millis(),
                ],
            )?;
            append_event(tx, &adoption.event)?;
            Ok(adoption.version)
        })
        .await
    }

    /// 全部已採用的版本，由舊到新。
    pub async fn settings_versions(&self) -> StoreResult<Vec<SettingsVersion>> {
        self.read(|conn| Ok(load_versions(conn, None)?)).await
    }

    pub async fn settings_version(&self, version: i64) -> StoreResult<Option<SettingsVersion>> {
        self.read(move |conn| Ok(load_versions(conn, Some(version))?.pop())).await
    }

    pub async fn latest_settings_version(&self) -> StoreResult<Option<SettingsVersion>> {
        self.read(|conn| Ok(load_versions(conn, None)?.pop())).await
    }

    /// Run 目前綁定的設定版本；還沒有快照為 `None`。
    pub async fn run_settings_pin(&self, run_id: &str) -> StoreResult<Option<RunSettingsPin>> {
        let run_id = run_id.to_string();
        self.read(move |conn| Ok(load_pin(conn, &run_id)?)).await
    }

    /// 綁定 Run 與設定版本；Run 已有綁定時保留原本的（回傳實際生效的那一筆）。
    pub async fn pin_run_settings(&self, pin: RunSettingsPin) -> StoreResult<RunSettingsPin> {
        self.write(move |tx| {
            tx.execute(
                "INSERT OR IGNORE INTO run_settings_snapshot (run_id, version, content_hash, directory, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![pin.run_id, pin.version, pin.content_hash, pin.directory, pin.created_at.as_millis()],
            )?;
            Ok(load_pin(tx, &pin.run_id)?.expect("剛寫入或原本就有的綁定一定存在"))
        })
        .await
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("可序列化")
}

fn load_versions(conn: &Connection, only: Option<i64>) -> rusqlite::Result<Vec<SettingsVersion>> {
    let mut statement = conn.prepare(
        "SELECT version, content_hash, bundle, sources, exclusions, adopted_at FROM shared_settings_version
         WHERE ?1 IS NULL OR version = ?1 ORDER BY version",
    )?;
    let rows = statement.query_map([only], |r| {
        Ok(SettingsVersion {
            version: r.get(0)?,
            content_hash: r.get(1)?,
            bundle: json_column::<SettingsBundle>(2, &r.get::<_, String>(2)?)?,
            sources: json_column(3, &r.get::<_, String>(3)?)?,
            exclusions: json_column(4, &r.get::<_, String>(4)?)?,
            adopted_at: Timestamp::from_millis(r.get(5)?),
        })
    })?;
    rows.collect()
}

fn load_pin(conn: &Connection, run_id: &str) -> rusqlite::Result<Option<RunSettingsPin>> {
    conn.query_row(
        "SELECT run_id, version, content_hash, directory, created_at FROM run_settings_snapshot WHERE run_id = ?1",
        [run_id],
        |r| {
            Ok(RunSettingsPin {
                run_id: r.get(0)?,
                version: r.get(1)?,
                content_hash: r.get(2)?,
                directory: r.get(3)?,
                created_at: Timestamp::from_millis(r.get(4)?),
            })
        },
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

#[cfg(test)]
mod tests {
    use baton_domain::shared_settings::{import, SourceFile, SourceFiles};

    use super::*;

    const T0: Timestamp = Timestamp::from_millis(500);

    fn candidate(model: &str) -> ImportCandidate {
        let source = |path: &str, text: Option<String>| SourceFile { path: path.into(), bytes: text.map(String::into_bytes) };
        import(&SourceFiles {
            settings: source("settings.json", Some(format!(r#"{{"model":"{model}"}}"#))),
            instructions: source("CLAUDE.md", None),
            user_state: source(".claude.json", None),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn each_adoption_appends_a_version_and_leaves_earlier_ones_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path(), T0).await.unwrap();

        let first = store.adopt_settings(candidate("opus"), T0).await.unwrap();
        let second = store.adopt_settings(candidate("sonnet"), T0.plus_millis(1)).await.unwrap();
        let third = store.adopt_settings(candidate("sonnet"), T0.plus_millis(2)).await.unwrap();

        assert_eq!((first.version, second.version, third.version), (1, 2, 3));
        assert_eq!(store.settings_version(1).await.unwrap(), Some(first));
        assert_eq!(store.latest_settings_version().await.unwrap(), Some(third));
        assert_eq!(store.settings_versions().await.unwrap().len(), 3);
        assert_eq!(store.settings_version(9).await.unwrap(), None);
    }

    #[tokio::test]
    async fn adopted_versions_cannot_be_modified_or_deleted_even_with_raw_sql() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path(), T0).await.unwrap();
        store.adopt_settings(candidate("opus"), T0).await.unwrap();
        drop(store);

        let raw = Connection::open(dir.path().join(DB_FILE_NAME)).unwrap();
        assert!(raw.execute("UPDATE shared_settings_version SET content_hash = 'x'", []).is_err());
        assert!(raw.execute("DELETE FROM shared_settings_version", []).is_err());
    }

    #[tokio::test]
    async fn adoption_appends_an_event_with_the_version_as_aggregate_version() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path(), T0).await.unwrap();
        store.adopt_settings(candidate("opus"), T0).await.unwrap();
        store.adopt_settings(candidate("sonnet"), T0).await.unwrap();
        drop(store);

        let raw = Connection::open(dir.path().join(DB_FILE_NAME)).unwrap();
        let versions: Vec<i64> = raw
            .prepare("SELECT aggregate_version FROM event WHERE aggregate_type = 'shared_settings' ORDER BY seq")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(versions, [1, 2]);
    }
}
