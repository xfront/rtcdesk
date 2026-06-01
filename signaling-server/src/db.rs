//! SQLite 数据访问层。
//!
//! 提供 users 和 machines 表的 CRUD 操作，
//! 使用 rusqlite + params! 宏防止 SQL 注入。

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::sync::Mutex;

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
pub struct MachineRecord {
    pub host_id: String,
    pub user_id: i64,
    pub name: String,
    pub os: String,
    pub last_heartbeat: i64,
    pub online: bool,
}

// ─── Database 结构体 ──────────────────────────────────────────────────

/// SQLite 数据库访问对象（内部持有 Mutex<Connection>，线程安全）。
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// 打开（或创建）SQLite 数据库文件，并执行建表。
    pub fn new(db_path: &str) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open database at {}", db_path))?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.create_tables()?;
        log::info!("Database opened: {}", db_path);
        Ok(db)
    }

    /// 执行建表 SQL（幂等）。
    fn create_tables(&self) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS users (
                    id            INTEGER PRIMARY KEY AUTOINCREMENT,
                    username      TEXT NOT NULL UNIQUE,
                    password_hash TEXT NOT NULL,
                    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
                );

                CREATE TABLE IF NOT EXISTS machines (
                    host_id         TEXT PRIMARY KEY,
                    user_id         INTEGER NOT NULL,
                    name            TEXT NOT NULL DEFAULT '',
                    os              TEXT NOT NULL DEFAULT '',
                    last_heartbeat  INTEGER NOT NULL DEFAULT 0,
                    online          INTEGER NOT NULL DEFAULT 0,
                    FOREIGN KEY (user_id) REFERENCES users(id)
                );
                ",
            )
            .context("Failed to create tables")?;
        log::info!("Database tables verified/created");
        Ok(())
    }

    /// 插入新用户，返回自增 id。
    pub fn create_user(&self, username: &str, password_hash: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO users (username, password_hash) VALUES (?1, ?2)",
            params![username, password_hash],
        )
        .with_context(|| format!("Failed to create user '{}'", username))?;
        let id = conn.last_insert_rowid();
        log::info!("User '{}' created with id={}", username, id);
        Ok(id)
    }

    /// 按用户名查找用户。
    pub fn get_user_by_username(&self, username: &str) -> Option<UserRecord> {
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
        })
        .ok()
    }

    /// 按 id 查找用户。
    pub fn get_user_by_id(&self, id: i64) -> Option<UserRecord> {
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
        })
        .ok()
    }

    /// 插入或更新机器信息（UPSERT）。
    pub fn upsert_machine(
        &self,
        host_id: &str,
        user_id: i64,
        name: &str,
        os: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO machines (host_id, user_id, name, os, last_heartbeat, online)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1)
                 ON CONFLICT(host_id) DO UPDATE SET
                     user_id = excluded.user_id,
                     name = excluded.name,
                     os = excluded.os,
                     online = 1,
                     last_heartbeat = excluded.last_heartbeat",
            params![host_id, user_id, name, os, chrono::Utc::now().timestamp()],
        )
            .with_context(|| format!("Failed to upsert machine '{}'", host_id))?;
        log::info!("Machine '{}' upserted for user_id={}", host_id, user_id);
        Ok(())
    }

    /// 更新心跳时间戳并标记在线。
    pub fn update_heartbeat(&self, host_id: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE machines SET last_heartbeat = ?1, online = 1 WHERE host_id = ?2",
            params![now, host_id],
        )
            .with_context(|| format!("Failed to update heartbeat for '{}'", host_id))?;
        Ok(())
    }

    /// 获取指定用户的所有在线机器。
    pub fn get_online_machines(&self, user_id: i64) -> Vec<MachineRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT host_id, user_id, name, os, last_heartbeat, online
             FROM machines WHERE user_id = ?1 AND online = 1",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = stmt
            .query_map(params![user_id], |row| {
                let online: i32 = row.get(5)?;
                Ok(MachineRecord {
                    host_id: row.get(0)?,
                    user_id: row.get(1)?,
                    name: row.get(2)?,
                    os: row.get(3)?,
                    last_heartbeat: row.get(4)?,
                    online: online != 0,
                })
            })
            .ok();

        match rows {
            Some(iter) => iter.filter_map(|r| r.ok()).collect(),
            None => Vec::new(),
        }
    }

    /// 获取所有在线机器。
    pub fn get_all_online_machines(&self) -> Vec<MachineRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT host_id, user_id, name, os, last_heartbeat, online
             FROM machines WHERE online = 1",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = stmt
            .query_map([], |row| {
                let online: i32 = row.get(5)?;
                Ok(MachineRecord {
                    host_id: row.get(0)?,
                    user_id: row.get(1)?,
                    name: row.get(2)?,
                    os: row.get(3)?,
                    last_heartbeat: row.get(4)?,
                    online: online != 0,
                })
            })
            .ok();

        match rows {
            Some(iter) => iter.filter_map(|r| r.ok()).collect(),
            None => Vec::new(),
        }
    }

    /// 标记超时机器为离线，返回本次变为离线的 host_id 列表。
    pub fn mark_offline_machines(&self, timeout_secs: i64) -> Result<Vec<String>> {
        let now = chrono::Utc::now().timestamp();
        let cutoff = now - timeout_secs;

        let conn = self.conn.lock().unwrap();

        // 先查出即将变为离线的机器
        let mut stmt = conn.prepare(
            "SELECT host_id FROM machines WHERE online = 1 AND last_heartbeat < ?1",
        )?;

        let offlined: Vec<String> = stmt
            .query_map(params![cutoff], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        if offlined.is_empty() {
            return Ok(Vec::new());
        }

        // 批量更新为离线
        conn.execute(
            "UPDATE machines SET online = 0 WHERE online = 1 AND last_heartbeat < ?1",
            params![cutoff],
        )
        .with_context(|| "Failed to mark offline machines")?;

        for mid in &offlined {
            log::warn!("Machine '{}' marked offline (timeout)", mid);
        }

        Ok(offlined)
    }
}
