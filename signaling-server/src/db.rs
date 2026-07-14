//! 数据库访问层 - 同时支持 SQLite 和 MySQL。
//!
//! 通过 URL 前缀自动选择后端：
//! - `sqlite://` 或文件路径 → SQLite
//! - `mysql://` → MySQL
//!
//! 使用 trait 抽象 + enum dispatch 实现双后端支持。

use anyhow::{Context, Result};
use std::sync::Arc;

// ─── Record 类型 ──────────────────────────────────────────────────────

/// users 表行记录
#[derive(Debug, Clone)]
pub struct UserRecord {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub created_at: String,
}

/// machines 表行记录
#[derive(Debug, Clone)]
pub struct PeerRecord {
    pub node_id: String,
    pub user_id: Option<i64>,  // None means unassigned
    pub name: String,
    pub os: String,
    pub last_heartbeat: i64,
    pub online: bool,
}

// ─── Database 统一接口 ────────────────────────────────────────────────

/// 数据库后端枚举，根据 URL 自动选择 SQLite 或 MySQL
#[derive(Clone)]
pub struct Database {
    inner: Arc<DatabaseInner>,
}

enum DatabaseInner {
    Sqlite(SqliteDatabase),
    Mysql(MysqlDatabase),
}

impl Database {
    /// 根据 URL 创建数据库连接。
    /// - `sqlite://path` 或 `path.db` → SQLite
    /// - `mysql://user:pass@host/db` → MySQL
    pub async fn new(database_url: &str) -> Result<Self> {
        let inner = if database_url.starts_with("mysql://") {
            DatabaseInner::Mysql(MysqlDatabase::new(database_url).await?)
        } else {
            // Default to SQLite - strip sqlite:// prefix if present
            let path = database_url.strip_prefix("sqlite://").unwrap_or(database_url);
            DatabaseInner::Sqlite(SqliteDatabase::new(path)?)
        };
        
        let db = Self { inner: Arc::new(inner) };
        db.create_tables().await?;
        Ok(db)
    }

    /// 执行建表 SQL（幂等）
    pub async fn create_tables(&self) -> Result<()> {
        match self.inner.as_ref() {
            DatabaseInner::Sqlite(db) => db.create_tables(),
            DatabaseInner::Mysql(db) => db.create_tables().await,
        }
    }

    /// 插入新用户，返回自增 id
    pub async fn create_user(&self, username: &str, password_hash: &str) -> Result<i64> {
        match self.inner.as_ref() {
            DatabaseInner::Sqlite(db) => db.create_user(username, password_hash),
            DatabaseInner::Mysql(db) => db.create_user(username, password_hash).await,
        }
    }

    /// 按用户名查找用户
    pub async fn get_user_by_username(&self, username: &str) -> Option<UserRecord> {
        match self.inner.as_ref() {
            DatabaseInner::Sqlite(db) => db.get_user_by_username(username),
            DatabaseInner::Mysql(db) => db.get_user_by_username(username).await,
        }
    }

    /// 按 id 查找用户
    pub async fn get_user_by_id(&self, id: i64) -> Option<UserRecord> {
        match self.inner.as_ref() {
            DatabaseInner::Sqlite(db) => db.get_user_by_id(id),
            DatabaseInner::Mysql(db) => db.get_user_by_id(id).await,
        }
    }

    /// 插入或更新机器信息（UPSERT）
    pub async fn upsert_machine(
        &self,
        node_id: &str,
        user_id: Option<i64>,
        name: &str,
        os: &str,
    ) -> Result<()> {
        match self.inner.as_ref() {
            DatabaseInner::Sqlite(db) => db.upsert_machine(node_id, user_id, name, os),
            DatabaseInner::Mysql(db) => db.upsert_machine(node_id, user_id, name, os).await,
        }
    }

    /// 更新心跳时间戳并标记在线
    pub async fn update_heartbeat(&self, node_id: &str) -> Result<()> {
        match self.inner.as_ref() {
            DatabaseInner::Sqlite(db) => db.update_heartbeat(node_id),
            DatabaseInner::Mysql(db) => db.update_heartbeat(node_id).await,
        }
    }

    /// 获取指定用户的所有在线机器
    pub async fn get_online_machines(&self, user_id: i64) -> Vec<PeerRecord> {
        match self.inner.as_ref() {
            DatabaseInner::Sqlite(db) => db.get_online_machines(user_id),
            DatabaseInner::Mysql(db) => db.get_online_machines(user_id).await,
        }
    }

    /// 获取所有在线机器
    pub async fn get_all_online_machines(&self) -> Vec<PeerRecord> {
        match self.inner.as_ref() {
            DatabaseInner::Sqlite(db) => db.get_all_online_machines(),
            DatabaseInner::Mysql(db) => db.get_all_online_machines().await,
        }
    }

    /// 标记超时机器为离线，返回本次变为离线的 node_id 列表
    pub async fn mark_offline_machines(&self, timeout_secs: i64) -> Result<Vec<String>> {
        match self.inner.as_ref() {
            DatabaseInner::Sqlite(db) => db.mark_offline_machines(timeout_secs),
            DatabaseInner::Mysql(db) => db.mark_offline_machines(timeout_secs).await,
        }
    }

    /// Unbind a machine from a user (set user_id to NULL)
    pub async fn machine_unbind(&self, node_id: &str, user_id: i64) -> Result<bool> {
        match self.inner.as_ref() {
            DatabaseInner::Sqlite(db) => db.machine_unbind(node_id, user_id),
            DatabaseInner::Mysql(db) => db.machine_unbind(node_id, user_id).await,
        }
    }

    /// Get machines bound to a user
    pub async fn get_user_machines(&self, user_id: i64) -> Vec<PeerRecord> {
        match self.inner.as_ref() {
            DatabaseInner::Sqlite(db) => db.get_user_machines(user_id),
            DatabaseInner::Mysql(db) => db.get_user_machines(user_id).await,
        }
    }

    /// Get machine record by node_id
    pub async fn get_machine_by_node_id(&self, node_id: &str) -> Result<Option<PeerRecord>> {
        match self.inner.as_ref() {
            DatabaseInner::Sqlite(db) => db.get_machine_by_node_id(node_id),
            DatabaseInner::Mysql(db) => db.get_machine_by_node_id(node_id).await,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// SQLite 后端实现
// ═══════════════════════════════════════════════════════════════════════

use rusqlite::{params, Connection};
use std::sync::Mutex;

/// SQLite 数据库访问对象
struct SqliteDatabase {
    conn: Mutex<Connection>,
}

impl SqliteDatabase {
    /// 打开（或创建）SQLite 数据库文件
    fn new(db_path: &str) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open SQLite database at {}", db_path))?;
        log::info!("SQLite database opened: {}", db_path);
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn create_tables(&self) -> Result<()> {
        self.conn.lock().unwrap().execute_batch(
            "
            CREATE TABLE IF NOT EXISTS users (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                username      TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                created_at    TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS machines (
                node_id         TEXT PRIMARY KEY,
                user_id         INTEGER,
                name            TEXT NOT NULL DEFAULT '',
                os              TEXT NOT NULL DEFAULT '',
                last_heartbeat  INTEGER NOT NULL DEFAULT 0,
                online          INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE SET NULL
            );
            "
        ).context("Failed to create SQLite tables")?;
        log::info!("SQLite tables verified/created");
        Ok(())
    }

    fn create_user(&self, username: &str, password_hash: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO users (username, password_hash) VALUES (?1, ?2)",
            params![username, password_hash],
        ).with_context(|| format!("Failed to create user '{}'", username))?;
        let id = conn.last_insert_rowid();
        log::info!("User '{}' created with id={}", username, id);
        Ok(id)
    }

    fn get_user_by_username(&self, username: &str) -> Option<UserRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, username, password_hash, created_at FROM users WHERE username = ?1")
            .ok()?;
        stmt.query_row(params![username], |row| {
            Ok(UserRecord {
                id: row.get(0)?,
                username: row.get(1)?,
                password_hash: row.get(2)?,
                created_at: row.get(3)?,
            })
        }).ok()
    }

    fn get_user_by_id(&self, id: i64) -> Option<UserRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, username, password_hash, created_at FROM users WHERE id = ?1")
            .ok()?;
        stmt.query_row(params![id], |row| {
            Ok(UserRecord {
                id: row.get(0)?,
                username: row.get(1)?,
                password_hash: row.get(2)?,
                created_at: row.get(3)?,
            })
        }).ok()
    }

    fn upsert_machine(&self, node_id: &str, user_id: Option<i64>, name: &str, os: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO machines (node_id, user_id, name, os, last_heartbeat, online)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)
             ON CONFLICT(node_id) DO UPDATE SET
                 user_id = excluded.user_id,
                 name = excluded.name,
                 os = excluded.os,
                 online = 1,
                 last_heartbeat = excluded.last_heartbeat",
            params![node_id, user_id, name, os, chrono::Utc::now().timestamp()],
        ).with_context(|| format!("Failed to upsert machine '{}'", node_id))?;
        log::info!("Peer '{}' upserted (user_id={:?})", node_id, user_id);
        Ok(())
    }

    fn update_heartbeat(&self, node_id: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE machines SET last_heartbeat = ?1, online = 1 WHERE node_id = ?2",
            params![now, node_id],
        ).with_context(|| format!("Failed to update heartbeat for '{}'", node_id))?;
        Ok(())
    }

    fn get_online_machines(&self, user_id: i64) -> Vec<PeerRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT node_id, user_id, name, os, last_heartbeat, online
             FROM machines WHERE user_id IS NOT NULL AND user_id = ?1 AND online = 1",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = stmt.query_map(params![user_id], |row| {
            let online: i32 = row.get(5)?;
            Ok(PeerRecord {
                node_id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                os: row.get(3)?,
                last_heartbeat: row.get(4)?,
                online: online != 0,
            })
        }).ok();

        match rows {
            Some(iter) => iter.filter_map(|r| r.ok()).collect(),
            None => Vec::new(),
        }
    }

    fn get_all_online_machines(&self) -> Vec<PeerRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT node_id, user_id, name, os, last_heartbeat, online
             FROM machines WHERE online = 1",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = stmt.query_map([], |row| {
            let online: i32 = row.get(5)?;
            Ok(PeerRecord {
                node_id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                os: row.get(3)?,
                last_heartbeat: row.get(4)?,
                online: online != 0,
            })
        }).ok();

        match rows {
            Some(iter) => iter.filter_map(|r| r.ok()).collect(),
            None => Vec::new(),
        }
    }

    fn mark_offline_machines(&self, timeout_secs: i64) -> Result<Vec<String>> {
        let now = chrono::Utc::now().timestamp();
        let cutoff = now - timeout_secs;
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT node_id FROM machines WHERE online = 1 AND last_heartbeat < ?1",
        )?;

        let offlined: Vec<String> = stmt
            .query_map(params![cutoff], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        if offlined.is_empty() {
            return Ok(Vec::new());
        }

        conn.execute(
            "UPDATE machines SET online = 0 WHERE online = 1 AND last_heartbeat < ?1",
            params![cutoff],
        ).with_context(|| "Failed to mark offline machines")?;

        for mid in &offlined {
            log::warn!("Peer '{}' marked offline (timeout)", mid);
        }

        Ok(offlined)
    }

    fn machine_unbind(&self, node_id: &str, user_id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows_affected = conn.execute(
            "UPDATE machines SET user_id = NULL WHERE node_id = ?1 AND user_id = ?2",
            params![node_id, user_id],
        ).with_context(|| format!("Failed to unbind machine '{}'", node_id))?;
        
        let unbound = rows_affected > 0;
        if unbound {
            log::info!("Peer '{}' unbound from user_id={}", node_id, user_id);
        } else {
            log::warn!("Peer '{}' not found or not bound to user_id={}", node_id, user_id);
        }
        Ok(unbound)
    }

    fn get_user_machines(&self, user_id: i64) -> Vec<PeerRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT node_id, user_id, name, os, last_heartbeat, online
             FROM machines WHERE user_id = ?1",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = stmt.query_map(params![user_id], |row| {
            let online: i32 = row.get(5)?;
            Ok(PeerRecord {
                node_id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                os: row.get(3)?,
                last_heartbeat: row.get(4)?,
                online: online != 0,
            })
        }).ok();

        match rows {
            Some(iter) => iter.filter_map(|r| r.ok()).collect(),
            None => Vec::new(),
        }
    }

    fn get_machine_by_node_id(&self, node_id: &str) -> Result<Option<PeerRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT node_id, user_id, name, os, last_heartbeat, online
             FROM machines WHERE node_id = ?1",
        )?;

        let mut rows = stmt.query_map(params![node_id], |row| {
            let online: i32 = row.get(5)?;
            Ok(PeerRecord {
                node_id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                os: row.get(3)?,
                last_heartbeat: row.get(4)?,
                online: online != 0,
            })
        })?;

        if let Some(row) = rows.next() {
            Ok(Some(row?))
        } else {
            Ok(None)
        }
    }
}

// MySQL 后端实现
// ═══════════════════════════════════════════════════════════════════════

use mysql_async::{Pool, prelude::*};

/// MySQL 数据库访问对象
struct MysqlDatabase {
    pool: Arc<Pool>,
}

impl MysqlDatabase {
    /// 创建 MySQL 连接池
    async fn new(database_url: &str) -> Result<Self> {
        let pool = Pool::new(database_url);
        log::info!("MySQL database connected: {}", database_url);
        Ok(Self { pool: Arc::new(pool) })
    }

    async fn create_tables(&self) -> Result<()> {
        let mut conn = self.pool.get_conn().await?;
        
        conn.query_drop(
            "CREATE TABLE IF NOT EXISTS users (
                id            BIGINT PRIMARY KEY AUTO_INCREMENT,
                username      VARCHAR(255) NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                created_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            )"
        ).await.context("Failed to create MySQL users table")?;

        conn.query_drop(
            "CREATE TABLE IF NOT EXISTS machines (
                node_id         VARCHAR(255) PRIMARY KEY,
                user_id         BIGINT,
                name            VARCHAR(255) NOT NULL DEFAULT '',
                os              VARCHAR(255) NOT NULL DEFAULT '',
                last_heartbeat  BIGINT NOT NULL DEFAULT 0,
                online          TINYINT NOT NULL DEFAULT 0,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE SET NULL
            )"
        ).await.context("Failed to create MySQL machines table")?;

        log::info!("MySQL tables verified/created");
        Ok(())
    }

    async fn create_user(&self, username: &str, password_hash: &str) -> Result<i64> {
        let mut conn = self.pool.get_conn().await?;
        
        conn.exec_drop(
            "INSERT INTO users (username, password_hash) VALUES (?, ?)",
            (username, password_hash),
        ).await.with_context(|| format!("Failed to create user '{}'", username))?;
        
        let id: i64 = conn.last_insert_id().unwrap_or(0) as i64;
        log::info!("User '{}' created with id={}", username, id);
        Ok(id)
    }

    async fn get_user_by_username(&self, username: &str) -> Option<UserRecord> {
        let mut conn = self.pool.get_conn().await.ok()?;
        
        let result = conn.exec_map(
            "SELECT id, username, password_hash, DATE_FORMAT(created_at, '%Y-%m-%d %H:%i:%s') as created_at FROM users WHERE username = ?",
            (username,),
            |(id, username, password_hash, created_at): (i64, String, String, String)| {
                UserRecord { id, username, password_hash, created_at }
            }
        ).await.ok()?;
        
        result.into_iter().next()
    }

    async fn get_user_by_id(&self, id: i64) -> Option<UserRecord> {
        let mut conn = self.pool.get_conn().await.ok()?;
        
        let result = conn.exec_map(
            "SELECT id, username, password_hash, DATE_FORMAT(created_at, '%Y-%m-%d %H:%i:%s') as created_at FROM users WHERE id = ?",
            (id,),
            |(id, username, password_hash, created_at): (i64, String, String, String)| {
                UserRecord { id, username, password_hash, created_at }
            }
        ).await.ok()?;
        
        result.into_iter().next()
    }

    async fn upsert_machine(&self, node_id: &str, user_id: Option<i64>, name: &str, os: &str) -> Result<()> {
        let mut conn = self.pool.get_conn().await?;
        let now = chrono::Utc::now().timestamp();
        
        conn.exec_drop(
            "INSERT INTO machines (node_id, user_id, name, os, last_heartbeat, online)
             VALUES (?, ?, ?, ?, ?, 1)
             ON DUPLICATE KEY UPDATE
                 user_id = VALUES(user_id),
                 name = VALUES(name),
                 os = VALUES(os),
                 online = 1,
                 last_heartbeat = VALUES(last_heartbeat)",
            (node_id, user_id, name, os, now),
        ).await.with_context(|| format!("Failed to upsert machine '{}'", node_id))?;
        
        log::info!("Peer '{}' upserted (user_id={:?})", node_id, user_id);
        Ok(())
    }

    async fn update_heartbeat(&self, node_id: &str) -> Result<()> {
        let mut conn = self.pool.get_conn().await?;
        let now = chrono::Utc::now().timestamp();
        
        conn.exec_drop(
            "UPDATE machines SET last_heartbeat = ?, online = 1 WHERE node_id = ?",
            (now, node_id),
        ).await.with_context(|| format!("Failed to update heartbeat for '{}'", node_id))?;
        
        Ok(())
    }

    async fn get_online_machines(&self, user_id: i64) -> Vec<PeerRecord> {
        let mut conn = match self.pool.get_conn().await {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        
        conn.exec_map(
            "SELECT node_id, user_id, name, os, last_heartbeat, online
             FROM machines WHERE user_id IS NOT NULL AND user_id = ? AND online = 1",
            (user_id,),
            |(node_id, user_id, name, os, last_heartbeat, online): (String, Option<i64>, String, String, i64, i8)| {
                PeerRecord { node_id, user_id, name, os, last_heartbeat, online: online != 0 }
            }
        ).await.unwrap_or_default()
    }

    async fn get_all_online_machines(&self) -> Vec<PeerRecord> {
        let mut conn = match self.pool.get_conn().await {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        
        conn.exec_map(
            "SELECT node_id, user_id, name, os, last_heartbeat, online FROM machines WHERE online = 1",
            (),
            |(node_id, user_id, name, os, last_heartbeat, online): (String, Option<i64>, String, String, i64, i8)| {
                PeerRecord { node_id, user_id, name, os, last_heartbeat, online: online != 0 }
            }
        ).await.unwrap_or_default()
    }

    async fn mark_offline_machines(&self, timeout_secs: i64) -> Result<Vec<String>> {
        let mut conn = self.pool.get_conn().await?;
        let now = chrono::Utc::now().timestamp();
        let cutoff = now - timeout_secs;
        
        let offlined: Vec<String> = conn.exec_map(
            "SELECT node_id FROM machines WHERE online = 1 AND last_heartbeat < ?",
            (cutoff,),
            |node_id: String| node_id
        ).await.context("Failed to query offline machines")?;
        
        if offlined.is_empty() {
            return Ok(Vec::new());
        }
        
        conn.exec_drop(
            "UPDATE machines SET online = 0 WHERE online = 1 AND last_heartbeat < ?",
            (cutoff,),
        ).await.context("Failed to mark offline machines")?;
        
        for mid in &offlined {
            log::warn!("Peer '{}' marked offline (timeout)", mid);
        }
        
        Ok(offlined)
    }

    async fn machine_unbind(&self, node_id: &str, user_id: i64) -> Result<bool> {
        let mut conn = self.pool.get_conn().await?;
        
        conn.exec_drop(
            "UPDATE machines SET user_id = NULL WHERE node_id = ? AND user_id = ?",
            (node_id, user_id),
        ).await.with_context(|| format!("Failed to unbind machine '{}'", node_id))?;
        
        // Get affected rows count
        let affected: u64 = conn.query_first::<u64, _>("SELECT ROW_COUNT()")
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
        
        let unbound = affected > 0;
        if unbound {
            log::info!("Peer '{}' unbound from user_id={}", node_id, user_id);
        } else {
            log::warn!("Peer '{}' not found or not bound to user_id={}", node_id, user_id);
        }
        Ok(unbound)
    }

    async fn get_user_machines(&self, user_id: i64) -> Vec<PeerRecord> {
        let mut conn = match self.pool.get_conn().await {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        
        conn.exec_map(
            "SELECT node_id, user_id, name, os, last_heartbeat, online
             FROM machines WHERE user_id = ?",
            (user_id,),
            |(node_id, user_id, name, os, last_heartbeat, online): (String, Option<i64>, String, String, i64, i8)| {
                PeerRecord { node_id, user_id, name, os, last_heartbeat, online: online != 0 }
            }
        ).await.unwrap_or_default()
    }

    async fn get_machine_by_node_id(&self, node_id: &str) -> Result<Option<PeerRecord>> {
        let mut conn = self.pool.get_conn().await?;
        
        let result = conn.exec_map(
            "SELECT node_id, user_id, name, os, last_heartbeat, online
             FROM machines WHERE node_id = ?",
            (node_id,),
            |(node_id, user_id, name, os, last_heartbeat, online): (String, Option<i64>, String, String, i64, i8)| {
                PeerRecord { node_id, user_id, name, os, last_heartbeat, online: online != 0 }
            }
        ).await?;
        
        Ok(result.into_iter().next())
    }
}
