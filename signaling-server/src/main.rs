//! signaling-server 入口。
//!
//! WebRTC 远程桌面信令服务器：处理用户认证、机器注册、心跳和在线状态管理。
//! WebSocket 信令路由在 T03 中实现。

mod auth;
mod db;
mod machine;
mod ws_handler;

use std::sync::Arc;

/// 确保至少存在一个默认用户（admin/admin），方便首次启动测试。
fn ensure_default_admin(db: &db::Database) -> anyhow::Result<()> {
    use argon2::password_hash::rand_core::OsRng;
    use argon2::password_hash::SaltString;
    use argon2::{Argon2, PasswordHasher};

    if db.get_user_by_username("admin").is_some() {
        log::info!("Default admin user already exists");
        return Ok(());
    }

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(b"admin123", &salt)
        .map_err(|e| anyhow::anyhow!("Failed to hash default password: {}", e))?
        .to_string();

    db.create_user("admin", &password_hash)?;
    log::info!("Default admin user created (username='admin', password='admin123')");
    Ok(())
}

/// 解析 JWT 密钥：优先环境变量 → 其次文件 → 最后自动生成。
fn resolve_jwt_secret() -> String {
    // 1. 环境变量优先
    if let Ok(secret) = std::env::var("RTCDESK_JWT_SECRET") {
        if !secret.is_empty() {
            return secret;
        }
    }

    // 2. 尝试从持久化文件读取
    const SECRET_FILE: &str = "jwt_secret.txt";
    if let Ok(secret) = std::fs::read_to_string(SECRET_FILE) {
        let trimmed = secret.trim().to_string();
        if !trimmed.is_empty() {
            log::info!("JWT secret loaded from {}", SECRET_FILE);
            return trimmed;
        }
    }

    // 3. 自动生成并持久化
    let secret = uuid::Uuid::new_v4().to_string();
    if let Err(e) = std::fs::write(SECRET_FILE, &secret) {
        log::warn!("Failed to persist JWT secret to {}: {}", SECRET_FILE, e);
    } else {
        log::info!("JWT secret generated and saved to {}", SECRET_FILE);
    }
    secret
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    // 1. 解析环境变量
    let bind_addr =
        std::env::var("RTCDESK_ADDR").unwrap_or_else(|_| "0.0.0.0:9009".to_string());
    let db_path = std::env::var("RTCDESK_DB_PATH")
        .unwrap_or_else(|_| "./rtcdesk.db".to_string());
    let jwt_secret = resolve_jwt_secret();

    // 2. 初始化数据库
    let database = db::Database::new(&db_path)?;
    let db = Arc::new(database);

    // 2.1 如果没有用户，创建默认 admin/admin 用户
    ensure_default_admin(&db)?;

    // 3. 初始化认证服务
    let auth_service = Arc::new(auth::AuthService::new(db.clone(), jwt_secret));

    // 4. 初始化机器注册表
    let machine_registry = Arc::new(machine::MachineRegistry::new(db.clone(), 90));

    log::info!("signaling-server initialized");
    log::info!("Database: {}", db_path);
    log::info!("Bind address: {}", bind_addr);

    // 5. 启动心跳清理定时器（每 60 秒）
    let mach_clone = machine_registry.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            match mach_clone.check_timeouts() {
                Ok(offlined) => {
                    for mid in offlined {
                        log::info!("Machine {} timed out, marked offline", mid);
                        // TODO: T03 中广播 MachineStatus{machine_id: mid, online: false}
                    }
                }
                Err(e) => log::error!("Heartbeat check error: {}", e),
            }
        }
    });

    // 6. 构建共享应用状态
    let peer_registry = Arc::new(ws_handler::PeerRegistry::new());
    let app_state = ws_handler::AppState {
        auth: auth_service,
        machines: machine_registry,
        peers: peer_registry,
    };

    // 7. 配置 axum 路由
    let app = axum::Router::new()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .route("/ws", axum::routing::get(ws_handler::ws_upgrade))
        .with_state(app_state);

    log::info!("signaling-server running on {}", bind_addr);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
