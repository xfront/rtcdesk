# RTCDesk — 基于 WebRTC 的跨平台远程桌面

RTCDesk 是一款使用 Rust 编写的开源远程桌面工具，基于 WebRTC 实现 P2P 加密传输，无需中继服务器转发画面数据。支持屏幕共享、远程控制、剪贴板同步和文件传输。

## 整体架构

```
┌──────────────────────────────────────────────────────────────────┐
│                        RTCDesk 架构                              │
├──────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌─────────────┐   信令 (WebSocket)    ┌──────────────────┐     │
│  │  Host 端    │◄─────────────────────►│  信令服务器       │     │
│  │ (main.rs)   │   JSON 消息转发       │  (server.rs)     │     │
│  │             │                      │  axum + WS       │     │
│  │ • 屏幕捕获   │                      └────────┬─────────┘     │
│  │ • WebRTC    │   信令 (WebSocket)             │               │
│  │ • 输入模拟   │◄───────────────────────────────┘               │
│  │ • 剪贴板    │                                                │
│  └──────┬──────┘                                                │
│         │ WebRTC DataChannel                                     │
│         │  "desktop"   — JPEG 帧 (分片传输)                       │
│         │  "input"     — 鼠标 / 键盘 / 滚轮事件                   │
│         │  "clipboard" — 文本 / 文件同步                          │
│         ▼                                                        │
│  ┌─────────────┐                                                 │
│  │  Viewer 端  │  Dioxus Desktop GUI                             │
│  │ (client.rs) │  • 连接表单 + 远程视图                           │
│  │ (viewer.rs) │  • 浮动工具栏 (画质 / 缩放 / 剪贴板 / 全屏)      │
│  │             │  • 帧重组 & Base64 渲染                          │
│  │             │  • 输入事件转发                                   │
│  │             │  • 剪贴板自动双向同步                              │
│  └─────────────┘                                                 │
│                                                                  │
│  共享模块:                                                        │
│  • frame.rs  — 帧分片协议、常量、FrameAssembler                   │
│  • signal.rs — 信令消息类型、SDP 参数编解码                         │
│  • capture.rs — 屏幕截图 + JPEG 编码 (Host 专用)                   │
│  • peer.rs   — WebRTC PeerConnection 管理 (Host 专用)             │
└──────────────────────────────────────────────────────────────────┘
```

## 连接流程

1. **注册** — Host 和 Viewer 分别连接信令服务器，注册自己的 Peer ID
2. **发起连接** — Viewer 发送 `RequestConnect`，服务器转发给 Host
3. **SDP 协商** — Host 创建 Offer（含 DataChannel），Viewer 创建 Answer（注入画质参数到 SDP）
4. **ICE 穿透** — 双方通过信令服务器交换 ICE Candidates，尝试建立 P2P 连接
5. **流传输** — WebRTC P2P 连接建立后，桌面流通过 DataChannel 传输，不再经过信令服务器

```
 Viewer                        信令服务器                        Host
   │                              │                              │
   │── Register ─────────────────►│◄── Register ─────────────────│
   │                              │                              │
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

## 模块说明

### `main.rs` — Host 入口

- 启动内置信令服务器 (`axum` + WebSocket)
- 以 Host 身份连接信令服务器
- 创建 `RtcPeer` 响应 Viewer 的连接请求
- 管理 WebRTC SDP 协商和 ICE 候选交换
- 启动屏幕捕获和帧推送

### `client.rs` — Viewer GUI

Dioxus Desktop 应用，提供全功能的远程桌面查看器：

- **连接表单** — 填写服务器地址、远程设备 ID、选择画质预设
- **远程视图** — Base64 JPEG 渲染 + 鼠标/键盘事件捕捉 + 坐标转换
- **浮动工具栏** — Remmina 风格，支持画质切换、缩放模式、剪贴板同步、全屏、断开连接
- **ScaleMode** — `Fit`（自适应）、`Stretch`（拉伸填充）、`Original`（原始大小 + 滚动条）
- **画质预设** — 4 档（最佳画质 / 均衡 / 流畅优先 / 低带宽），每档配置分辨率、JPEG 质量、FPS、码率
- **剪贴板自动同步** — 200ms 轮询本地剪贴板，hash 去重防回环

### `viewer.rs` — Viewer 网络层

`RtcViewer` 封装 Viewer 端的 WebRTC 连接：

- WebSocket 信令客户端（注册、连接请求、ICE 转发）
- WebRTC PeerConnection 管理
- 帧重组（`FrameAssembler`）
- 输入事件编码发送
- 剪贴板数据收发

### `peer.rs` — Host 网络层

`RtcPeer` 封装 Host 端的 WebRTC 连接：

- PeerConnection 管理 + SDP 协商
- 帧捕获循环 → 分片 → "desktop" DataChannel 推送
- 接收 Viewer 输入 → `enigo` 模拟鼠标/键盘/滚轮
- 剪贴板双向同步（文本 + 文件传输）
- 动态画质调整（运行时响应 Viewer 的画质切换）

### `server.rs` — 信令服务器

基于 `axum` + WebSocket 的轻量信令服务器：

- Peer 注册表 (HashMap)
- 消息转发（Offer/Answer/ICE/QualityUpdate）
- 房间事件广播（PeerJoined/PeerLeft）
- 内嵌于 Host 进程中运行

### `capture.rs` — 屏幕捕获

- 使用 `xcap` 库进行桌面截图
- RGBA → RGB → JPEG 编码（可调质量 0-100）
- 按需缩放（Lanczos3 滤波，保持宽高比）
- 返回 `CapturedFrame`：原始分辨率 + 缩放后分辨率 + JPEG 数据

### `frame.rs` — 协议定义

**帧分片格式：**

```
┌────────────┬────────────┬───────────────┬──────────────────┐
│ frame_id   │ chunk_ix   │ total_chunks  │ chunk_data       │
│ u32 BE     │ u32 BE     │ u32 BE        │ ≤ 15 KiB         │
└────────────┴────────────┴───────────────┴──────────────────┘
```

**帧头格式（16 字节）：**

```
┌─────────┬─────────┬──────────┬──────────┬──────────────────┐
│ orig_w  │ orig_h  │ frame_w  │ frame_h  │   JPEG data      │
│ u32 BE  │ u32 BE  │ u32 BE   │ u32 BE   │                  │
└─────────┴─────────┴──────────┴──────────┴──────────────────┘
```

- `orig_w` / `orig_h` — 远程显示器实际分辨率（用于鼠标坐标映射）
- `frame_w` / `frame_h` — 缩放后的 JPEG 尺寸（可能小于 orig，节省带宽）

**输入事件格式：**

| 类型 | 格式 |
|------|------|
| 鼠标 | `0x01` + x(f64 LE) + y(f64 LE) + btn(u8) + pressed(u8) |
| 键盘 | `0x02` + key_code(u32 LE) + pressed(u8) |
| 滚轮 | `0x03` + dx(i32 LE) + dy(i32 LE) |

**剪贴板格式：**

| 标签 | 含义 |
|------|------|
| `0x01` | 文本数据 |
| `0x03` | 文件列表头 |
| `0x04` | 文件数据块 (64 KiB) |
| `0x05` | 传输完成 |

### `signal.rs` — 信令协议

JSON 格式的信令消息（`serde` tag="type"）：

- `Register` / `Registered` / `PeerJoined` / `PeerLeft` — 会话管理
- `Offer` / `Answer` / `IceCandidate` — WebRTC SDP 协商
- `RequestConnect` / `ConnectRequest` — 连接请求
- `QualityUpdate` — 动态画质调整（quality / max_dim / fps）
- `Input` — 输入事件转发（备用路径，通常走 DataChannel）
- `Error` — 错误消息

SDP 参数编码：Viewer 的画质参数通过 `a=rtcdesk-*` SDP 属性行注入 Answer，Host 端从 SDP 中提取。

## 坐标映射管线

Viewer 端鼠标坐标需经过两步转换后才能发送给 Host：

```
 element pixel  ──[ScaleMode]──▶  frame pixel  ──[orig/frame 比例]──▶  远程屏幕坐标
```

**举例：** 远程 1920×1080 屏幕，"均衡"画质发送 1280×720 帧，点击窗口中心：

- Frame 坐标: (640, 360)
- 远程坐标: (640×1.5, 360×1.5) = **(960, 540)** ✓

三种 ScaleMode 均经过此管线正确映射。

## 构建运行

### 依赖

- Rust 1.85+
- Linux: X11 开发库 (`libxcb`, `libxrandr` 等)
- 可选: `xclip`, `xdotool`（Host 端剪贴板粘贴支持）

### 构建

```bash
cargo build --release
```

### 运行 Host

```bash
# 默认 STUN 服务器
cargo run --release

# 自定义 STUN
RTCDESK_STUN=stun:your-server.com:3478 cargo run --release
```

### 运行 Viewer

```bash
cargo run --release --bin client
```

## 技术栈

| 组件 | 技术 |
|------|------|
| 语言 | Rust 2024 |
| WebRTC | webrtc-rs 0.20.0-alpha |
| 信令 | axum + tokio-tungstenite |
| GUI | Dioxus Desktop (系统 WebView) |
| 屏幕捕获 | xcap + image (JPEG 编码) |
| 输入模拟 | enigo |
| 剪贴板 | arboard |
| 加密 | rustls (aws-lc-rs) |
| 序列化 | serde + serde_json |
| 运行时 | tokio |

## 项目结构

```
RtcDesk/
├── Cargo.toml          # 项目配置，三个二进制目标
├── src/
│   ├── main.rs         # Host 入口
│   ├── client.rs       # Viewer GUI (Dioxus Desktop)
│   ├── viewer.rs       # Viewer WebRTC 连接管理
│   ├── peer.rs         # Host WebRTC 连接管理
│   ├── server.rs       # WebSocket 信令服务器
│   ├── capture.rs      # 屏幕捕获与 JPEG 编码
│   ├── frame.rs        # 帧分片协议、常量、FrameAssembler
│   └── signal.rs       # 信令消息类型定义
├── assets/             # GUI 资源 (CSS, 图标)
├── stunlist.txt        # 公共 STUN 服务器列表
└── target/             # 编译输出
```

## License

MIT
