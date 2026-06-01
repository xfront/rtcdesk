# RTCDesk Client UI — Phase 2 改进概览

## 完成内容

### 1. 本机 ID 生成与持久化
- **生成策略**（分层回退）：MAC 地址 hash → hostname hash → 随机
- Linux 下读取 `/sys/class/net/<iface>/address`，排除 `lo` 和零 MAC
- 输出 9 位纯数字 ID（如 `378 529 164`），显示时自动分组
- 首次加载时如果没有 `local_id` 自动生成并持久化
- `HomeTab` 主卡片显示 ID + 一键复制按钮
- `SettingsTab > 设备标识` 显示完整 ID + 重置/复制按钮
- 连接时使用本机 ID 作为 Viewer 的信令注册 ID

### 2. 亮色/暗色主题切换
- 定义 `Theme::Dark` / `Theme::Light` 枚举 + 两套完整 `ThemeColors`
- 标题栏新增 `ThemeToggleBtn`（太阳/月亮图标）
- 设置页新增"外观主题"区域，可视切换
- 所有组件通过 `tc(theme)` 获取当前色彩
- 主题持久化到 `AppConfig.theme`

### 3. STUN 服务器可配置
- `AppConfig.stun_servers: Vec<String>` 持久化多 STUN 服务器
- 设置页新增"STUN 服务器"输入框（逗号分隔），失焦自动保存
- `load_stun_servers()` 优先级：配置文件 > env `RTCDESK_STUN` > 默认 Google STUN

### 4. 配置持久化增强
`AppConfig` 新增字段：
- `local_id: String` — 本机设备 ID
- `stun_servers: Vec<String>` — 用户配置的 STUN 服务器
- `theme: Theme` — UI 主题（Dark/Light）

`load_config()` 自动初始化空 `local_id`。

## 文件变更
- `client/main.rs` — 完整重写（~1750 行 → ~1950 行）

## 关键设计决策
- 主题色彩双轨制：Rust 端 `ThemeColors` + CSS 端 `var(--xxx)`，通过 `data-theme` 属性联动
- 远程视图工具栏永远暗色（功能区域不受主题影响）
- 本机 ID 基于 MAC 地址确保多设备不冲突
- 密码字段留待信令层实际认证协议就绪后接入
- CSS 样式集中管理：`app.css` 包含所有主题变量、全局重置、组件 class、布局工具类
