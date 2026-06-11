# RTCDesk — 基于 WebRTC 的跨平台远程桌面

RTCDesk 是一款使用 Rust 编写的开源远程桌面工具，基于 WebRTC 实现 P2P 加密传输，无需中继服务器转发画面数据。支持屏幕共享、远程控制、剪贴板同步和文件传输。

**RPC 框架**: 使用 **ice0 v0.0.1** 实现双向 RPC 通信，支持服务端回调客户端。

## 架构设计

### 核心设计理念

RTCDesk 采用 **Client-Host-Signal 三层架构**，通过 ice0 RPC 实现双向通信：

- **Client (Viewer)**: GUI 应用，负责用户交互和远程桌面显示
- **Host-Service**: 后台服务，负责所有 Signal 交互和屏幕捕获
- **Signal Server**: 信令服务器，负责认证、机器注册和信令转发
- **IPC 代理**: Client 与 Host-Service 通过 ice0 over interprocess 通信，Host-Service 代理所有 Signal 交互
- **双向 RPC**: Signal Server 可回调 Host-Service（如通知机器状态变更），Host-Service 可回调 Client（如通知连接状态）

### 功能需求

#### Host-Service 功能

1. **可安装为系统服务** 
   - Linux: systemd user service
   - Windows: Windows Service
   - 配置文件: `~/.config/rtcdesk/rtcdesk.json`

2. **可由 Client 拉起**
   - Client 启动时自动检测并启动 host-service
   - 通过 `HostProcess::spawn()` 派生子进程

3. **负责与 Signal 所有交互**
   - 启动时机器注册上线 (`Register` + `PeerJoin`)
   - 代理 Client 的登录/登出请求
   - 代理机器列表请求与响应
   - 广播机器状态变更通知

4. **负责接受远程 Client 连接控制**
   - WebRTC 连接建立 (Offer/Answer/ICE 交换)
   - 屏幕捕获与编码 (支持 VA-API 硬件加速)
   - 响应远程鼠标/键盘输入
   - 剪贴板双向同步
   - 文件传输

#### Client 功能

1. **启动时拉起 Host**
   - 检测 host-service 是否运行
   - 如未运行，自动启动 host 进程
   - 等待 IPC 连接就绪

2. **自动登录 (Host 代理)**
   - 从配置文件读取登录凭据
   - 通过 IPC 发送登录命令到 host-service
   - 接收登录结果并更新 UI 状态

3. **拉取最新机器列表 (Host 代理)**
   - 通过 IPC 发送机器列表请求
   - Host-service 转发到 signal server
   - 接收并显示机器列表及在线状态
   - 实时接收机器状态变更通知

### 整体架构图

```
┌────────────────────────────────────────────────────────────────────────┐
│                          RTCDesk 架构 (Workspace)                      │
├────────────────────────────────────────────────────────────────────────┤
│                                                                        │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │                    Client GUI (Viewer)                           │  │
│  │                                                                  │  │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌────────────┐        │  │
│  │  │ Auto-    │  │ Auto-    │  │ Peer     │  │ Remote     │        │  │
│  │  │ Launch   │  │ Login    │  │ List     │  │ Desktop    │        │  │
│  │  │ Host     │  │ (IPC)    │  │ (IPC)    │  │ Viewer     │        │  │
│  │  └────┬─────┘  └────┬─────┘  └────┬─────┘  └─────┬──────┘        │  │
│  │       └──────────────┴─────────────┴──────────────┘              │  │
│  │                          │ ice0 RPC (interprocess)               │  │
│  └──────────────────────────┼───────────────────────────────────────┘  │
│                             │                                          │
│  ┌──────────────────────────┼──────────────────────────────────────┐   │
│  │                    Host-Service                                 │   │
│  │                                                                 │   │
│  │  ┌───────────────────────┼──────────────────────────────────┐   │   │
│  │  │  IPC Server           │                                  │   │   │
│  │  │  ├─ Login/Logout      │                                  │   │   │
│  │  │  ├─ PeerList          │                                  │   │   │
│  │  │  ├─ WebRTC Signaling  │                                  │   │   │
│  │  │  └─ Status Broadcast  │                                  │   │   │
│  │  └───────────────────────┼──────────────────────────────────┘   │   │
│  │                          │ ice0 RPC (interprocess)              │   │
│  │  ┌───────────────────────┼──────────────────────────────────┐   │   │
│  │  │  Signal Client        │                                  │   │   │
│  │  │  ├─ Register/ Login   │                                  │   │   │
│  │  │  ├─ Peer Register     │                                  │   │   │
│  │  │  ├─ Peer List         │                                  │   │   │
│  │  │  └─ WebRTC Peer       │                                  │   │   │
│  │  └───────────────────────┼──────────────────────────────────┘   │   │
│  │                          │ ice0 RPC (TCP)                       │   │
│  │  ┌───────────────────────┼──────────────────────────────────┐   │   │
│  │  │  Screen Capturer      │                                  │   │   │
│  │  │  ├─ VA-API Encode     │                                  │   │   │
│  │  │  ├─ Input Handler     │                                  │   │   │
│  │  │  └─ Clipboard Sync    │                                  │   │   │
│  │  └──────────────────────────────────────────────────────────┘   │   │
│  └─────────────────────────────────────────────────────────────────┘   │
│                             │ ice0 RPC (TCP)                           │
│  ┌──────────────────────────┼──────────────────────────────────────┐   │
│  │                    Signal Server                                │   │
│  │  ┌──────────────────────┼──────────────────────────────────┐    │   │
│  │  │  Authentication      │                                  │    │   │
│  │  │  Peer Registry       │                                  │    │   │
│  │  │  ice0 RPC Server     │                                  │    │   │
│  │  └─────────────────────────────────────────────────────────┘    │   │
│  └─────────────────────────────────────────────────────────────────┘   │
│                                                                        │
│  ┌───────────────────┐                                                 │
│  │   shared          │  共享模块：帧协议、信令消息、IPC协议、本地ID    │
│  └───────────────────┘                                                 │
└────────────────────────────────────────────────────────────────────────┘
```

## 连接流程

### 启动流程

```
[Client 启动]
    ↓
读取配置文件 (~/.config/rtcdesk/rtcdesk.json)
    ↓
检查 host-service 是否运行
    ├─ 已运行 → 跳过
    └─ 未运行 → 启动 host-service 进程
    ↓
等待 IPC 连接建立 (最多重试5次)
    ↓
延迟3秒后尝试自动登录 (如配置了凭据)
    ↓
请求机器列表
    ↓
显示主界面
```

### WebRTC 连接流程

**架构说明**: Host-Service 分为两部分：
- **本地 Host-Service**: 运行在 Viewer 端，负责代理 Viewer 的 Signal 请求
- **被控 Host-Service**: 运行在被控机器端，负责屏幕捕获、WebRTC 连接建立和输入响应

```
Viewer (Client)    本地 Host-Service    Signal Server    被控 Host-Service
   │                      │                  │                  │
   │── Auto Login (IPC) ─►│                  │                  │
   │                      │── Login ─────────►│                  │
   │◄─ LoginResult (IPC) ◄│◄─ JWT Token ─────│                  │
   │                      │                  │                  │
   │── PeerList (IPC) ───►│                  │                  │
   │                      │── GetPeerList ───►│                  │
   │◄─ PeerList (IPC) ───◄│◄─ PeerList ──────│                  │
   │                      │                  │                  │
   │── ConnectRequest ───►│                  │                  │
   │   (IPC)              │── ConnectRequest─►│                  │
   │                      │                  │── Callback ─────►│
   │                      │                  │  (HostCallback)  │
   │                      │                  │                  │
   │                      │                  │◄─ Answer ────────│
   │                      │◄─ Answer ────────│  (Callback)      │
   │◄─ Answer (IPC) ──────│                  │                  │
   │                      │                  │                  │
   │◄─ ICE Candidates ───◄│◄─ ICE ───────────◄│── ICE ─────────►│
   │   (IPC)              │   Candidates     │   Candidates     │
   │                      │                  │                  │
   │══════════════════════ WebRTC P2P (DataChannel) ═══════════════════════│
   │◄════════════════ JPEG 帧 ("desktop" channel) ════════════════════════│
   │═════════════════ 输入事件 ("input" channel) ═════════════════════════►│
   │═════════════════ 剪贴板 ("clipboard" channel, 双向) ═════════════════│
```

## 项目结构

```
rtcdesk/
├── Cargo.toml                  # Workspace 配置
├── client/                     # Viewer GUI 应用
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs             # 应用入口 + 窗口管理 + 自动启动host
│       ├── viewer.rs           # WebRTC 连接管理 + 帧接收
│       ├── viewer_ipc.rs       # IPC 信令代理 (替代直接WebSocket)
│       ├── ipc_client.rs       # IPC 客户端
│       ├── ipc_machine.rs      # IPC 机器状态管理
│       ├── quality.rs          # 画质预设 + 缩放模式定义
│       ├── theme.rs            # 暗色/亮色主题定义
│       ├── config.rs           # 配置持久化 + 连接历史
│       ├── clipboard.rs        # 剪贴板同步逻辑
│       ├── screenshot.rs       # 远程截图功能
│       ├── toast.rs            # Toast 通知组件
│       ├── host_process.rs     # Host 进程管理 + systemd 服务
│       ├── components/         # UI 组件
│       │   ├── auth.rs         # 登录/注册表单 + 自动登录
│       │   ├── connect.rs      # 连接设置页
│       │   ├── home.rs         # 主页 (机器列表)
│       │   ├── host.rs         # Host 模式页 + 服务管理
│       │   ├── settings.rs     # 设置页
│       │   ├── sidebar.rs      # 侧边栏导航
│       │   ├── titlebar.rs     # 自定义标题栏 (含主题切换)
│       │   └── resize.rs       # 窗口边缘拖拽缩放
│       ├── remote/             # 远程会话模块
│       │   ├── mod.rs          # RemoteView 状态管理
│       │   ├── view.rs         # 远程视图渲染 + 全屏
│       │   ├── toolbar.rs      # 浮动工具栏
│       │   ├── mouse.rs        # 鼠标坐标映射 + 事件处理
│       │   ├── keyboard.rs     # 键盘事件捕获
│       │   ├── file_transfer.rs # 文件传输 UI
│       │   └── viewer_loop.rs  # Viewer 主循环 (帧接收/分发)
│       └── assets/
│           └── app.css         # 全局样式 (暗色/亮色双主题)
├── host-service/               # Host 端服务
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs             # 入口：配置加载、注册机器、主循环
│       ├── signal_client.rs    # 信令 WS 客户端 (登录、心跳、消息收发)
│       ├── ipc_server.rs       # IPC 服务器 (接收client命令并转发)
│       ├── peer.rs             # WebRTC PeerConnection 管理
│       └── capture.rs          # 屏幕捕获与编码
├── signaling-server/           # 信令服务器
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs             # 入口：axum HTTP/WS 服务
│       ├── ws_handler.rs       # WebSocket 信令处理
│       ├── auth.rs             # JWT 签发/验证 + 用户注册/登录
│       ├── db.rs               # SQLite 数据访问层
│       └── machine.rs          # 机器注册、心跳、在线状态
├── shared/                     # 共享协议与类型
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── frame.rs            # 帧分片协议 + FrameAssembler
│       ├── signal.rs           # 信令消息枚举定义
│       ├── ipc.rs              # IPC 消息协议 (ClientCommand/HostMessage)
│       ├── ice0_service.rs     # ice0 RPC 服务定义 (HostService, HostCallback, ClientCallback)
│       ├── auth.rs             # JWT Claims 类型
│       └── local_id.rs         # 本地设备 ID 生成
├── tools/                      # 辅助工具
│   ├── Cargo.toml
│   └── src/
│       └── stuncheck.rs        # STUN 服务器连通性检查
└── stunlist.txt                # 公共 STUN 服务器列表
```

## 功能特性

### Viewer (client)

- **自动启动 Host** — Client 启动时自动检测并启动 host-service
- **自动登录** — 从配置文件读取凭据，通过 host 代理登录
- **机器列表** — 实时显示在线机器列表，状态自动更新
- **自定义标题栏** — 无边框窗口，包含窗口控制按钮和主题切换
- **侧边栏导航** — 连接、Host 模式、设置
- **连接管理** — 填写服务器地址、远程设备 ID、画质预设；连接历史记录
- **三种缩放模式**
  - `Fit` — 自适应（保持宽高比，完整显示）
  - `Stretch` — 拉伸填充（填满整个视口）
  - `Original` — 原始大小（1:1 像素，超出出现滚动条）
- **浮动工具栏** — 自动隐藏，支持：
  - 画质切换（4 档：最佳画质 / 均衡 / 流畅优先 / 低带宽）
  - 缩放模式切换
  - 只看模式（仅观看，不发送输入）
  - 截图
  - 文件发送
  - 剪贴板同步
  - 全屏 / 最小化 / 断开连接
- **暗色 / 亮色主题** — 完整的双主题支持，工具栏和所有 UI 适配
- **窗口边缘拖拽缩放** — 自定义无边框窗口的拉伸支持
- **文件传输** — 基于 DataChannel 的文件收发
- **远程截图** — 保存远程桌面截图到本地

### Host (host-service)

- **系统服务** — 可安装为 systemd/Windows Service，开机自启
- **屏幕捕获与编码** — 支持 VA-API 硬件加速
- **WebRTC 信令代理** — 代理所有 Client 与 Signal 的信令交互
- **帧分片推送** — "desktop" channel 传输 JPEG 帧
- **输入事件接收与模拟** — `enigo` 库模拟键鼠操作
- **剪贴板双向同步** — 文本和文件传输
- **动态画质调整** — 响应 Viewer 切换画质
- **机器注册与心跳** — 启动时自动注册，定期心跳保持在线
- **统一配置** — 从 `rtcdesk.json` 读取所有配置

### Signaling Server

- **JWT 用户认证** — 注册/登录/token 刷新
- **信令消息转发** — Offer/Answer/ICE 转发
- **机器管理** — 机器注册、在线状态、心跳超时检测
- **SQLite 存储** — 用户与机器数据持久化
- **房间事件广播** — PeerJoined/PeerLeft 通知

## IPC 通信协议

### 概述

Client 与 Host-Service 通过 **ice0 RPC** over **interprocess** crate (v2.4) 进行本地 IPC 通信：

- **Linux/macOS**: Unix domain sockets
- **Windows**: Named pipes
- **Socket 名称**: `rtcdesk-host-ice0`
- **RPC 框架**: ice0 v0.0.1 (双向 RPC，支持服务端回调)
- **序列化**: bincode (二进制，高性能)

### ClientCommand (Client → Host)

```rust
enum ClientCommand {
    Login { username, password },
    Logout,
    Register { username, password, email },
    PeerBind { node_id, name, os },
    PeerUnbind { node_id },
    GetPeerList,
    ViewerConnect { viewer_id, node_id, password },
    ViewerAnswer { viewer_id, node_id, sdp },
    ViewerIceCandidate { viewer_id, node_id, candidate, ... },
    ViewerQualityUpdate { viewer_id, quality, ... },
    GetStatus,
}
```

### HostMessage (Host → Client)

```rust
enum HostMessage {
    ConnectionStatus { status, message },
    PeerStatus { node_id, name, os, online },
    PeerList { machines: Vec<PeerInfo> },
    CommandResult { success, message },
    AuthStatus { logged_in, username },
    LoginResult { success, token, reason },
    RegisterResult { success, message },
    ViewerOffer { from, to, sdp },
    ViewerIceCandidate { from, to, candidate, ... },
    ViewerAnswer { viewer_id, accepted, reason },
    ViewerPeerLeft { id },
    ViewerError { message },
}
```

## 协议格式

### 帧分片

```
┌────────────┬────────────┬───────────────┬──────────────────┐
│ frame_id   │ chunk_ix   │ total_chunks  │ chunk_data       │
│ u32 BE     │ u32 BE     │ u32 BE        │ ≤ 15 KiB         │
└────────────┴────────────┴───────────────┴──────────────────┘
```

### 帧头 (16 字节)

```
┌─────────┬─────────┬──────────┬──────────┬──────────────────┐
│ orig_w  │ orig_h  │ frame_w  │ frame_h  │   JPEG data      │
│ u32 BE  │ u32 BE  │ u32 BE   │ u32 BE   │                  │
└─────────┴─────────┴──────────┴──────────┴──────────────────┘
```

- `orig_w / orig_h` — 远程显示器实际分辨率（鼠标坐标映射）
- `frame_w / frame_h` — JPEG 尺寸（可能缩放以节省带宽）

### 输入事件

| 类型 | 格式 |
|------|------|
| 鼠标 | `0x01` + x(f64 LE) + y(f64 LE) + btn(u8) + pressed(u8) |
| 键盘 | `0x02` + key_code(u32 LE) + pressed(u8) |
| 滚轮 | `0x03` + dx(i32 LE) + dy(i32 LE) |

### 剪贴板

| 标签 | 含义 |
|------|------|
| `0x01` | 文本数据 |
| `0x03` | 文件列表头 |
| `0x04` | 文件数据块 (64 KiB) |
| `0x05` | 传输完成 |

### 信令消息 (JSON, serde tag="type")

- `Login` / `LoginResult` — JWT 认证
- `Register` / `Registered` — Peer 注册
- `PeerJoin` / `PeerStatus` — 机器注册与在线状态
- `PeerList` / `GetPeerList` — 机器列表
- `Heartbeat` — 心跳（30s 间隔）
- `TokenRefresh` / `TokenRefreshResult` — Token 刷新
- `Offer` / `Answer` / `IceCandidate` — WebRTC SDP 协商
- `RequestConnect` / `ConnectRequest` / `Answer` — 连接请求与认证
- `QualityUpdate` — 动态画质调整
- `PeerJoined` / `PeerLeft` — 房间事件
- `Error` — 错误消息

## 坐标映射管线

Viewer 端鼠标坐标需经过两步转换：

```
element pixel  ──[ScaleMode]──▶  frame pixel  ──[orig/frame 比例]──▶  远程屏幕坐标
```

**举例：** 远程 1920×1080，"均衡"画质发送 1280×720，点击中心：

- Frame 坐标: (640, 360)
- 远程坐标: (640×1.5, 360×1.5) = **(960, 540)**

三种 ScaleMode 均正确映射。Original 模式下 viewport div 尺寸 = 帧像素，1:1 精确对应。

## 构建运行

### 依赖

- Rust 1.85+ (edition 2024)
- Linux: X11 开发库 (`libxcb`, `libxrandr` 等)
- 可选: `xclip`, `xdotool`（Host 端剪贴板粘贴）

### 构建

```bash
# 构建所有 crate
cargo build --release

# 单独构建
cargo build --release -p rtcdesk-client
cargo build --release -p rtcdesk-host
cargo build --release -p signaling-server
```

### 配置文件

创建配置文件 `~/.config/rtcdesk/rtcdesk.json`:

```json
{
  "server_addr": "ws://your-signal-server:9009",
  "login_username": "your-username",
  "login_password": "your-password",
  "host_password": "optional-host-password",
  "stun_servers": [
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302"
  ],
  "theme": "dark",
  "locale": "zh-CN"
}
```

### 运行信令服务器

```bash
cargo run --release -p signaling-server

# 自定义地址
RTCDESK_ADDR=0.0.0.0:9009 cargo run --release -p signaling-server
```

### 运行 Client (自动启动 Host)

```bash
cargo run --release -p rtcdesk-client

# Client 会自动:
# 1. 读取配置文件
# 2. 启动 host-service (如未运行)
# 3. 建立 IPC 连接
# 4. 自动登录 (如配置了凭据)
# 5. 请求机器列表
```

### 安装 Host 为系统服务 (Linux)

```bash
# 通过 Client UI 安装
# 或手动安装:
cp host/linux/rtcdesk-host.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now rtcdesk-host
```

## 技术栈

| 组件 | 技术 |
|------|------|
| 语言 | Rust 2024 edition |
| **RPC 框架** | **ice0 v0.0.1** (双向 RPC，支持回调) |
| WebRTC | webrtc-rs 0.20.0-alpha |
| 信令 | ice0 RPC Server (TCP) |
| GUI | Dioxus 0.7 Desktop (系统 WebView) |
| IPC | ice0 over interprocess 2.4 (tokio) |
| 屏幕捕获 | xcap 0.9 + image 0.25 |
| 输入模拟 | enigo 0.6 |
| 剪贴板 | clipboard-rs 0.2 |
| 加密 | rustls (aws-lc-rs) |
| 序列化 | serde + serde_json + bincode |
| 数据库 | rusqlite 0.31 (SQLite) |
| 认证 | jsonwebtoken 9 + argon2 0.5 |
| 运行时 | tokio |

## ice0 RPC 框架

### 概述

RTCDesk 使用 **ice0 v0.0.1** 作为统一的 RPC 框架，替代了原有的 tarpc 和 jsonrpsee。ice0 是一个受 ZeroC Ice 启发的现代 Rust RPC 框架，具有以下特性：

- **双向 RPC**: 服务端可以回调客户端，客户端可以接收服务端通知
- **连接复用**: 单个 TCP 连接支持多路并发流（最多 10,000 个）
- **多协议支持**: TCP、UDP、SSL、WebSocket、HTTP/2、HTTP/3、IPC
- **高性能序列化**: bincode 二进制序列化
- **类型安全**: 使用 `#[ice_service]` 宏生成类型安全的客户端代理和服务端骨架

### 服务定义

在 `shared/src/ice0_service.rs` 中定义了三个核心服务：

```rust
// 1. HostService - 客户端调用主机服务
#[ice_service]
pub trait HostService {
    async fn user_login(username: String, password: String) -> LoginResult;
    async fn user_logout();
    async fn machine_list() -> Vec<MachineInfo>;
    async fn viewer_connect_with_offer(...) -> Answer;
    // ... 其他方法
}

// 2. HostCallback - 信号服务器回调主机服务
#[ice_service]
pub trait HostCallback {
    async fn on_peer_status_change(change: PeerStatusChange);
    async fn request_control_from_viewer(request: ViewerConnectionRequest) -> Answer;
}

// 3. ClientCallback - 主机服务回调客户端
#[ice_service]
pub trait ClientCallback {
    async fn on_connection_status_change(status: ConnectionStatus);
    async fn on_peer_status_change(change: PeerStatusChange);
}
```

### 双向通信模式

**Signal Server → Host-Service 回调**:
1. Host-Service 连接到 Signal Server 时，创建 callback adapter
2. 注册 `HostCallbackServant` 到 callback adapter
3. 通过 `connection.set_adapter()` 绑定回调到连接
4. Signal Server 通过 `HostCallbackPrx` 调用 Host-Service 的方法

**Host-Service → Client 回调**:
1. Client 通过 IPC 连接到 Host-Service
2. Host-Service 通过 `ClientCallbackPrx` 调用 Client 的方法
3. Client 实时接收连接状态、机器状态等通知

### ice0 API 使用

**创建服务端** (Signal Server):
```rust
let communicator = Communicator::default().await?;
let adapter = communicator.create_object_adapter(
    "HostServiceAdapter",
    Some("tcp -h 127.0.0.1 -p 9009".to_string()),
    None,
    None,
).await?;

let servant = Arc::new(HostServiceServant::new(rpc_impl));
let identity = Identity::new("HostService");
adapter.add_servant(identity, servant).await;
adapter.activate().await;
```

**创建客户端代理** (Host-Service Signal Client):
```rust
let communicator = Communicator::default().await?;
let proxy = communicator.string_to_proxy("HostService:tcp -h 127.0.0.1 -p 9009").await?;
let rpc_client = HostServicePrx::checked_cast(proxy)?;

// 调用远程方法（无需 Context 参数）
let result = rpc_client.user_login(username, password).await?;
```

**设置回调** (双向通信):
```rust
// 创建回调 adapter
let callback_adapter = communicator.create_object_adapter(
    "HostCallbackAdapter", None, None, None
).await?;

// 注册回调 servant
let servant = Arc::new(HostCallbackServant::new(callback_impl));
callback_adapter.add_servant(Identity::new("HostCallback"), servant.clone()).await;
callback_adapter.activate().await;

// 绑定到连接
let connection = rpc_client.to_proxy().ice_getConnection()?;
connection.set_adapter(callback_adapter.clone());
connection.add_servant(Identity::new("HostCallback"), servant).await;
```

rustup target add x86_64-pc-windows-gnu

# Install mingw-w64 FFmpeg
sudo apt-get install gcc-mingw-w64-x86-64 libavcodec-dev:amd64 libavformat-dev:amd64 libavutil-dev:amd64

# Set pkg-config for cross-compilation
export PKG_CONFIG_SYSROOT_DIR=/usr/x86_64-w64-mingw32
export PKG_CONFIG_PATH=/usr/x86_64-w64-mingw32/lib/pkgconfig
export PKG_CONFIG=x86_64-w64-mingw32-pkg-config
cargo build --release --target x86_64-pc-windows-gnu

# Build host-service for Windows (no FFmpeg needed)
cargo build --release --package rtcdesk-host --target x86_64-pc-windows-gnu
## License

MIT
