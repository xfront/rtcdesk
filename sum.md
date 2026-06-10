视频管道重新实现 — 完成
decoder.rs (从 1369 行重写至约 646 行):
DecodedFrame 现在包含 bgra_data 而非 jpeg_data
移除所有 Vulkan Video 解码代码
移除所有 DMA-BUF/EGL/Skia 代码
VAAPI 简化：解码 → av_hwframe_transfer_data → sws_scale(YUV→BGRA)
软件解码器：YUV → RGB → BGRA 转换
移除 rgb_to_jpeg()，移除 image crate 导入
viewer.rs — 解码线程已修复：
有界通道 sync_channel(3) 替代无限通道
解码所有帧按顺序进行（保留 H264 参考链）
将 BGRA 直接写入 FrameSlot（无中间通道）
使用 try_send() 在 on_track 中防止阻塞异步运行时
通道满时请求关键帧以实现干净重启
viewer_loop.rs + view.rs — UI 更新：
frame_b64: Signal<String> → frame_seq: Signal<u64>
<img src="data:image/jpeg;base64,..."> → <img src="rtcdesk://frame?seq=N">
完全移除 Base64 编码
ViewerCallback::on_video_frame 现在只传递维度（像素数据在 FrameSlot 中）
screenshot.rs — 从 FrameSlot 的 BMP 数据直接保存（无 base64 解码）
Cargo.toml — 已简化：
默认特性：["desktop", "vaapi"]
移除：vaapi-dmabuf、skia-render、vulkan-video 特性
移除依赖项：gpu-video、wgpu、winit、glow、libloading、skia-safe
已删除 gpu_ctx.rs (1212 行) 和 skia_renderer.rs (335 行)
构建：使用 cargo build -p rtcdesk-client --features vaapi 成功编译 — 仅存在警告（未使用的变量），无错误