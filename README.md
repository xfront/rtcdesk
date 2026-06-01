# RTCDesk — 基于 WebRTC 的跨平台远程桌面

RTCDesk 是一款使用 Rust 编写的开源远程桌面工具，基于 WebRTC 实现 P2P 加密传输，无需中继服务器转发画面数据。支持屏幕共享、远程控制、剪贴板同步和文件传输。

## 整体架构

```
┌──────────────────────────────────────────────────────────────────────────┐
│                          RTCDesk 架构 (Workspace)                        │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌───────────────────┐          WebSocket            ┌────────────────┐  │
│  │   host-service    │◄──────────────────────────────►│ signaling-     │  │
│  │   (Host 端)       │   JSON 信令 + JWT 认证        │ server         │  │
│  │                   │                               │ (axum + WS)    │  │
│  │  • 屏幕捕获/编码   │                               │                │  │
│  │  • WebRTC Peer    │                               │  • 信令转发     │  │
│  │  • 输入模拟       │                               │  • JWT 认证     │  │
│  │  • 剪贴板同步     │          WebSocket            │  • SQLite 存储  │  │
│  └────────┬──────────┘◄──────────────────────────────┤  • 心跳/在线    │  │
│           │                                        ──┤                │  │
│           │  WebRTC DataChannel (P2P)               │  └────────────────┘  │
│           │  "desktop"   — JPEG 帧 (分片传输)        │                      │
│           │  "input"     — 鼠标 / 键盘 / 滚轮        │                      │
│           │  "clipboard" — 文本 / 文件同步           │                      │
│           ▼                                         │                      │
│  ┌───────────────────┐                              │                      │
│  │   client (Viewer) │  Dioxus Desktop GUI           │                      │
│  │                   │  • 自定义标题栏 + 侧边栏       │                      │
│  │  • 连接管理       │  • 远程视图 (三种缩放模式)     │                      │
│  │  • 远程视图       │  • 浮动工具栏                 │                      │
│  │  • 输入转发       │  • 暗色 / 亮色主题            │                      │
│  │  • 文件传输       │  • 窗口边缘拖拽缩放            │                      │
│  │  • 截图           │  • 连接历史                   │                      │
│  └───────────────────┘                              │                      │
│                                                                          │
│  ┌───────────────────┐                                                    │
│  │   shared          │  共享模块：帧协议、信令消息、JWT 类型、本地 ID      │
│  └───────────────────┘                                                    │
└──────────────────────────────────────────────────────────────────────────┘
```

## 连接流程

1. **启动 signaling-server** — 独立信令服务，管理用户认证和消息转发
2. **Host 登录注册** — host-service 启动后连接信令服务器，JWT 认证，注册机器信息
3. **Viewer 登录** — client 连接信令服务器，JWT 认证，获取在线机器列表
4. **发起连接** — Viewer 选择目标机器发起 `RequestConnect`
5. **SDP 协商** — Host 创建 Offer（含 DataChannel），Viewer 创建 Answer（注入画质参数）
6. **ICE 穿透** — 双方通过信令服务器交换 ICE Candidates，建立 P2P 连接
7. **流传输** — WebRTC P2P 连接建立后，桌面流通过 DataChannel 传输，不再经过信令服务器

```
 Viewer (client)               signaling-server               Host (host-service)
   │                              │                              │
   │── Login ────────────────────►│◄── Login ────────────────────│
   │◄── JWT Token ───────────────│── JWT Token ────────────────►│
   │── Register(Viewer) ─────────►│◄── Register(Host) ───────────│
   │                              │◄── Heartbeat (每30s) ────────│
   │── MachineList ──────────────►│── MachineList ──────────────►│
   │── RequestConnect ───────────►│── RequestConnect ───────────►│
   │                              │                              │
   │◄────────────── Offer ◄───────│◄─────── Offer ───────────────│
   │── Answer ───────────────────►│── Answer ──────────────────► │
   │                              │                              │
   │◄══ ICE Candidates (双向) ═══►│◄══ ICE Candidates (双向) ═══►│
   │                              │                              │
   │═══════ WebRTC P2P (DataChannel) ════════════════════════════│
   │◄══════════ JPEG 帧 ("desktop" channel) ═════════════════════│
   │═══════════ 输入事件 ("input" channel) ══════════════════════►│
   │═══════════ 剪贴板 ("clipboard" channel, 双向) ══════════════│
```

## 项目结构

```
rtcdesk/
├── Cargo.toml                  # Workspace 配置
├── client/                     # Viewer GUI 应用
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs             # 应用入口 + 窗口管理
│       ├── viewer.rs           # WebRTC 连接管理 + 帧接收
│       ├── quality.rs          # 画质预设 + 缩放模式定义
│       ├── theme.rs            # 暗色/亮色主题定义
│       ├── config.rs           # 配置持久化 + 连接历史
│       ├── clipboard.rs        # 剪贴板同步逻辑
│       ├── screenshot.rs       # 远程截图功能
│       ├── toast.rs            # Toast 通知组件
│       ├── host_process.rs     # Host 进程管理
│       ├── components/         # UI 组件
│       │   ├── auth.rs         # 登录/注册表单
│       │   ├── connect.rs      # 连接设置页
│       │   ├── home.rs         # 主页 (机器列表)
│       │   ├── host.rs         # Host 模式页
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
│       ├── main.rs             # 入口：登录、注册机器、主循环
│       ├── signal_client.rs    # 信令 WS 客户端 (登录、心跳、消息收发)
│       ├── peer.rs             # WebRTC PeerConnection 管理
│       └── capture.rs          # 屏幕捕获与 JPEG 编码
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

- 屏幕捕获与 JPEG 编码（`xcap`）
- WebRTC PeerConnection 管理（Offer/Answer 协商）
- 帧分片推送（"desktop" channel）
- 输入事件接收与模拟（`enigo`）
- 剪贴板双向同步
- 动态画质调整（响应 Viewer 切换）
- JWT 认证 + 心跳保持

### Signaling Server

- JWT 用户认证（注册/登录/token 刷新）
- 信令消息转发（Offer/Answer/ICE）
- SQLite 用户与机器数据存储
- 机器在线状态管理（心跳超时 90s）
- 房间事件广播

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
- `MachineRegister` / `MachineStatus` — 机器注册与在线状态
- `Heartbeat` — 心跳（30s 间隔）
- `TokenRefresh` / `TokenRefreshResult` — Token 刷新
- `Offer` / `Answer` / `IceCandidate` — WebRTC SDP 协商
- `RequestConnect` / `ConnectRequest` / `AuthResult` — 连接请求与认证
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
cargo build --release -p host-service
cargo build --release -p signaling-server
```

### 运行信令服务器

```bash
cargo run --release -p signaling-server

# 自定义地址
RTCDESK_ADDR=0.0.0.0:9009 cargo run --release -p signaling-server
```

### 运行 Host

```bash
cargo run --release -p host-service -- --server ws://localhost:9009/ws --username admin --password admin
```

### 运行 Viewer

```bash
cargo run --release -p rtcdesk-client
```

## 技术栈

| 组件 | 技术 |
|------|------|
| 语言 | Rust 2024 edition |
| WebRTC | webrtc-rs 0.20.0-alpha |
| 信令 | axum 0.8 + tokio-tungstenite |
| GUI | Dioxus 0.7 Desktop (系统 WebView) |
| 屏幕捕获 | xcap 0.9 + image 0.25 |
| 输入模拟 | enigo 0.6 |
| 剪贴板 | clipboard-rs 0.2 |
| 加密 | rustls (aws-lc-rs) |
| 序列化 | serde + serde_json |
| 数据库 | rusqlite 0.31 (SQLite) |
| 认证 | jsonwebtoken 9 + argon2 0.5 |
| 运行时 | tokio |

## License

MIT
