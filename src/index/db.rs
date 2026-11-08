//! SQLite 인덱스 (WAL). 벡터는 항상 원본(768차원) f32 LE BLOB으로 저장.

use crate::embed::vecmath;
use crate::extract::{Kind, Modality};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

pub const ST_OK: i64 = 1;
pub const ST_ERROR: i64 = 2;
pub const ST_NAME_ONLY: i64 = 3;

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS files(
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    kind INTEGER NOT NULL,
    status INTEGER NOT NULL,
    error TEXT,
    indexed_at INTEGER
);
CREATE TABLE IF NOT EXISTS chunks(
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    modality INTEGER NOT NULL,
    locator INTEGER NOT NULL,
    snippet TEXT,
    vec BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS chunks_file ON chunks(file_id);
CREATE TABLE IF NOT EXISTS thumbs(
    chunk_id INTEGER PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE,
    jpeg BLOB NOT NULL
);
"#;

pub struct Db {
    conn: Connection,
}

#[derive(Clone, Copy, Debug)]
pub struct FileRow {
    pub id: i64,
    pub size: i64,
    pub mtime: i64,
    pub status: i64,
}

pub struct ChunkRecord {
    pub modality: Modality,
    pub locator: i64,
    pub snippet: String,
    pub vec: Vec<f32>,
    pub thumb: Option<Vec<u8>>,
}

pub struct FileRecord {
    pub path: String,
    pub size: i64,
    pub mtime: i64,
    pub kind: Kind,
    pub status: i64,
    pub error: Option<String>,
    pub chunks: Vec<ChunkRecord>,
}

pub struct StoreRow {
    pub chunk_id: i64,
    pub file_id: i64,
    pub modality: Modality,
    pub locator: i64,
    pub kind: Kind,
    pub path: String,
    pub vec: Vec<f32>,
}

pub struct ChunkDetail {
    pub snippet: String,
    pub has_thumb: bool,
}

#[derive(Clone, Default, Debug)]
pub struct DbStats {
    pub files: u64,
    pub chunks: u64,
    pub files_by_kind: [u64; 6],
    pub errors: u64,
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(15))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn })
    }

    pub fn meta_get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn chunk_count(&self) -> Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get::<_, i64>(0))? as u64)
    }

    pub fn load_file_map(&self) -> Result<HashMap<String, FileRow>> {
        let mut st = self
            .conn
            .prepare("SELECT path, id, size, mtime, status FROM files")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                FileRow {
                    id: r.get(1)?,
                    size: r.get(2)?,
                    mtime: r.get(3)?,
                    status: r.get(4)?,
                },
            ))
        })?;
        let mut m = HashMap::new();
        for row in rows {
            let (p, f) = row?;
            m.insert(p, f);
        }
        Ok(m)
    }

    /// 파일 하나를 트랜잭션으로 기록 (기존 청크는 교체). 반환: (file_id, chunk_ids)
    pub fn write_file(&mut self, rec: &FileRecord) -> Result<(i64, Vec<i64>)> {
        let tx = self.conn.transaction()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let file_id: i64 = tx.query_row(
            "INSERT INTO files(path,size,mtime,kind,status,error,indexed_at) VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(path) DO UPDATE SET size=excluded.size, mtime=excluded.mtime, kind=excluded.kind,
                status=excluded.status, error=excluded.error, indexed_at=excluded.indexed_at
             RETURNING id",
            params![rec.path, rec.size, rec.mtime, rec.kind as i64, rec.status, rec.error, now],
            |r| r.get(0),
        )?;
        tx.execute("DELETE FROM chunks WHERE file_id=?1", [file_id])?;
        let mut ids = Vec::with_capacity(rec.chunks.len());
        {
            let mut ins = tx.prepare_cached(
                "INSERT INTO chunks(file_id,modality,locator,snippet,vec) VALUES(?1,?2,?3,?4,?5)",
            )?;
            let mut th = tx.prepare_cached("INSERT INTO thumbs(chunk_id,jpeg) VALUES(?1,?2)")?;
            for c in &rec.chunks {
                ins.execute(params![
                    file_id,
                    c.modality as i64,
                    c.locator,
                    c.snippet,
                    vecmath::to_bytes(&c.vec)
                ])?;
                let cid = tx.last_insert_rowid();
                if let Some(t) = &c.thumb {
                    th.execute(params![cid, t])?;
                }
                ids.push(cid);
            }
        }
        tx.commit()?;
        Ok((file_id, ids))
    }

    pub fn delete_files(&mut self, ids: &[i64]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut st = tx.prepare_cached("DELETE FROM files WHERE id=?1")?;
            for id in ids {
                st.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn clear_all(&mut self) -> Result<()> {
        self.conn
            .execute_batch("DELETE FROM thumbs; DELETE FROM chunks; DELETE FROM files;")?;
        Ok(())
    }

    /// 검색용 메모리 저장소 적재
    pub fn for_each_vector(&self, mut f: impl FnMut(StoreRow)) -> Result<()> {
        let mut st = self.conn.prepare(
            "SELECT c.id, c.file_id, c.modality, c.locator, c.vec, f.kind, f.path
             FROM chunks c JOIN files f ON f.id = c.file_id",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let blob: Vec<u8> = r.get(4)?;
            f(StoreRow {
                chunk_id: r.get(0)?,
                file_id: r.get(1)?,
                modality: Modality::from_u8(r.get::<_, i64>(2)? as u8),
                locator: r.get(3)?,
                vec: vecmath::from_bytes(&blob),
                kind: Kind::from_u8(r.get::<_, i64>(5)? as u8),
                path: r.get(6)?,
            });
        }
        Ok(())
    }

    pub fn chunk_detail(&self, chunk_id: i64) -> Result<Option<ChunkDetail>> {
        Ok(self
            .conn
            .query_row(
                "SELECT COALESCE(c.snippet,''), EXISTS(SELECT 1 FROM thumbs t WHERE t.chunk_id=c.id)
                 FROM chunks c WHERE c.id=?1",
                [chunk_id],
                |r| {
                    Ok(ChunkDetail {
                        snippet: r.get(0)?,
                        has_thumb: r.get::<_, i64>(1)? != 0,
                    })
                },
            )
            .optional()?)
    }

    pub fn thumb(&self, chunk_id: i64) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT jpeg FROM thumbs WHERE chunk_id=?1", [chunk_id], |r| r.get(0))
            .optional()?)
    }

    /// 파일의 대표 썸네일 (이미지/영상 첫 프레임)
    pub fn file_thumb_chunk(&self, file_id: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT t.chunk_id FROM thumbs t JOIN chunks c ON c.id=t.chunk_id
                 WHERE c.file_id=?1 ORDER BY c.locator LIMIT 1",
                [file_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn stats(&self) -> Result<DbStats> {
        let mut s = DbStats {
            chunks: self.chunk_count()?,
            ..Default::default()
        };
        let mut st = self
            .conn
            .prepare("SELECT kind, COUNT(*), SUM(status=2) FROM files GROUP BY kind")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let k = Kind::from_u8(r.get::<_, i64>(0)? as u8);
            let n = r.get::<_, i64>(1)? as u64;
            s.files_by_kind[k.idx()] = n;
            s.files += n;
            s.errors += r.get::<_, Option<i64>>(2)?.unwrap_or(0) as u64;
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_replace() {
        let dir = std::env::temp_dir().join(format!("gs-test-{}", std::process::id()));
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);
        let mut db = Db::open(&path).unwrap();
        let rec = |n: usize| FileRecord {
            path: r"C:\x\a.txt".into(),
            size: 10,
            mtime: 20,
            kind: Kind::Text,
            status: ST_OK,
            error: None,
            chunks: (0..n)
                .map(|i| ChunkRecord {
                    modality: Modality::Text,
                    locator: i as i64,
                    snippet: format!("s{i}"),
                    vec: vec![i as f32; 4],
                    thumb: (i == 0).then(|| vec![1, 2, 3]),
                })
                .collect(),
        };
        let (id1, c1) = db.write_file(&rec(3)).unwrap();
        assert_eq!(c1.len(), 3);
        let (id2, c2) = db.write_file(&rec(2)).unwrap();
        assert_eq!(id1, id2, "같은 경로는 같은 file_id");
        assert_eq!(db.chunk_count().unwrap(), 2, "기존 청크는 교체");
        assert_eq!(db.thumb(c2[0]).unwrap(), Some(vec![1, 2, 3]));
        let mut n = 0;
        db.for_each_vector(|r| {
            assert_eq!(r.vec.len(), 4);
            n += 1;
        })
        .unwrap();
        assert_eq!(n, 2);
        db.delete_files(&[id1]).unwrap();
        assert_eq!(db.chunk_count().unwrap(), 0, "CASCADE 삭제");
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }
}
