//! 认证服务。
//!
//! 提供用户注册、登录、JWT 签发/验证/刷新等功能。
//! 密码使用 Argon2id 哈希，JWT 使用 HS256 算法。

use anyhow::{bail, Context, Result};
use std::sync::Arc;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};

use shared::auth::{Claims, JWT_EXPIRE_SECS, JWT_REFRESH_BEFORE_SECS};
use shared::signal::PeerRole;

use crate::db::Database;

// ─── AuthService ──────────────────────────────────────────────────────

/// 认证服务，封装密码哈希和 JWT 操作。
pub struct AuthService {
    db: Arc<Database>,
    jwt_secret: String,
}

impl AuthService {
    /// 创建新的认证服务实例。
    pub fn new(db: Arc<Database>, jwt_secret: String) -> Self {
        Self { db, jwt_secret }
    }

    /// 注册新用户。
    ///
    /// 返回 `(user_id, jwt_token)`。
    /// 如果用户名已存在，返回错误。
    pub fn register(&self, username: &str, password: &str) -> Result<(i64, String)> {
        // 检查用户名是否已存在
        if self.db.get_user_by_username(username).is_some() {
            bail!("Username '{}' already exists", username);
        }

        // 哈希密码
        let password_hash = self.hash_password(password)?;

        // 插入数据库
        let user_id = self.db.create_user(username, &password_hash)?;

        // 签发 JWT
        let token = self.issue_token(user_id, username, PeerRole::Host)?;

        log::info!("User '{}' registered (id={})", username, user_id);
        Ok((user_id, token))
    }

    /// 用户登录。
    ///
    /// 验证用户名和密码，成功后返回 JWT token。
    pub fn login(&self, username: &str, password: &str) -> Result<String> {
        let user = self
            .db
            .get_user_by_username(username)
            .with_context(|| format!("User '{}' not found", username))?;

        // 验证密码
        let parsed_hash = PasswordHash::new(&user.password_hash)
            .map_err(|e| anyhow::anyhow!("Failed to parse stored password hash: {}", e))?;
        if Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_err()
        {
            bail!("Invalid password for user '{}'", username);
        }

        // 签发 JWT
        let token = self.issue_token(user.id, &user.username, PeerRole::Host)?;

        log::info!("User '{}' logged in", username);
        Ok(token)
    }

    /// 验证 JWT token，返回 Claims。
    pub fn verify_token(&self, token: &str) -> Result<Claims> {
        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &Validation::new(jsonwebtoken::Algorithm::HS256),
        )
        .context("Invalid or expired JWT token")?;

        Ok(token_data.claims)
    }

    /// 刷新 JWT token。
    ///
    /// 若当前 token 在过期前 `JWT_REFRESH_BEFORE_SECS` 秒内，签发新 token。
    /// 若 token 仍然有效但不在刷新窗口内，也返回新 token（方便客户端使用）。
    /// 若 token 已过期，返回错误。
    pub fn refresh_token(&self, token: &str) -> Result<String> {
        let claims = self.verify_token(token)?;

        let now = chrono::Utc::now().timestamp();
        let remaining = claims.exp - now;

        // token 已过期（verify_token 应该已经拒绝，但保险起见）
        if remaining <= 0 {
            bail!("Token has expired");
        }

        // 在刷新窗口内或已接近过期，签发新 token
        if remaining <= JWT_REFRESH_BEFORE_SECS {
            let new_token = self.issue_token(
                claims.sub.parse::<i64>().unwrap_or(0),
                &claims.username,
                claims.role,
            )?;
            log::info!("Token refreshed for user '{}'", claims.username);
            return Ok(new_token);
        }

        // 不在刷新窗口内，仍返回新 token 以延长会话
        let new_token = self.issue_token(
            claims.sub.parse::<i64>().unwrap_or(0),
            &claims.username,
            claims.role,
        )?;
        log::debug!("Token re-issued for user '{}' (not in refresh window)", claims.username);
        Ok(new_token)
    }

    /// 使用 Argon2id 对密码进行哈希。
    pub fn hash_password(&self, password: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| anyhow::anyhow!("Failed to hash password: {}", e))?;
        Ok(hash.to_string())
    }

    // ─── 内部辅助 ──────────────────────────────────────────────────

    /// 签发 JWT token。
    fn issue_token(&self, user_id: i64, username: &str, role: PeerRole) -> Result<String> {
        let now = chrono::Utc::now().timestamp();
        let claims = Claims {
            sub: user_id.to_string(),
            username: username.to_string(),
            role,
            exp: now + JWT_EXPIRE_SECS,
            iat: now,
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.jwt_secret.as_bytes()),
        )
        .context("Failed to encode JWT")?;

        Ok(token)
    }
}
