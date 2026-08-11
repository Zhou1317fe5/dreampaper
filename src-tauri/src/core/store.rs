use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::error::AppResult;

#[derive(Clone, Debug)]
pub struct Store {
    db_path: PathBuf,
}

impl Store {
    pub fn initialize(app_data: &Path) -> AppResult<Self> {
        std::fs::create_dir_all(app_data)?;
        for name in ["uploads", "outputs", "templates", "logs"] {
            std::fs::create_dir_all(app_data.join(name))?;
        }

        let store = Self {
            db_path: app_data.join("dreampaper.sqlite"),
        };
        let conn = store.connection()?;
        conn.execute_batch(SCHEMA)?;
        Self::run_fts5_smoke_test(&conn)?;
        Ok(store)
    }

    pub fn connection(&self) -> AppResult<Connection> {
        let conn = Connection::open(&self.db_path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        Ok(conn)
    }

    fn run_fts5_smoke_test(conn: &Connection) -> AppResult<()> {
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS fts_probe USING fts5(body);\
             DELETE FROM fts_probe;\
             INSERT INTO fts_probe(body) VALUES ('dream paper sqlite fts5 probe');",
        )?;
        {
            let mut stmt =
                conn.prepare("SELECT rowid FROM fts_probe WHERE fts_probe MATCH 'dream'")?;
            let mut rows = stmt.query([])?;
            let _ = rows.next()?;
        }
        conn.execute_batch("DROP TABLE IF EXISTS fts_probe;")?;
        Ok(())
    }
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT OR IGNORE INTO meta(key, value) VALUES ('schema_version', '1');

CREATE TABLE IF NOT EXISTS config(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS assets(
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  filename TEXT NOT NULL,
  mime TEXT NOT NULL,
  path TEXT NOT NULL,
  bytes INTEGER NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS template_packs(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  version TEXT,
  source_path TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS templates(
  id TEXT PRIMARY KEY,
  pack_id TEXT,
  source_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  category TEXT,
  rounded_ratio TEXT,
  visual_intent TEXT NOT NULL,
  content_summary TEXT NOT NULL,
  image_path TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS jobs(
  id TEXT PRIMARY KEY,
  mode TEXT NOT NULL,
  status TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  result_json TEXT,
  error_json TEXT,
  message TEXT,
  stage TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS job_stages(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id TEXT NOT NULL,
  stage TEXT NOT NULL,
  message TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS artifacts(
  id TEXT PRIMARY KEY,
  job_id TEXT,
  kind TEXT NOT NULL,
  path TEXT NOT NULL,
  mime TEXT,
  meta_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS documents(
  id TEXT PRIMARY KEY,
  asset_id TEXT,
  title TEXT,
  parser TEXT NOT NULL,
  status TEXT NOT NULL,
  meta_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS chunks(
  id TEXT PRIMARY KEY,
  document_id TEXT NOT NULL,
  ord INTEGER NOT NULL,
  heading TEXT,
  page INTEGER,
  start_char INTEGER,
  end_char INTEGER,
  text TEXT NOT NULL,
  meta_json TEXT NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
  title,
  heading,
  body
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_is_idempotent_and_survives_fts_probe() {
        let dir = std::env::temp_dir().join(format!(
            "dreampaper-store-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);

        Store::initialize(&dir).expect("首次初始化应成功");
        let store = Store::initialize(&dir).expect("重复初始化应成功");

        let conn = store.connection().expect("应能取到连接");
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('jobs','job_stages','assets','templates','documents','chunks')",
                [],
                |row| row.get(0),
            )
            .expect("应能查询 schema");
        assert_eq!(tables, 6, "建表不完整");

        let probe: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'fts_probe'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(probe, 0, "fts_probe 未被清理");

        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
