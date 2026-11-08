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
CREATE TABLE IF NOT EXISTS roots(
    path TEXT PRIMARY KEY,
    first_indexed INTEGER,
    last_indexed INTEGER
);
CREATE TABLE IF NOT EXISTS crash_suspects(
    path TEXT PRIMARY KEY,
    count INTEGER NOT NULL
);
"#;

/// 이 횟수만큼 서버 충돌 때 처리 중이던 파일은 내용 인덱싱을 건너뛴다
pub const CRASH_SUSPECT_LIMIT: i64 = 2;

/// 벡터 저장 형식 (meta.vec_format). 예전 DB(값 없음 + 청크 있음)는 f32.
const META_VEC_FORMAT: &str = "vec_format";
const META_MIGRATE_LAST: &str = "f16_migrate_last_id";

pub struct Db {
    conn: Connection,
    /// true면 벡터를 FP16으로 저장/해석
    f16: bool,
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
    pub f16: bool,
    pub file_bytes: u64,
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(30))?;
        conn.execute_batch(SCHEMA)?;
        let mut db = Db { conn, f16: false };
        match db.meta_get(META_VEC_FORMAT).as_deref() {
            Some("f16") => db.f16 = true,
            Some(_) => db.f16 = false,
            None => {
                // 새 DB는 처음부터 FP16. 형식 기록이 없는데 청크가 있으면 예전(f32) DB.
                if db.chunk_count()? == 0 {
                    db.meta_set(META_VEC_FORMAT, "f16")?;
                    db.f16 = true;
                }
            }
        }
        Ok(db)
    }

    pub fn is_f16(&self) -> bool {
        self.f16
    }

    fn decode_vec(&self, b: &[u8]) -> Vec<f32> {
        if self.f16 {
            vecmath::from_f16_bytes(b)
        } else {
            vecmath::from_bytes(b)
        }
    }

    /// 예전 f32 DB를 FP16으로 변환 후 VACUUM으로 파일 크기를 줄인다.
    /// 배치마다 커밋하고 진행 위치를 기록하므로 중간에 꺼져도 이어서 변환된다.
    /// progress(단계, 완료, 전체)
    pub fn migrate_to_f16(&mut self, mut progress: impl FnMut(&str, u64, u64)) -> Result<()> {
        if self.f16 {
            return Ok(());
        }
        let total = self.chunk_count()?;
        let mut last: i64 = self
            .meta_get(META_MIGRATE_LAST)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let mut done: u64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks WHERE id <= ?1", [last], |r| r.get::<_, i64>(0))?
            as u64;
        progress("FP16 변환", done, total);
        loop {
            let tx = self.conn.transaction()?;
            let rows: Vec<(i64, Vec<u8>)> = {
                let mut st =
                    tx.prepare("SELECT id, vec FROM chunks WHERE id > ?1 ORDER BY id LIMIT 2000")?;
                let mapped = st.query_map([last], |r| Ok((r.get(0)?, r.get(1)?)))?;
                mapped.collect::<std::result::Result<_, _>>()?
            };
            if rows.is_empty() {
                tx.commit()?;
                break;
            }
            {
                let mut up = tx.prepare_cached("UPDATE chunks SET vec=?1 WHERE id=?2")?;
                for (id, blob) in &rows {
                    let v = vecmath::from_bytes(blob);
                    up.execute(params![vecmath::to_f16_bytes(&v), id])?;
                }
            }
            last = rows.last().map(|r| r.0).unwrap_or(last);
            tx.execute(
                "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![META_MIGRATE_LAST, last.to_string()],
            )?;
            tx.commit()?;
            done += rows.len() as u64;
            progress("FP16 변환", done, total);
        }
        self.meta_set(META_VEC_FORMAT, "f16")?;
        self.conn
            .execute("DELETE FROM meta WHERE key=?1", [META_MIGRATE_LAST])?;
        self.f16 = true;
        progress("DB 압축(VACUUM)", total, total);
        self.vacuum()?;
        Ok(())
    }

    /// 빈 공간을 없애 파일 크기를 줄인다 (데이터는 그대로)
    pub fn vacuum(&self) -> Result<()> {
        self.conn.execute_batch("VACUUM;")?;
        // WAL 파일도 비워서 실제 디스크 사용량을 줄인다
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        Ok(())
    }

    /// 인덱싱한 대상 폴더 기록 (나중에 대상에서 빼도 검색 폴더 목록에 남도록)
    pub fn register_roots(&self, roots: &[String]) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for r in roots {
            let r = r.trim();
            if r.is_empty() {
                continue;
            }
            self.conn.execute(
                "INSERT INTO roots(path, first_indexed, last_indexed) VALUES(?1, ?2, ?2)
                 ON CONFLICT(path) DO UPDATE SET last_indexed=excluded.last_indexed",
                params![r, now],
            )?;
        }
        Ok(())
    }

    /// 서버 충돌 때 처리 중이던 파일 기록 (+1)
    pub fn add_crash_suspects(&self, paths: &[String]) -> Result<()> {
        for p in paths {
            self.conn.execute(
                "INSERT INTO crash_suspects(path, count) VALUES(?1, 1)
                 ON CONFLICT(path) DO UPDATE SET count = count + 1",
                [p],
            )?;
        }
        Ok(())
    }

    pub fn crash_suspects(&self) -> Result<HashMap<String, i64>> {
        let mut st = self.conn.prepare("SELECT path, count FROM crash_suspects")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn roots(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT path FROM roots ORDER BY path")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
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
        if rec.status != ST_ERROR {
            // 정상 처리됐으면 충돌 의심 기록 해제
            tx.execute("DELETE FROM crash_suspects WHERE path=?1", [&rec.path])?;
        }
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
                    if self.f16 {
                        vecmath::to_f16_bytes(&c.vec)
                    } else {
                        vecmath::to_bytes(&c.vec)
                    }
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
        self.conn.execute_batch(
            "DELETE FROM thumbs; DELETE FROM chunks; DELETE FROM files; DELETE FROM crash_suspects;",
        )?;
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
                vec: self.decode_vec(&blob),
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
            // 다른 연결이 변환했을 수 있으니 매번 meta에서 읽는다
            f16: self.meta_get(META_VEC_FORMAT).as_deref() == Some("f16"),
            ..Default::default()
        };
        if let Some(p) = self.conn.path() {
            let base = std::path::PathBuf::from(p);
            let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            let wal = base.with_extension("db-wal");
            s.file_bytes = size(&base) + size(&wal);
        }
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
        assert!(db.is_f16(), "새 DB는 FP16");
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn crash_suspects_count_and_clear() {
        let dir = std::env::temp_dir().join(format!("gs-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut db = Db::open(&dir.join("t.db")).unwrap();
        let a = r"C:\x\a.jpg".to_string();
        let b = r"C:\x\b.jpg".to_string();
        db.add_crash_suspects(&[a.clone(), b.clone()]).unwrap();
        db.add_crash_suspects(&[a.clone()]).unwrap();
        let s = db.crash_suspects().unwrap();
        assert_eq!(s[&a], 2);
        assert_eq!(s[&b], 1);
        let rec = |path: &str, status| FileRecord {
            path: path.into(),
            size: 1,
            mtime: 1,
            kind: Kind::Image,
            status,
            error: None,
            chunks: vec![],
        };
        db.write_file(&rec(&a, ST_ERROR)).unwrap();
        db.write_file(&rec(&b, ST_OK)).unwrap();
        let s = db.crash_suspects().unwrap();
        assert_eq!(s.get(&a), Some(&2), "오류로 기록되면 의심 기록 유지");
        assert!(s.get(&b).is_none(), "정상 처리되면 해제");
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrates_legacy_f32_to_f16() {
        let dir = std::env::temp_dir().join(format!("gs-mig-{}", std::process::id()));
        let path = dir.join("t.db");
        let _ = std::fs::remove_dir_all(&dir);
        // 예전 형식 DB 흉내: vec_format = f32
        {
            let db = Db::open(&path).unwrap();
            db.meta_set(META_VEC_FORMAT, "f32").unwrap();
        }
        let mut db = Db::open(&path).unwrap();
        assert!(!db.is_f16());
        let vecs: Vec<Vec<f32>> = (0..2500)
            .map(|i| {
                let mut v: Vec<f32> = (0..16).map(|j| ((i * 7 + j) as f32).sin()).collect();
                vecmath::normalize(&mut v);
                v
            })
            .collect();
        let rec = FileRecord {
            path: r"C:\x\big.txt".into(),
            size: 1,
            mtime: 1,
            kind: Kind::Text,
            status: ST_OK,
            error: None,
            chunks: vecs
                .iter()
                .enumerate()
                .map(|(i, v)| ChunkRecord {
                    modality: Modality::Text,
                    locator: i as i64,
                    snippet: String::new(),
                    vec: v.clone(),
                    thumb: None,
                })
                .collect(),
        };
        db.write_file(&rec).unwrap();
        let mut stages = Vec::new();
        db.migrate_to_f16(|s, done, total| stages.push((s.to_string(), done, total)))
            .unwrap();
        assert!(db.is_f16());
        assert!(stages.iter().any(|(s, ..)| s.contains("VACUUM")));
        assert_eq!(stages.iter().filter(|(s, ..)| s == "FP16 변환").last().unwrap().1, 2500);
        drop(db);
        // 다시 열어도 FP16으로 읽히고 값이 거의 같아야 함
        let db = Db::open(&path).unwrap();
        assert!(db.is_f16());
        let mut i = 0usize;
        db.for_each_vector(|r| {
            assert_eq!(r.vec.len(), 16);
            let err: f32 = r.vec.iter().zip(&vecs[r.locator as usize]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert!(err < 1e-3, "변환 오차 {err}");
            i += 1;
        })
        .unwrap();
        assert_eq!(i, 2500);
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }
}
