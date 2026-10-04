//! 手机发链接到电脑：在局域网地址上运行一个小网页服务，手机扫码打开后粘贴链接发送。
//!
//! - 默认关闭；只接受局域网和本机地址的请求
//! - 二维码 / 快捷指令里带访问令牌；新设备第一次发送时在电脑上确认配对，之后可在设置里撤销
//! - 网页只能发送链接、查看自己发送的任务状态，不能浏览电脑上的文件
//! - 限制请求大小和频率；关闭功能时立即停止服务

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::download::TaskStatus;
use crate::error::{AppError, AppResult};
use crate::settings::PairedDevice;
use crate::{clipboard, db, providers, tray, AppState};

pub const EVT_PAIR_REQUEST: &str = "phone://pair-request";
pub const EVT_RECEIVED: &str = "phone://received";

const MAX_HEADER: usize = 8 * 1024;
const MAX_BODY: usize = 16 * 1024;
/// 每个地址每分钟最多请求数
const RATE_PER_MIN: u32 = 60;
const MAX_SENDS: usize = 200;

#[derive(Default)]
pub struct PhoneState {
    server: Mutex<Option<Server>>,
    sends: Mutex<Vec<Sent>>,
    pending: Mutex<Vec<PairRequest>>,
    rate: Mutex<HashMap<IpAddr, (Instant, u32)>>,
    next_id: Mutex<u64>,
    last_error: Mutex<Option<String>>,
}

struct Server {
    port: u16,
    task: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairRequest {
    pub device_id: String,
    pub name: String,
    pub ip: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Sent {
    id: u64,
    #[serde(skip)]
    device: String,
    text: String,
    title: Option<String>,
    /// pending_pair / rejected / resolving / confirm / queued / downloading / done / failed
    state: String,
    message: Option<String>,
    #[serde(skip)]
    task_ids: Vec<i64>,
    at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneInfo {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    /// 手机网页地址（带令牌）
    pub url: Option<String>,
    /// 快捷指令使用的接口地址（不带令牌）
    pub api_url: Option<String>,
    pub token: String,
    pub qr_svg: Option<String>,
    pub devices: Vec<PairedDevice>,
    pub pending: Vec<PairRequest>,
    pub error: Option<String>,
}

fn st(app: &AppHandle) -> Arc<AppState> {
    app.state::<Arc<AppState>>().inner().clone()
}

fn random_token() -> String {
    use aes_gcm::aead::rand_core::RngCore;
    let mut b = [0u8; 16];
    aes_gcm::aead::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

/// 本机的局域网 IPv4 地址（通过 UDP “连接”选路得到，不会真的发包）。
pub fn lan_ip() -> Option<IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.168.1.1:9").or_else(|_| sock.connect("10.255.255.255:9")).ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// 只接受局域网、链路本地和本机地址。
pub fn is_lan(ip: IpAddr) -> bool {
    match ip {
        // 100.64.0.0/10：运营商级 NAT 和 Tailscale 等组网工具使用的地址段，不属于公网
        IpAddr::V4(v) => v.is_private() || v.is_loopback() || v.is_link_local() || (v.octets()[0] == 100 && (v.octets()[1] & 0xc0) == 64),
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return is_lan(IpAddr::V4(v4));
            }
            let seg = v.segments()[0];
            v.is_loopback() || (seg & 0xfe00) == 0xfc00 || (seg & 0xffc0) == 0xfe80
        }
    }
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn info(app: &AppHandle) -> PhoneInfo {
    let state = st(app);
    let settings = state.settings();
    let running = state.phone.server.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.port);
    let pending = state.phone.pending.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let ip = lan_ip();
    let (url, api_url) = match (running, ip) {
        (Some(port), Some(ip)) => (Some(format!("http://{ip}:{port}/?t={}", settings.phone.token)), Some(format!("http://{ip}:{port}/api/send"))),
        _ => (None, None),
    };
    let qr_svg = url
        .as_deref()
        .and_then(|u| qrcode::QrCode::new(u.as_bytes()).ok())
        .map(|c| c.render::<qrcode::render::svg::Color>().min_dimensions(200, 200).quiet_zone(true).build());
    let mut error = state.phone.last_error.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if running.is_some() && ip.is_none() {
        error = Some("没有找到局域网地址。请确认电脑已连接 Wi-Fi 或有线网络。".into());
    }
    PhoneInfo {
        enabled: settings.phone.enabled,
        running: running.is_some(),
        port: running.unwrap_or(settings.phone.port),
        url,
        api_url,
        token: settings.phone.token.clone(),
        qr_svg,
        devices: settings.phone.devices.clone(),
        pending,
        error,
    }
}

fn update_phone_settings(app: &AppHandle, f: impl FnOnce(&mut crate::settings::PhoneSettings)) -> AppResult<()> {
    let state = st(app);
    let mut s = state.settings.write().unwrap_or_else(|e| e.into_inner());
    f(&mut s.phone);
    s.save(&state.settings_path)
}

/// 开启或关闭手机发送。
pub async fn set_enabled(app: &AppHandle, on: bool) -> AppResult<PhoneInfo> {
    update_phone_settings(app, |p| {
        p.enabled = on;
        if p.token.is_empty() {
            p.token = random_token();
        }
    })?;
    if on {
        start(app).await?;
    } else {
        stop(app);
    }
    Ok(info(app))
}

pub fn reset_token(app: &AppHandle) -> AppResult<PhoneInfo> {
    update_phone_settings(app, |p| {
        p.token = random_token();
        p.devices.clear();
    })?;
    Ok(info(app))
}

pub fn revoke(app: &AppHandle, device_id: &str) -> AppResult<PhoneInfo> {
    update_phone_settings(app, |p| p.devices.retain(|d| d.id != device_id))?;
    Ok(info(app))
}

/// 电脑端确认或拒绝配对；确认后处理该设备等待中的链接。
pub fn respond_pair(app: &AppHandle, device_id: &str, accept: bool) -> AppResult<PhoneInfo> {
    let state = st(app);
    let req = {
        let mut pending = state.phone.pending.lock().unwrap_or_else(|e| e.into_inner());
        let pos = pending.iter().position(|p| p.device_id == device_id).ok_or_else(|| AppError::invalid("这个配对请求已经处理过了。"))?;
        pending.remove(pos)
    };
    if accept {
        let now = db::now();
        update_phone_settings(app, |p| {
            p.devices.retain(|d| d.id != req.device_id);
            p.devices.push(PairedDevice { id: req.device_id.clone(), name: req.name.clone(), added_at: now, last_seen: now });
        })?;
    }
    let waiting: Vec<(u64, String)> = {
        let mut sends = state.phone.sends.lock().unwrap_or_else(|e| e.into_inner());
        sends
            .iter_mut()
            .filter(|s| s.device == device_id && s.state == "pending_pair")
            .map(|s| {
                if !accept {
                    s.state = "rejected".into();
                    s.message = Some("电脑上拒绝了配对".into());
                }
                (s.id, s.text.clone())
            })
            .collect()
    };
    if accept {
        for (id, text) in waiting {
            process(app.clone(), id, text);
        }
    }
    Ok(info(app))
}

pub async fn start(app: &AppHandle) -> AppResult<()> {
    stop(app);
    let state = st(app);
    let port = state.settings().phone.port;
    // 优先使用上次的端口，让快捷指令里的地址不变
    let listener = match TcpListener::bind(("0.0.0.0", port)).await {
        Ok(l) => l,
        Err(_) if port != 0 => TcpListener::bind(("0.0.0.0", 0)).await?,
        Err(e) => return Err(e.into()),
    };
    let actual = listener.local_addr()?.port();
    if actual != port {
        update_phone_settings(app, |p| p.port = actual)?;
    }
    *state.phone.last_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let app2 = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else { continue };
            if !is_lan(peer.ip()) {
                continue;
            }
            let app = app2.clone();
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(15), handle(app, sock, peer)).await;
            });
        }
    });
    log::info!("phone send server listening on port {actual}");
    *state.phone.server.lock().unwrap_or_else(|e| e.into_inner()) = Some(Server { port: actual, task });
    Ok(())
}

pub fn stop(app: &AppHandle) {
    if let Some(s) = st(app).phone.server.lock().unwrap_or_else(|e| e.into_inner()).take() {
        s.task.abort();
        log::info!("phone send server stopped");
    }
}

/// 启动时按设置恢复服务。
pub fn restore(app: &AppHandle) {
    if !st(app).settings().phone.enabled {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = start(&app).await {
            log::warn!("phone send server failed to start: {e}");
            *st(&app).phone.last_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("服务启动失败：{e}"));
        }
    });
}

// ---------- HTTP ----------

pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                Ok(b) => {
                    out.push(b);
                    i += 2;
                }
                Err(_) => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 解析请求头部分（不含正文）。返回请求和正文长度。
pub fn parse_head(head: &str) -> Option<(Request, usize)> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let target = first.next()?;
    let (path, qs) = target.split_once('?').unwrap_or((target, ""));
    let query = qs.split('&').filter_map(|kv| kv.split_once('=').map(|(k, v)| (url_decode(k), url_decode(v)))).collect();
    let headers: HashMap<String, String> =
        lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let len = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    Some((Request { method, path: path.to_string(), query, headers, body: vec![] }, len))
}

async fn read_request(sock: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::with_capacity(2048);
    let mut tmp = [0u8; 2048];
    let head_end = loop {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        if buf.len() > MAX_HEADER {
            return None;
        }
    };
    let (mut req, len) = parse_head(std::str::from_utf8(&buf[..head_end]).ok()?)?;
    if len > MAX_BODY {
        return None;
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(len);
    req.body = body;
    Some(req)
}

async fn respond(sock: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\r\n",
        body.len()
    );
    let _ = sock.write_all(head.as_bytes()).await;
    let _ = sock.write_all(body).await;
    let _ = sock.flush().await;
}

async fn json(sock: &mut TcpStream, status: &str, v: serde_json::Value) {
    respond(sock, status, "application/json; charset=utf-8", v.to_string().as_bytes()).await;
}

fn rate_ok(state: &AppState, ip: IpAddr) -> bool {
    let mut rate = state.phone.rate.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let e = rate.entry(ip).or_insert((now, 0));
    if now.duration_since(e.0) > Duration::from_secs(60) {
        *e = (now, 0);
    }
    e.1 += 1;
    e.1 <= RATE_PER_MIN
}

#[derive(Deserialize)]
struct SendBody {
    text: String,
    #[serde(default)]
    device: String,
    #[serde(default)]
    name: String,
}

fn clean_device(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(64).collect()
}

async fn handle(app: AppHandle, mut sock: TcpStream, peer: SocketAddr) {
    let state = st(&app);
    let Some(req) = read_request(&mut sock).await else { return };
    if !rate_ok(&state, peer.ip()) {
        json(&mut sock, "429 Too Many Requests", serde_json::json!({"error": "请求太频繁，请稍后再试"})).await;
        return;
    }
    let token = state.settings().phone.token;
    let given = req.headers.get("x-token").or_else(|| req.query.get("t")).cloned().unwrap_or_default();
    let authorized = !token.is_empty() && ct_eq(&given, &token);

    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => {
            if authorized {
                respond(&mut sock, "200 OK", "text/html; charset=utf-8", page(&token).as_bytes()).await;
            } else {
                respond(&mut sock, "403 Forbidden", "text/html; charset=utf-8", "<meta name=viewport content='width=device-width'><p style='font:16px sans-serif;padding:24px'>二维码已失效，请在电脑上的清影“设置 → 手机发送”重新扫码。</p>".as_bytes()).await;
            }
        }
        ("GET", "/favicon.ico") => respond(&mut sock, "404 Not Found", "text/plain", b"").await,
        (_, p) if p.starts_with("/api/") && !authorized => json(&mut sock, "403 Forbidden", serde_json::json!({"error": "访问令牌无效，请重新扫码"})).await,
        ("POST", "/api/send") => {
            let body: Result<SendBody, _> = serde_json::from_slice(&req.body).or_else(|_| {
                // 快捷指令也可以直接发送纯文本
                Ok::<_, serde_json::Error>(SendBody { text: String::from_utf8_lossy(&req.body).into_owned(), device: String::new(), name: String::new() })
            });
            let Ok(body) = body else { return };
            let device = clean_device(if body.device.is_empty() { req.headers.get("x-device").map(String::as_str).unwrap_or("") } else { &body.device });
            let device = if device.is_empty() { format!("ip-{}", peer.ip()) } else { device };
            let name: String = if body.name.trim().is_empty() { "手机".to_string() } else { body.name.trim().chars().take(40).collect() };
            let text: String = body.text.trim().chars().take(4000).collect();
            if providers::extract_urls(&text).is_empty() {
                json(&mut sock, "400 Bad Request", serde_json::json!({"error": "没有找到链接，请粘贴分享文案或链接"})).await;
                return;
            }
            let (id, state_name) = receive(&app, &device, &name, &peer, text);
            json(&mut sock, "200 OK", serde_json::json!({"id": id, "state": state_name})).await;
        }
        ("GET", "/api/status") => {
            let device = clean_device(req.query.get("device").map(String::as_str).unwrap_or(""));
            let list = statuses(&app, &device);
            json(&mut sock, "200 OK", serde_json::json!({"items": list})).await;
        }
        _ => respond(&mut sock, "404 Not Found", "text/plain", b"not found").await,
    }
}

/// 收到一条链接：已配对的设备直接处理，新设备先请求配对。返回编号和状态。
fn receive(app: &AppHandle, device: &str, name: &str, peer: &SocketAddr, text: String) -> (u64, String) {
    let state = st(app);
    let paired = state.settings().phone.devices.iter().any(|d| d.id == device);
    let id = {
        let mut n = state.phone.next_id.lock().unwrap_or_else(|e| e.into_inner());
        *n += 1;
        *n
    };
    let initial = if paired { "resolving" } else { "pending_pair" };
    {
        let mut sends = state.phone.sends.lock().unwrap_or_else(|e| e.into_inner());
        sends.push(Sent {
            id,
            device: device.to_string(),
            text: text.clone(),
            title: None,
            state: initial.into(),
            message: None,
            task_ids: vec![],
            at: db::now(),
        });
        let excess = sends.len().saturating_sub(MAX_SENDS);
        sends.drain(..excess);
    }
    if paired {
        let now = db::now();
        let _ = update_phone_settings(app, |p| {
            if let Some(d) = p.devices.iter_mut().find(|d| d.id == device) {
                d.last_seen = now;
            }
        });
        process(app.clone(), id, text);
    } else {
        let req = PairRequest { device_id: device.to_string(), name: name.to_string(), ip: peer.ip().to_string() };
        let mut pending = state.phone.pending.lock().unwrap_or_else(|e| e.into_inner());
        if !pending.iter().any(|p| p.device_id == device) {
            pending.push(req.clone());
            let _ = app.emit(EVT_PAIR_REQUEST, req);
            tray::show_main(app);
        }
    }
    (id, initial.into())
}

fn set_sent(app: &AppHandle, id: u64, f: impl FnOnce(&mut Sent)) {
    if let Some(s) = st(app).phone.sends.lock().unwrap_or_else(|e| e.into_inner()).iter_mut().find(|s| s.id == id) {
        f(s);
    }
}

/// 处理一条已授权的链接：开启“自动下载”时直接加入队列，否则交给电脑端确认。
fn process(app: AppHandle, id: u64, text: String) {
    let settings = st(&app).settings();
    let _ = app.emit(EVT_RECEIVED, text.clone());
    if !settings.auto_download {
        let links = providers::detect_links_with(&text, &crate::settings::Settings { clipboard_all_sites: true, ..settings.clone() });
        set_sent(&app, id, |s| {
            s.state = "confirm".into();
            s.message = Some("已发送到电脑，请在电脑上确认下载".into());
        });
        clipboard::on_links(&app, text, links, false);
        return;
    }
    tauri::async_runtime::spawn(async move {
        match clipboard::resolve_and_enqueue_full(&app, &text).await {
            Ok(r) => set_sent(&app, id, |s| {
                s.title = Some(r.title.clone());
                if r.task_ids.is_empty() {
                    s.state = "done".into();
                    s.message = Some(if r.already_queued > 0 { "已在下载队列中".into() } else { "之前已下载过".into() });
                } else {
                    s.state = "queued".into();
                }
                s.task_ids = r.task_ids;
            }),
            Err(e) => set_sent(&app, id, |s| {
                s.state = "failed".into();
                s.message = Some(e.to_string());
            }),
        }
    });
}

/// 某个设备发送的链接及其最新状态（按下载任务状态更新）。
fn statuses(app: &AppHandle, device: &str) -> Vec<Sent> {
    let state = st(app);
    let tasks = state.downloads.snapshots();
    let mut sends = state.phone.sends.lock().unwrap_or_else(|e| e.into_inner());
    for s in sends.iter_mut().filter(|s| s.device == device && !s.task_ids.is_empty()) {
        let mine: Vec<_> = tasks.iter().filter(|t| s.task_ids.contains(&t.id)).collect();
        if mine.is_empty() {
            continue;
        }
        let (state_name, msg) = if mine.iter().all(|t| t.status == TaskStatus::Done) {
            ("done", None)
        } else if let Some(t) = mine.iter().find(|t| t.status == TaskStatus::Failed) {
            ("failed", t.error.clone())
        } else if mine.iter().any(|t| t.status == TaskStatus::Running) {
            let (r, total) = mine.iter().fold((0u64, 0u64), |(r, tt), t| (r + t.received, tt + t.total.unwrap_or(0)));
            ("downloading", (total > 0).then(|| format!("{}%", r * 100 / total)))
        } else {
            ("queued", None)
        };
        s.state = state_name.into();
        s.message = msg;
    }
    sends.iter().filter(|s| s.device == device).rev().take(20).cloned().collect()
}

fn page(token: &str) -> String {
    PAGE.replace("__TOKEN__", token)
}

const PAGE: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>发送到清影</title>
<style>
:root{--bg:#f6f4f1;--card:#fff;--fg:#1f1d1a;--mute:#7b756d;--acc:#d9772f;--line:#e6e1da;--ok:#2f8a57;--err:#c4423a}
@media (prefers-color-scheme:dark){:root{--bg:#181715;--card:#22201d;--fg:#ece8e2;--mute:#9b948a;--line:#34312c}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 -apple-system,BlinkMacSystemFont,"PingFang SC","Microsoft YaHei",sans-serif}
main{max-width:520px;margin:0 auto;padding:20px 16px 40px}
h1{font-size:19px;margin:4px 0 2px}h1 b{color:var(--acc)}p.m{color:var(--mute);margin:0 0 14px;font-size:13px}
textarea{width:100%;min-height:110px;padding:12px;border:1px solid var(--line);border-radius:10px;background:var(--card);color:var(--fg);font:inherit;resize:vertical}
button{width:100%;margin-top:10px;padding:13px;border:0;border-radius:10px;background:var(--acc);color:#fff;font:600 16px/1 inherit;font-family:inherit}
button:disabled{opacity:.6}
#msg{min-height:20px;margin:10px 0;font-size:13px;color:var(--mute)}
ul{list-style:none;margin:8px 0 0;padding:0}li{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:10px 12px;margin-bottom:8px}
li .t{font-size:14px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}li .s{font-size:12px;color:var(--mute)}
.ok{color:var(--ok)!important}.err{color:var(--err)!important}
</style></head><body><main>
<h1>发送到<b>清影</b></h1>
<p class="m">长按输入框粘贴分享文案或链接，电脑上的清影会开始下载。</p>
<textarea id="text" placeholder="粘贴链接，例如 https://v.douyin.com/…"></textarea>
<button id="send">发送到电脑</button>
<div id="msg"></div>
<ul id="list"></ul>
</main><script>
const TOKEN="__TOKEN__";
let dev=localStorage.getItem("cc_device");if(!dev){dev="web-"+Math.random().toString(36).slice(2,12);localStorage.setItem("cc_device",dev)}
const ua=navigator.userAgent;const name=/iPhone/.test(ua)?"iPhone":/iPad/.test(ua)?"iPad":/Android/.test(ua)?"Android 手机":"浏览器";
const S={pending_pair:"等待电脑确认配对",rejected:"电脑拒绝了配对",resolving:"解析中",confirm:"已发送，请在电脑上确认",queued:"排队中",downloading:"下载中",done:"已完成",failed:"失败"};
const $=id=>document.getElementById(id);
async function api(path,opt={}){const r=await fetch(path,{...opt,headers:{"X-Token":TOKEN,"Content-Type":"application/json"}});const j=await r.json().catch(()=>({}));if(!r.ok)throw new Error(j.error||("HTTP "+r.status));return j}
$("send").onclick=async()=>{const text=$("text").value.trim();if(!text){$("msg").textContent="请先粘贴链接";return}
$("send").disabled=true;try{const r=await api("/api/send",{method:"POST",body:JSON.stringify({text,device:dev,name})});$("text").value="";$("msg").textContent=r.state==="pending_pair"?"第一次使用：请在电脑上点“允许”完成配对":"已发送";refresh()}catch(e){$("msg").textContent=e.message;$("msg").className="err"}finally{$("send").disabled=false}};
let timer;async function refresh(){clearTimeout(timer);try{const r=await api("/api/status?device="+encodeURIComponent(dev));const ul=$("list");ul.innerHTML="";
let busy=false;for(const it of r.items){const li=document.createElement("li");const t=document.createElement("div");t.className="t";t.textContent=it.title||it.text;
const s=document.createElement("div");s.className="s"+(it.state==="done"?" ok":it.state==="failed"||it.state==="rejected"?" err":"");s.textContent=(S[it.state]||it.state)+(it.message?" · "+it.message:"");
li.append(t,s);ul.append(li);if(!["done","failed","rejected"].includes(it.state))busy=true}
timer=setTimeout(refresh,busy?2000:10000)}catch(e){timer=setTimeout(refresh,10000)}}
refresh();
</script></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_addresses() {
        for ip in ["192.168.1.5", "10.0.0.2", "100.100.1.2", "172.16.3.4", "127.0.0.1", "169.254.1.1", "::1", "fe80::1", "fd00::5", "::ffff:192.168.0.9"] {
            assert!(is_lan(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "100.128.0.1", "172.32.0.1", "2001:db8::1", "::ffff:1.1.1.1"] {
            assert!(!is_lan(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn request_head() {
        let (r, len) = parse_head("POST /api/send?t=abc%20d&x=1 HTTP/1.1\r\nHost: x\r\nContent-Length: 12\r\nX-Token: tok").unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/api/send");
        assert_eq!(r.query.get("t").map(String::as_str), Some("abc d"));
        assert_eq!(r.headers.get("x-token").map(String::as_str), Some("tok"));
        assert_eq!(len, 12);
        assert_eq!(url_decode("%E4%B8%AD+a%2"), "中 a%2");
    }

    #[test]
    fn helpers() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "ab"));
        assert_eq!(clean_device("web-1a<script>"), "web-1ascript");
        assert_eq!(random_token().len(), 32);
        assert!(page("TOK").contains("const TOKEN=\"TOK\""));
    }
}
