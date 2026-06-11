//! 机器注册/心跳/在线状态管理。
//!
//! 提供机器注册、心跳更新、在线列表查询及超时离线检测。

use anyhow::Result;
use std::sync::Arc;

use shared::ice0_service::PeerInfo;

use crate::db::Database;

// ─── PeerRegistry ──────────────────────────────────────────────────

/// 机器注册表，管理机器的注册、心跳和在线状态。
pub struct PeerRegistry {
    pub db: Arc<Database>,
    heartbeat_timeout_secs: i64,
}

impl PeerRegistry {
    /// 创建新的机器注册表。
    pub fn new(db: Arc<Database>, heartbeat_timeout_secs: i64) -> Self {
        Self {
            db,
            heartbeat_timeout_secs,
        }
    }

    /// 注册或更新机器信息。
    ///
    /// 如果机器已存在，更新 name/os/online 状态；
    /// 如果不存在，插入新记录。
    pub fn register(
        &self,
        node_id: &str,
        user_id: Option<i64>,  // None means unassigned
        name: &str,
        os: &str,
    ) -> Result<()> {
        self.db.upsert_machine(node_id, user_id, name, os)?;
        log::info!(
            "Peer '{}' registered (user_id={:?}, name='{}', os='{}')",
            node_id,
            user_id,
            name,
            os
        );
        Ok(())
    }

    /// 处理心跳消息。
    ///
    /// 更新机器的 last_heartbeat 时间戳并标记为在线。
    pub fn heartbeat(&self, node_id: &str) -> Result<()> {
        self.db.update_heartbeat(node_id)?;
        log::debug!("Heartbeat received from machine '{}'", node_id);
        Ok(())
    }

    /// 获取指定用户的在线机器列表。
    pub fn get_online_machines(&self, user_id: i64) -> Vec<PeerInfo> {
        self.db
            .get_online_machines(user_id)
            .into_iter()
            .map(|r| PeerInfo {
                node_id: r.node_id,
                name: r.name,
                os: r.os,
                online: r.online,
            })
            .collect()
    }

    /// 获取指定用户的在线机器列表（用户ID为字符串）。
    pub fn get_online_machines_for_user(&self, user_id: &str) -> Vec<PeerInfo> {
        // Parse user_id string to i64, default to -1 if invalid
        let uid = user_id.parse::<i64>().unwrap_or(-1);
        self.get_online_machines(uid)
    }

    /// 获取所有在线机器列表。
    pub fn get_all_online_machines(&self) -> Vec<PeerInfo> {
        self.db
            .get_all_online_machines()
            .into_iter()
            .map(|r| PeerInfo {
                node_id: r.node_id,
                name: r.name,
                os: r.os,
                online: r.online,
            })
            .collect()
    }

    /// 检查心跳超时，将超时机器标记为离线。
    ///
    /// 返回本次变为离线的 node_id 列表（用于广播通知）。
    pub fn check_timeouts(&self) -> Result<Vec<String>> {
        let offlined = self.db.mark_offline_machines(self.heartbeat_timeout_secs)?;
        if !offlined.is_empty() {
            log::info!(
                "Heartbeat timeout check: {} machine(s) marked offline",
                offlined.len()
            );
        }
        Ok(offlined)
    }
}
