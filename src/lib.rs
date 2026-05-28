//! RTCDesk Viewer Client — Rust library with UniFFI exports.
//!
//! Architecture:
//! - A single global tokio runtime drives all async operations.
//! - Each `ViewerSession` communicates with its background task via channels.
//! - The background task manages a single `RtcViewer` lifecycle (connect → streaming → disconnect).
//! - `ViewerCallback` delivers frames and status back to Kotlin.

mod frame;
mod signal;
mod viewer;

use log::{error, info};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, Notify};

use viewer::RtcViewer;

// ─── Accumulated viewer params (set before Connect) ───────────────────

#[derive(Debug, Clone, Default)]
struct SessionParams {
    pic_quality: u32,
    width: u32,
    height: u32,
    fps: u32,
}

// ─── Global Runtime ────────────────────────────────────────────────────

fn runtime() -> &'static tokio::runtime::Runtime {
    use std::sync::OnceLock;
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        // rustls 0.23 requires explicit crypto provider initialization
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .expect("Failed to install rustls crypto provider");

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("rtcdesk-viewer")
            .build()
            .expect("Failed to create tokio runtime")
    })
}

// ─── Internal Commands ──────────────────────────────────────────────────

enum SessionCommand {
    Connect {
        server_addr: String,
        stun_servers: Vec<String>,
        params: SessionParams,
    },
    Disconnect,
    MouseEvent {
        x: f64,
        y: f64,
        button: u8,
        pressed: bool,
    },
    KeyEvent {
        key_code: u32,
        pressed: bool,
    },
    SendClipboardData {
        data: Vec<u8>,
    },
    SendFiles {
        file_list: Vec<u8>,
        chunks: Vec<u8>,
    },
}

// ─── UniFFI Callback Interface ──────────────────────────────────────────

#[uniffi::export(callback_interface)]
pub trait ViewerCallback: Send + Sync {
    fn on_frame(&self, data: Vec<u8>, width: u32, height: u32);
    fn on_connected(&self);
    fn on_disconnected(&self);
    fn on_error(&self, message: String);
    fn on_clipboard_text(&self, text: String);
    fn on_clipboard_file_uris(&self, uris: String);
}

// ─── Bridge ─────────────────────────────────────────────────────────────

struct CallbackBridge {
    inner: Arc<dyn ViewerCallback>,
}
impl viewer::ViewerCallback for CallbackBridge {
    fn on_connected(&self) {
        self.inner.on_connected();
    }
    fn on_disconnected(&self) {
        self.inner.on_disconnected();
    }
    fn on_frame(&self, data: Vec<u8>, width: u32, height: u32) {
        self.inner.on_frame(data, width, height);
    }
    fn on_connection_state(&self, _: String) {}
    fn on_error(&self, msg: String) {
        self.inner.on_error(msg);
    }
    fn on_clipboard_text(&self, text: String) {
        self.inner.on_clipboard_text(text);
    }
    fn on_clipboard_file_uris(&self, uris: String) {
        self.inner.on_clipboard_file_uris(uris);
    }
}

// ─── ViewerSession ──────────────────────────────────────────────────────

#[derive(uniffi::Object)]
pub struct ViewerSession {
    cmd_tx: mpsc::UnboundedSender<SessionCommand>,
    is_connected: Arc<AtomicBool>,
    shutdown: Arc<Notify>,
    params: Mutex<SessionParams>,
}

#[uniffi::export]
impl ViewerSession {
    #[uniffi::constructor]
    pub fn new(viewer_id: String, host_id: String, callback: Box<dyn ViewerCallback>) -> Arc<Self> {
        let cb: Arc<dyn ViewerCallback> = Arc::from(callback);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let is_connected = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(Notify::new());
        let is_conn = is_connected.clone();
        let sd = shutdown.clone();
        let cb_clone = cb.clone();

        runtime().spawn(async move {
            run_session_loop(viewer_id, host_id, cb_clone, cmd_rx, is_conn, sd).await;
        });

        Arc::new(Self {
            cmd_tx,
            is_connected,
            shutdown,
            params: Mutex::new(SessionParams::default()),
        })
    }

    pub fn connect(&self, server_addr: String, stun_servers: Vec<String>) {
        let params = self.params.blocking_lock().clone();
        let _ = self.cmd_tx.send(SessionCommand::Connect {
            server_addr,
            stun_servers,
            params,
        });
    }
    pub fn disconnect(&self) {
        self.shutdown.notify_one();
    }
    pub fn set_pic_quality(&self, pic_quality: u32) {
        self.params.blocking_lock().pic_quality = pic_quality;
    }
    pub fn set_resolution(&self, width: u32, height: u32) {
        let mut p = self.params.blocking_lock();
        p.width = width;
        p.height = height;
    }
    pub fn set_fps(&self, fps: u32) {
        self.params.blocking_lock().fps = fps;
    }
    pub fn send_mouse_event(&self, x: f64, y: f64, button: u8, pressed: bool) {
        let _ = self.cmd_tx.send(SessionCommand::MouseEvent {
            x,
            y,
            button,
            pressed,
        });
    }
    pub fn send_key_event(&self, key_code: u32, pressed: bool) {
        let _ = self
            .cmd_tx
            .send(SessionCommand::KeyEvent { key_code, pressed });
    }
    pub fn send_clipboard_data(&self, data: Vec<u8>) {
        let _ = self.cmd_tx.send(SessionCommand::SendClipboardData { data });
    }
    pub fn send_files(&self, file_list: Vec<u8>, chunks: Vec<u8>) {
        let _ = self
            .cmd_tx
            .send(SessionCommand::SendFiles { file_list, chunks });
    }
    pub fn is_connected(&self) -> bool {
        self.is_connected.load(Ordering::SeqCst)
    }
}

// ── Background session loop ────────────────────────────────────────────

async fn run_session_loop(
    viewer_id: String,
    host_id: String,
    cb: Arc<dyn ViewerCallback>,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    is_conn: Arc<AtomicBool>,
    sd: Arc<Notify>,
) {
    let mut viewer: Option<RtcViewer> = None;
    let mut ws_disconnected: Option<Arc<Notify>> = None;

    loop {
        tokio::select! {
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    SessionCommand::Connect { server_addr, stun_servers, params } => {
                        handle_session_connect(
                            &mut viewer, &mut ws_disconnected,
                            &viewer_id, &host_id, &cb, &is_conn,
                            server_addr, stun_servers, params,
                        ).await;
                    }
                    SessionCommand::Disconnect => {
                        if let Some(ref v) = viewer {
                            info!("[Session] User disconnect");
                            v.disconnect().await;
                        }
                        is_conn.store(false, Ordering::SeqCst);
                        viewer = None;
                        ws_disconnected = None;
                    }
                    SessionCommand::MouseEvent { x, y, button, pressed } => {
                        if let Some(ref v) = viewer {
                            let _ = v.send_mouse_event(x, y, button, pressed).await;
                        }
                    }
                    SessionCommand::KeyEvent { key_code, pressed } => {
                        if let Some(ref v) = viewer {
                            let _ = v.send_key_event(key_code, pressed).await;
                        }
                    }
                    SessionCommand::SendClipboardData { data } => {
                        if let Some(ref v) = viewer {
                            let _ = v.send_clipboard_data(data).await;
                        }
                    }
                    SessionCommand::SendFiles { file_list, chunks } => {
                        if let Some(ref v) = viewer {
                            let _ = v.send_files(file_list, chunks).await;
                        }
                    }
                }
            }
            // Natural connection drop (WebSocket closed)
            _ = async {
                if let Some(ref wd) = ws_disconnected {
                    wd.notified().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                info!("[Session] Connection dropped naturally");
                is_conn.store(false, Ordering::SeqCst);
                viewer = None;
                ws_disconnected = None;
            }
            // External shutdown from disconnect()
            _ = sd.notified() => {
                info!("[Session] Shutting down");
                if let Some(ref v) = viewer {
                    v.disconnect().await;
                }
                is_conn.store(false, Ordering::SeqCst);
                break;
            }
        }
    }
    info!("[Session] Background task exited");
}

async fn handle_session_connect(
    viewer: &mut Option<RtcViewer>,
    ws_disconnected: &mut Option<Arc<Notify>>,
    viewer_id: &str,
    host_id: &str,
    cb: &Arc<dyn ViewerCallback>,
    is_conn: &Arc<AtomicBool>,
    server_addr: String,
    stun_servers: Vec<String>,
    params: SessionParams,
) {
    // Disconnect any existing session first
    if let Some(v) = viewer.take() {
        info!("[Session] Disconnecting previous session");
        v.disconnect().await;
    }

    // Create new viewer with the requested params
    let mut v = RtcViewer::new(viewer_id.to_string(), host_id.to_string());
    v.set_pic_quality(params.pic_quality);
    v.set_resolution(params.width, params.height);
    v.set_fps(params.fps);
    info!(
        "[Session] Viewer params: quality={} {}x{} fps={}",
        params.pic_quality, params.width, params.height, params.fps
    );

    let wd = v.ws_disconnected.clone();
    v.set_callback(Arc::new(CallbackBridge { inner: cb.clone() }));

    match v.connect(server_addr, stun_servers).await {
        Ok(()) => {
            info!("[Session] Connection initiated");
            is_conn.store(true, Ordering::SeqCst);
            *viewer = Some(v);
            *ws_disconnected = Some(wd);
        }
        Err(e) => {
            error!("[Session] Connect failed: {}", e);
            cb.on_error(format!("Connection failed: {}", e));
            is_conn.store(false, Ordering::SeqCst);
        }
    }
}

uniffi::setup_scaffolding!();
