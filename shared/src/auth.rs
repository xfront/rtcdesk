//! JWT 认证相关类型与常量。
//!
//! 定义信令服务器和客户端共用的 Claims 结构及有效期常量。

use serde::{Deserialize, Serialize};
use crate::signal::PeerRole;

/// JWT Claims 结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// 用户 ID
    pub sub: String,
    /// 用户名
    pub username: String,
    /// 角色
    pub role: PeerRole,
    /// 过期时间（Unix timestamp）
    pub exp: i64,
    /// 签发时间
    pub iat: i64,
}

/// JWT token 有效期（秒）
pub const JWT_EXPIRE_SECS: i64 = 86400; // 24小时

/// 过期前多久可刷新（秒）
pub const JWT_REFRESH_BEFORE_SECS: i64 = 3600; // 1小时
