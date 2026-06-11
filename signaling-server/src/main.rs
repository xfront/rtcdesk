//! signaling-server entry point.
//!
//! WebRTC remote desktop signaling server using ice0 RPC server.
//! Handles user authentication, machine registration, heartbeat, and online status management.

mod auth;
mod connection;
mod db;
mod machine;
mod servant;
mod subscription;

use shared::ice0_service::SignalServiceServant;
use std::sync::Arc;
use ice0::prelude::*;

/// 确保至少存在一个默认用户（admin/admin），方便首次启动测试。
fn ensure_default_admin(db: &db::Database) -> anyhow::Result<()> {
    use argon2::password_hash::SaltString;
    use argon2::password_hash::rand_core::OsRng;
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
    let bind_addr = std::env::var("RTCDESK_ADDR").unwrap_or_else(|_| "0.0.0.0:9009".to_string());
    let db_path = std::env::var("RTCDESK_DB_PATH").unwrap_or_else(|_| "./rtcdesk.db".to_string());
    let jwt_secret = resolve_jwt_secret();

    // 2. 初始化数据库
    let database = db::Database::new(&db_path)?;
    let db = Arc::new(database);

    // 2.1 如果没有用户，创建默认 admin/admin 用户
    ensure_default_admin(&db)?;

    // 3. 初始化认证服务
    let auth_service = Arc::new(auth::AuthService::new(db.clone(), jwt_secret));

    // 4. 初始化机器注册表
    let machine_registry = Arc::new(machine::PeerRegistry::new(db.clone(), 90));

    // 4.1 初始化连接管理器
    let connection_manager = Arc::new(connection::ConnectionManager::new());

    // 4.2 初始化订阅管理器
    let subscription_manager = Arc::new(subscription::SubscriptionManager::new());

    // 4.3 设置连接断开回调
    let sub_clone = subscription_manager.clone();
    let db_clone = db.clone();
    connection_manager
        .set_disconnect_callback(move |host_id| {
            let sub_clone = sub_clone.clone();
            let db_clone = db_clone.clone();

            // Spawn a task to handle the disconnect notification
            tokio::spawn(async move {
                log::info!("Handling disconnect for machine: {}", host_id);

                // Get machine info from database
                if let Ok(Some(peer)) = db_clone.get_machine_by_node_id(&host_id) {
                    let change = shared::ice0_service::PeerStatusChange {
                        node_id: peer.node_id.clone(),
                        name: peer.name.clone(),
                        os: peer.os.clone(),
                        online: false,
                    };

                    // Broadcast offline status to all subscribers of the same user
                    log::info!(
                        "Broadcasting offline status for machine {} (user {:?})",
                        host_id,
                        peer.user_id
                    );
                    if let Some(uid) = peer.user_id {
                        sub_clone.broadcast_peer_status(uid, change).await;
                    }
                } else {
                    log::warn!(
                        "Machine {} not found in database during disconnect",
                        host_id
                    );
                }
            });
        })
        .await;

    log::info!("signaling-server initialized");
    log::info!("Database: {}", db_path);
    log::info!("Bind address: {}", bind_addr);

    // 5. Create RPC implementation
    let service_impl = servant::SignalServiceImpl {
        auth: auth_service.clone(),
        machines: machine_registry.clone(),
        connections: connection_manager.clone(),
        subscriptions: subscription_manager.clone(),
    };

    // 6. Start ice0 RPC server
    let communicator = Communicator::default().await?;
    let socket_addr: std::net::SocketAddr = bind_addr.parse()?;
    let adapter = communicator.create_object_adapter(
        "SignalServiceAdapter",
        Some(format!("tcp -h 0.0.0.0 -p {}", socket_addr.port())),
        None,
        None,
    ).await?;
    
    // Register SignalService servant
    let servant = Arc::new(SignalServiceServant::new(service_impl));
    let identity = Identity::new("SignalService");
    adapter.add_servant(identity, servant).await;
    let _ = adapter.activate().await;  // Ignore result - adapter activation is best-effort
    
    log::info!("ice0 signaling-server listening on {}", bind_addr);

    // 7. Start heartbeat cleanup timer（每 60 秒）
    let mach_clone = machine_registry.clone();
    let sub_clone = subscription_manager.clone();
    let db_clone = db.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            match mach_clone.check_timeouts() {
                Ok(offlined) => {
                    for node_id in offlined {
                        log::info!("Machine {} timed out, marked offline", node_id);
                        // Broadcast offline status to all subscribers of the same user
                        if let Ok(Some(peer)) = db_clone.get_machine_by_node_id(&node_id) {
                            let change = shared::ice0_service::PeerStatusChange {
                                node_id: peer.node_id.clone(),
                                name: peer.name.clone(),
                                os: peer.os.clone(),
                                online: false,
                            };
                            // Broadcast to the same user (if bound)
                            if let Some(uid) = peer.user_id {
                                sub_clone.broadcast_peer_status(uid, change).await;
                            }
                        }
                    }
                }
                Err(e) => log::error!("Heartbeat check error: {}", e),
            }
        }
    });

    // 8. Wait for shutdown
    tokio::signal::ctrl_c().await?;
    log::info!("Shutting down...");
    
    Ok(())
}
