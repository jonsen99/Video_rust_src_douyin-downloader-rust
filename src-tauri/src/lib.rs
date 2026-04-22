//! 抖音视频下载器 - Tauri 应用

pub mod api;
pub mod config;
pub mod downloader;
pub mod history;
pub mod media_proxy;
pub mod sign;

use api::{CookieStatus, DouyinClient, DownloadHistory, DownloadMediaItem, MediaType, UserInfo, VideoInfo};
use config::AppConfig;
use downloader::{Downloader, DownloaderEvent};
use history::HistoryManager;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use tauri::webview::Cookie;
use tokio::sync::{mpsc, Mutex};
use url::Url;

/// 应用状态
#[derive(Clone)]
pub struct AppState {
    pub(crate) config: Arc<Mutex<AppConfig>>,
    pub(crate) client: Arc<Mutex<Option<DouyinClient>>>,
    pub(crate) downloader: Arc<Mutex<Option<Downloader>>>,
    pub(crate) history: Arc<Mutex<HistoryManager>>,
    pub(crate) app_handle: Arc<Mutex<Option<tauri::AppHandle>>>,
    pub(crate) cookie_login: Arc<Mutex<Option<CookieLoginSession>>>,
    pub(crate) media_http_client: reqwest::Client,
    pub(crate) media_redirect_cache: Arc<Mutex<HashMap<String, String>>>,
}

#[derive(Clone)]
pub struct CookieLoginSession {
    pub label: String,
    pub cancelled: Arc<AtomicBool>,
}

impl AppState {
    pub fn new() -> Self {
        let config = AppConfig::load();
        let history = HistoryManager::load();
        let media_http_client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("failed to build media HTTP client");
        Self {
            config: Arc::new(Mutex::new(config)),
            client: Arc::new(Mutex::new(None)),
            downloader: Arc::new(Mutex::new(None)),
            history: Arc::new(Mutex::new(history)),
            app_handle: Arc::new(Mutex::new(None)),
            cookie_login: Arc::new(Mutex::new(None)),
            media_http_client,
            media_redirect_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

async fn get_client(state: &State<'_, AppState>) -> Result<DouyinClient, String> {
    state
        .client
        .lock()
        .await
        .clone()
        .ok_or_else(|| "Client not initialized".to_string())
}

async fn emit_cookie_login_status(app: &tauri::AppHandle, payload: serde_json::Value) {
    let _ = app.emit("cookie-login-status", payload);
}

fn serialize_cookie_string(cookies: &[Cookie<'static>]) -> String {
    cookies
        .iter()
        .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
        .collect::<Vec<_>>()
        .join("; ")
}

fn parse_cookie_string(cookie_string: &str) -> Vec<Cookie<'static>> {
    cookie_string
        .split(';')
        .filter_map(|item| {
            let item = item.trim();
            if item.is_empty() || !item.contains('=') {
                return None;
            }

            Cookie::parse(format!("{item}; Domain=.douyin.com; Path=/"))
                .ok()
                .map(|cookie| cookie.into_owned())
        })
        .collect()
}

fn has_douyin_login_cookie(cookies: &[Cookie<'static>]) -> bool {
    let mut cookie_names = HashSet::new();
    let mut passport_auth_status = None;

    for cookie in cookies {
        let name = cookie.name().to_string();
        if name == "passport_auth_status" {
            passport_auth_status = Some(cookie.value().to_string());
        }
        cookie_names.insert(name);
    }

    passport_auth_status.as_deref() == Some("1")
        || cookie_names.contains("sessionid")
        || cookie_names.contains("sessionid_ss")
        || cookie_names.contains("sid_guard")
}

async fn verify_douyin_login_cookie(
    config: &AppConfig,
    cookie: &str,
) -> Result<UserInfo, String> {
    let mut next_config = config.clone();
    next_config.cookie = cookie.to_string();

    let client = DouyinClient::new(next_config)
        .map_err(|error| format!("创建 Cookie 校验客户端失败: {}", error))?;

    client
        .get_current_user()
        .await
        .map_err(|error| format!("登录态校验失败: {}", error))
}

fn normalize_duration_seconds(value: i64) -> i64 {
    if value <= 0 {
        return 0;
    }

    if value >= 100_000 {
        return std::cmp::max(1, (value as f64 / 100_000.0).round() as i64);
    }
    if value >= 1_000 {
        return std::cmp::max(1, (value as f64 / 1_000.0).round() as i64);
    }
    if value >= 100 {
        return std::cmp::max(1, (value as f64 / 100.0).round() as i64);
    }

    std::cmp::max(1, value)
}

fn python_media_type(video: &VideoInfo) -> &'static str {
    let has_images = video
        .image_urls
        .as_ref()
        .map(|urls| !urls.is_empty())
        .unwrap_or(false);
    let has_live = video.has_live_photo
        || video
            .live_photo_urls
            .as_ref()
            .map(|urls| !urls.is_empty())
            .unwrap_or(false);

    if has_live && has_images {
        "mixed"
    } else if has_live {
        "live_photo"
    } else if has_images || video.is_image {
        "image"
    } else if !video.video.play_addr.is_empty() {
        "video"
    } else {
        "unknown"
    }
}

fn python_media_urls(video: &VideoInfo) -> Vec<serde_json::Value> {
    let mut items = Vec::new();

    if let Some(urls) = &video.live_photo_urls {
        for url in urls {
            if !url.is_empty() {
                items.push(serde_json::json!({ "type": "live_photo", "url": url }));
            }
        }
    }

    if let Some(urls) = &video.image_urls {
        for url in urls {
            if !url.is_empty() {
                items.push(serde_json::json!({ "type": "image", "url": url }));
            }
        }
    }

    if items.is_empty() && !video.video.play_addr.is_empty() {
        items.push(serde_json::json!({ "type": "video", "url": video.video.play_addr }));
    }

    items
}

fn python_cover_url(video: &VideoInfo) -> String {
    if !video.video.cover.is_empty() {
        return video.video.cover.clone();
    }

    video
        .image_urls
        .as_ref()
        .and_then(|urls| urls.first())
        .cloned()
        .unwrap_or_default()
}

fn python_music_play_url(video: &VideoInfo) -> String {
    video
        .music
        .as_ref()
        .and_then(|music| music.play_url.clone())
        .unwrap_or_default()
}

fn python_music_info(video: &VideoInfo) -> serde_json::Value {
    let play_url = python_music_play_url(video);
    serde_json::json!({
        "title": video.music.as_ref().map(|music| music.title.clone()).unwrap_or_default(),
        "author": video.music.as_ref().map(|music| music.author.clone()).unwrap_or_default(),
        "play_url": play_url,
        "duration": normalize_duration_seconds(video.music.as_ref().map(|music| music.duration).unwrap_or(0)),
    })
}

fn python_user_value(user: &UserInfo) -> serde_json::Value {
    serde_json::json!({
        "nickname": user.nickname,
        "unique_id": user.unique_id,
        "follower_count": user.follower_count,
        "following_count": user.following_count,
        "total_favorited": user.total_favorited,
        "aweme_count": user.aweme_count,
        "signature": user.signature,
        "sec_uid": user.sec_uid,
        "avatar_thumb": user.avatar_thumb,
        "avatar_larger": user.avatar_larger,
    })
}

fn python_video_summary(
    video: &VideoInfo,
    include_duration: bool,
    include_music: bool,
) -> serde_json::Value {
    let media_type = python_media_type(video);
    let media_urls = python_media_urls(video);
    let mut bgm_url = python_music_play_url(video);

    if bgm_url.is_empty() && !video.video.play_addr.is_empty() {
        bgm_url = video.video.play_addr.clone();
    }

    let mut value = serde_json::json!({
        "aweme_id": video.aweme_id,
        "desc": video.desc,
        "create_time": video.create_time,
        "digg_count": video.statistics.digg_count,
        "comment_count": video.statistics.comment_count,
        "share_count": video.statistics.share_count,
        "cover_url": python_cover_url(video),
        "media_type": media_type,
        "media_urls": media_urls,
        "bgm_url": bgm_url,
        "author": {
            "nickname": video.author.nickname,
            "avatar_thumb": video.author.avatar_thumb,
            "sec_uid": video.author.sec_uid,
        }
    });

    if include_duration {
        value["duration"] = serde_json::json!(normalize_duration_seconds(video.video.duration));
    }

    if include_music {
        let music = python_music_info(video);
        value["music"] = music.clone();
        value["music_title"] = music["title"].clone();
        value["music_author"] = music["author"].clone();
        value["music_url"] = music["play_url"].clone();
        value["music_duration"] = music["duration"].clone();
    }

    value
}

fn python_video_detail_value(video: &VideoInfo) -> serde_json::Value {
    let media_type = python_media_type(video);
    let media_urls = python_media_urls(video);

    serde_json::json!({
        "aweme_id": video.aweme_id,
        "desc": video.desc,
        "create_time": video.create_time,
        "digg_count": video.statistics.digg_count,
        "comment_count": video.statistics.comment_count,
        "share_count": video.statistics.share_count,
        "author": {
            "nickname": video.author.nickname,
            "unique_id": video.author.uid,
            "sec_uid": video.author.sec_uid,
            "avatar_thumb": video.author.avatar_thumb,
        },
        "statistics": {
            "digg_count": video.statistics.digg_count,
            "comment_count": video.statistics.comment_count,
            "share_count": video.statistics.share_count,
            "play_count": video.statistics.play_count,
        },
        "media_type": media_type,
        "media_urls": media_urls.clone(),
        "raw_media_type": media_type,
        "cover_url": python_cover_url(video),
        "images": video.image_urls.clone().unwrap_or_default(),
        "videos": media_urls,
        "bgm_url": python_music_play_url(video),
    })
}

fn python_recommended_video(video: &VideoInfo) -> serde_json::Value {
    let music = python_music_info(video);

    serde_json::json!({
        "aweme_id": video.aweme_id,
        "desc": video.desc,
        "create_time": video.create_time,
        "author": {
            "uid": video.author.uid,
            "nickname": video.author.nickname,
            "avatar_thumb": video.author.avatar_thumb,
            "sec_uid": video.author.sec_uid,
        },
        "statistics": {
            "digg_count": video.statistics.digg_count,
            "comment_count": video.statistics.comment_count,
            "share_count": video.statistics.share_count,
            "play_count": video.statistics.play_count,
        },
        "video": {
            "cover": video.video.cover,
            "dynamic_cover": video.video.dynamic_cover,
            "play_addr": video.video.play_addr,
            "width": video.video.width,
            "height": video.video.height,
            "duration": normalize_duration_seconds(video.video.duration),
        },
        "music": {
            "title": music["title"],
            "author": music["author"],
            "play_url": music["play_url"],
            "duration": music["duration"],
            "cover": video.music.as_ref().map(|item| item.cover_thumb.clone()).unwrap_or_default(),
        }
    })
}

fn download_media_type_from_payload(payload: &serde_json::Value) -> String {
    if let Some(value) = payload.get("raw_media_type") {
        if let Some(media_type) = value.as_str() {
            return media_type.trim().to_lowercase();
        }
        if let Some(code) = value.as_i64() {
            return match code {
                1 => "image".to_string(),
                _ => "video".to_string(),
            };
        }
    }

    if let Some(media_type) = payload.get("media_type").and_then(|value| value.as_str()) {
        return media_type.trim().to_lowercase();
    }

    "video".to_string()
}

fn infer_download_item_type(url: &str, fallback_type: &str) -> String {
    let lower_url = url.to_lowercase();

    if lower_url.ends_with(".mp3") || lower_url.ends_with(".m4a") {
        return "audio".to_string();
    }
    if lower_url.ends_with(".jpg")
        || lower_url.ends_with(".jpeg")
        || lower_url.ends_with(".png")
        || lower_url.ends_with(".webp")
        || lower_url.ends_with(".gif")
        || lower_url.contains("/image")
        || lower_url.contains("imagex")
    {
        return "image".to_string();
    }

    match fallback_type {
        "image" | "live_photo" | "video" | "audio" => fallback_type.to_string(),
        _ => "video".to_string(),
    }
}

fn parse_download_media_items(
    payload: &serde_json::Value,
    fallback_type: &str,
) -> Vec<DownloadMediaItem> {
    let mut items = Vec::new();

    if let Some(media_urls) = payload.get("media_urls").and_then(|value| value.as_array()) {
        for media in media_urls {
            let (media_type, url) = if let Some(url) = media.as_str() {
                (
                    infer_download_item_type(url.trim(), fallback_type),
                    url.trim().to_string(),
                )
            } else {
                let url = media
                    .get("url")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let media_type = media
                    .get("type")
                    .and_then(|value| value.as_str())
                    .map(|value| value.trim().to_lowercase())
                    .unwrap_or_else(|| infer_download_item_type(&url, fallback_type));
                (media_type, url)
            };

            if url.is_empty() || media_type == "audio" {
                continue;
            }

            items.push(DownloadMediaItem {
                r#type: media_type,
                url,
            });
        }
    }

    items
}

fn download_media_items_from_video(video: &VideoInfo) -> Vec<DownloadMediaItem> {
    let mut items = Vec::new();

    if let Some(urls) = &video.live_photo_urls {
        for url in urls {
            if !url.trim().is_empty() {
                items.push(DownloadMediaItem {
                    r#type: "live_photo".to_string(),
                    url: url.clone(),
                });
            }
        }
    }

    if let Some(urls) = &video.image_urls {
        for url in urls {
            if !url.trim().is_empty() {
                items.push(DownloadMediaItem {
                    r#type: "image".to_string(),
                    url: url.clone(),
                });
            }
        }
    }

    if items.is_empty() {
        if let Some(url) = DouyinClient::get_no_watermark_url(video) {
            items.push(DownloadMediaItem {
                r#type: "video".to_string(),
                url,
            });
        } else if !video.video.play_addr.trim().is_empty() {
            items.push(DownloadMediaItem {
                r#type: "video".to_string(),
                url: video.video.play_addr.clone(),
            });
        }
    }

    items
}

fn media_type_from_payload_or_items(raw_media_type: &str, items: &[DownloadMediaItem]) -> MediaType {
    if !raw_media_type.is_empty() {
        return match raw_media_type {
            "image" => MediaType::Image,
            "live_photo" => MediaType::LivePhoto,
            "mixed" => MediaType::Mixed,
            "audio" => MediaType::Audio,
            _ => MediaType::Video,
        };
    }

    let has_live = items.iter().any(|item| item.r#type == "live_photo");
    let has_image = items.iter().any(|item| item.r#type == "image");

    if has_live && has_image {
        MediaType::Mixed
    } else if has_live {
        MediaType::LivePhoto
    } else if has_image {
        MediaType::Image
    } else {
        MediaType::Video
    }
}

// ============================================================================
// Tauri Commands
// ============================================================================

/// 初始化客户端
#[tauri::command]
async fn init_client(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let config = state.config.lock().await.clone();

    let client = DouyinClient::new(config.clone()).map_err(|e| e.to_string())?;

    let (tx, mut rx) = mpsc::channel::<DownloaderEvent>(100);

    let downloader = Downloader::new(config, Some(tx)).map_err(|e| e.to_string())?;

    let app_handle = state.app_handle.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            let current_app_handle = app_handle.lock().await.clone();
            if let Some(app_handle) = current_app_handle {
                let _ = app_handle.emit(event.name, event.payload);
            }
        }
    });

    *state.client.lock().await = Some(client);
    *state.downloader.lock().await = Some(downloader);

    Ok(serde_json::json!({ "success": true }))
}

// ==================== 配置 API ====================

/// 获取配置
#[tauri::command]
fn get_config(state: State<'_, AppState>) -> serde_json::Value {
    let config = state.config.blocking_lock().clone();
    serde_json::to_value(&config).unwrap_or_else(|_| serde_json::json!({}))
}

/// 保存配置
#[tauri::command]
fn save_config(state: State<'_, AppState>, config: AppConfig) -> serde_json::Value {
    match config.save() {
        Ok(_) => {
            *state.config.blocking_lock() = config;
            serde_json::json!({ "success": true, "message": "配置保存成功" })
        }
        Err(e) => {
            serde_json::json!({ "success": false, "message": format!("保存失败: {}", e) })
        }
    }
}

/// 选择目录
#[tauri::command]
fn select_directory(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let folder = app.dialog().file().blocking_pick_folder();

    match folder {
        Some(path) => Ok(Some(path.to_string())),
        None => Ok(None),
    }
}

/// 验证 Cookie (简化版)
#[tauri::command]
#[allow(dead_code)]
async fn verify_cookie_simple(cookie: String) -> Result<bool, String> {
    // 简单检查 cookie 是否包含 sessionid
    Ok(cookie.contains("sessionid"))
}

#[tauri::command]
async fn open_verify_browser(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    target_url: Option<String>,
) -> Result<serde_json::Value, String> {
    let cookie = state.config.lock().await.cookie.clone();
    let requested_url = target_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("https://www.douyin.com/");
    let target_url =
        Url::parse(requested_url).map_err(|error| format!("URL 无效: {}", error))?;

    if let Some(window) = app.get_webview_window("verify-browser") {
        let _ = window.set_focus();
        let _ = window.show();
        return Ok(serde_json::json!({
            "success": true,
            "message": "验证窗口已打开，请完成验证"
        }));
    }

    let cookie_literal =
        serde_json::to_string(&cookie).map_err(|error| format!("Cookie 序列化失败: {}", error))?;
    let init_script = format!(
        r#"
        (() => {{
            const rawCookie = {cookie_literal};
            if (!rawCookie) return;
            if (window.sessionStorage && sessionStorage.getItem('__dy_verify_cookie_applied') === '1') return;
            rawCookie.split(';').map(item => item.trim()).filter(Boolean).forEach(item => {{
                try {{
                    document.cookie = `${{item}}; domain=.douyin.com; path=/`;
                }} catch (error) {{}}
            }});
            try {{
                if (window.sessionStorage) {{
                    sessionStorage.setItem('__dy_verify_cookie_applied', '1');
                    setTimeout(() => window.location.reload(), 120);
                }}
            }} catch (error) {{}}
        }})();
        "#
    );

    tauri::WebviewWindowBuilder::new(
        &app,
        "verify-browser",
        tauri::WebviewUrl::External(target_url),
    )
    .title("抖音验证")
    .inner_size(1100.0, 750.0)
    .resizable(true)
    .focused(true)
    .initialization_script(&init_script)
    .build()
    .map_err(|error| format!("无法打开验证窗口: {}", error))?;

    Ok(serde_json::json!({
        "success": true,
        "message": "验证窗口已打开，请完成验证"
    }))
}

#[tauri::command]
async fn cookie_browser_login(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    timeout: Option<u64>,
    browser: Option<String>,
) -> Result<serde_json::Value, String> {
    let _ = browser;
    let label = "cookie-browser-login".to_string();
    let login_url = Url::parse("https://www.douyin.com/").map_err(|error| error.to_string())?;

    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(serde_json::json!({
            "success": true,
            "message": "登录窗口已打开"
        }));
    }

    let cancelled = Arc::new(AtomicBool::new(false));
    *state.cookie_login.lock().await = Some(CookieLoginSession {
        label: label.clone(),
        cancelled: cancelled.clone(),
    });

    let window = tauri::WebviewWindowBuilder::new(
        &app,
        &label,
        tauri::WebviewUrl::External(login_url.clone()),
    )
    .title("登录抖音账号")
    .inner_size(1100.0, 820.0)
    .resizable(true)
    .focused(true)
    .build()
    .map_err(|error| format!("无法打开登录窗口: {}", error))?;

    let existing_cookie = state.config.lock().await.cookie.clone();
    for cookie in parse_cookie_string(&existing_cookie) {
        let _ = window.set_cookie(cookie);
    }
    let _ = window.navigate(login_url.clone());

    emit_cookie_login_status(
        &app,
        serde_json::json!({
            "event": "pending",
            "message": "请在弹出的窗口中完成登录"
        }),
    )
    .await;

    let config_state = state.config.clone();
    let client_state = state.client.clone();
    let downloader_state = state.downloader.clone();
    let cookie_login_state = state.cookie_login.clone();
    let login_timeout = timeout.unwrap_or(300);
    let label_clone = label.clone();

    tauri::async_runtime::spawn(async move {
        let started_at = std::time::Instant::now();
        let mut last_verify_attempt: Option<(String, std::time::Instant)> = None;

        loop {
            if cancelled.load(Ordering::SeqCst) {
                if let Some(window) = app.get_webview_window(&label_clone) {
                    let _ = window.close();
                }
                *cookie_login_state.lock().await = None;
                emit_cookie_login_status(
                    &app,
                    serde_json::json!({
                        "event": "cancelled",
                        "message": "已取消登录"
                    }),
                )
                .await;
                break;
            }

            if started_at.elapsed().as_secs() >= login_timeout {
                if let Some(window) = app.get_webview_window(&label_clone) {
                    let _ = window.close();
                }
                *cookie_login_state.lock().await = None;
                emit_cookie_login_status(
                    &app,
                    serde_json::json!({
                        "event": "timeout",
                        "message": "登录超时，请重试"
                    }),
                )
                .await;
                break;
            }

            let Some(window) = app.get_webview_window(&label_clone) else {
                *cookie_login_state.lock().await = None;
                emit_cookie_login_status(
                    &app,
                    serde_json::json!({
                        "event": "cancelled",
                        "message": "登录窗口已关闭"
                    }),
                )
                .await;
                break;
            };

            match window.cookies() {
                Ok(cookies) => {
                    let cookies: Vec<_> = cookies
                        .into_iter()
                        .filter(|cookie| {
                            let name = cookie.name();
                            cookie.domain().map(|domain| {
                                domain.contains("douyin.com")
                                    || domain.contains("amemv.com")
                                    || domain.contains("bytedance.com")
                                    || domain.contains("snssdk.com")
                            }).unwrap_or_else(|| {
                                matches!(
                                    name,
                                    "sessionid"
                                        | "sessionid_ss"
                                        | "sid_guard"
                                        | "uid_tt"
                                        | "passport_csrf_token"
                                        | "passport_auth_status"
                                        | "ttwid"
                                        | "msToken"
                                        | "s_v_web_id"
                                )
                            })
                        })
                        .collect();
                    let cookie_string = serialize_cookie_string(&cookies);
                    log::info!(
                        "cookie browser login poll: cookie_count={} names={}",
                        cookies.len(),
                        cookies
                            .iter()
                            .map(|cookie| cookie.name().to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    );

                    if has_douyin_login_cookie(&cookies) {
                        let should_verify = last_verify_attempt
                            .as_ref()
                            .map(|(last_cookie, last_at)| {
                                last_cookie != &cookie_string
                                    || last_at.elapsed() >= std::time::Duration::from_secs(5)
                            })
                            .unwrap_or(true);

                        if !should_verify {
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            continue;
                        }

                        last_verify_attempt =
                            Some((cookie_string.clone(), std::time::Instant::now()));

                        emit_cookie_login_status(
                            &app,
                            serde_json::json!({
                                "event": "pending",
                                "message": "已检测到登录 Cookie，正在校验登录状态"
                            }),
                        )
                        .await;

                        let base_config = config_state.lock().await.clone();
                        let current_user =
                            match verify_douyin_login_cookie(&base_config, &cookie_string).await {
                                Ok(user) => user,
                                Err(error) => {
                                    log::info!(
                                        "cookie browser login candidate rejected: {}",
                                        error
                                    );
                                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                    continue;
                                }
                            };

                        log::info!(
                            "cookie browser login success detected: cookie_count={} user_id={} nickname={}",
                            cookies.len(),
                            current_user.uid,
                            current_user.nickname
                        );
                        let mut next_config = config_state.lock().await.clone();
                        next_config.cookie = cookie_string.clone();
                        if let Err(error) = next_config.save() {
                            emit_cookie_login_status(
                                &app,
                                serde_json::json!({
                                    "event": "error",
                                    "message": format!("Cookie 保存失败: {}", error)
                                }),
                            )
                            .await;
                            break;
                        }

                        *config_state.lock().await = next_config.clone();
                        if let Ok(client) = DouyinClient::new(next_config.clone()) {
                            *client_state.lock().await = Some(client);
                        }
                        if let Some(downloader) = downloader_state.lock().await.as_mut() {
                            downloader.update_config(next_config);
                        }

                        let _ = window.close();
                        *cookie_login_state.lock().await = None;
                        emit_cookie_login_status(
                            &app,
                            serde_json::json!({
                                "event": "success",
                                "message": format!("Cookie 获取成功！已登录为 {}", current_user.nickname),
                                "cookie": cookie_string
                            }),
                        )
                        .await;
                        break;
                    }
                }
                Err(error) => {
                    log::warn!("failed to read login window cookies: {}", error);
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });

    Ok(serde_json::json!({
        "success": true,
        "message": "登录窗口已打开"
    }))
}

#[tauri::command]
async fn cancel_cookie_browser_login(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let session = state.cookie_login.lock().await.clone();
    if let Some(session) = session {
        session.cancelled.store(true, Ordering::SeqCst);
        if let Some(window) = app.get_webview_window(&session.label) {
            let _ = window.close();
        }
        Ok(serde_json::json!({
            "success": true,
            "message": "已取消"
        }))
    } else {
        Ok(serde_json::json!({
            "success": true,
            "message": "当前没有进行中的登录任务"
        }))
    }
}

// ==================== 视频/用户 API ====================

/// 解析视频链接
#[tauri::command]
async fn parse_url(state: State<'_, AppState>, url: String) -> Result<VideoInfo, String> {
    let client = get_client(&state).await?;

    // 提取视频 ID
    let aweme_id = DouyinClient::extract_aweme_id(&url)
        .ok_or_else(|| "Invalid URL or video ID".to_string())?;

    // 获取视频详情
    let video = client
        .get_video_detail(&aweme_id)
        .await
        .map_err(|e| e.to_string())?;

    Ok(video)
}

/// 解析分享链接 (处理重定向)
#[tauri::command]
async fn parse_link(state: State<'_, AppState>, link: String) -> Result<serde_json::Value, String> {
    let trimmed_link = link.trim().to_string();
    if trimmed_link.is_empty() {
        return Ok(serde_json::json!({
            "success": false,
            "message": "链接不能为空"
        }));
    }

    let client = match get_client(&state).await {
        Ok(client) => client,
        Err(_) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": "请先设置Cookie"
            }));
        }
    };

    let video = match client.parse_share_link(&trimmed_link).await {
        Ok(video) => video,
        Err(e) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": format!("解析链接失败: {}", e)
            }));
        }
    };

    let formatted_video = serde_json::json!({
        "author": {
            "nickname": video.author.nickname,
            "avatar_thumb": video.author.avatar_thumb,
            "sec_uid": video.author.sec_uid,
        },
        "aweme_id": video.aweme_id,
        "comment_count": video.statistics.comment_count,
        "cover_url": python_cover_url(&video),
        "create_time": video.create_time,
        "desc": video.desc,
        "digg_count": video.statistics.digg_count,
        "media_type": python_media_type(&video),
        "media_urls": python_media_urls(&video),
        "share_count": video.statistics.share_count
    });

    let mut response = serde_json::json!({
        "success": true,
        "type": "link_parse",
        "video": formatted_video.clone(),
        "videos": [formatted_video]
    });

    if !video.author.sec_uid.is_empty() {
        if let Ok(user_detail) = client.get_user_detail(&video.author.sec_uid).await {
            response["user"] = python_user_value(&user_detail.info);
        }
    }

    Ok(response)
}

/// 获取视频详情
#[tauri::command]
async fn get_video_detail(
    state: State<'_, AppState>,
    aweme_id: String,
) -> Result<serde_json::Value, String> {
    let aweme_id = aweme_id.trim().to_string();
    if aweme_id.is_empty() {
        return Ok(serde_json::json!({
            "success": false,
            "message": "视频ID不能为空"
        }));
    }

    let client = match get_client(&state).await {
        Ok(client) => client,
        Err(_) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": "请先设置Cookie"
            }));
        }
    };

    match client.get_video_detail(&aweme_id).await {
        Ok(video) => Ok(serde_json::json!({
            "success": true,
            "video": python_video_detail_value(&video)
        })),
        Err(e) => Ok(serde_json::json!({
            "success": false,
            "message": format!("获取视频详情失败: {}", e)
        })),
    }
}

/// 搜索用户
#[tauri::command]
async fn search_user(
    state: State<'_, AppState>,
    keyword: String,
) -> Result<serde_json::Value, String> {
    let keyword = keyword.trim().to_string();
    if keyword.is_empty() {
        return Ok(serde_json::json!({
            "success": false,
            "message": "请输入搜索关键词"
        }));
    }

    let client = match get_client(&state).await {
        Ok(client) => client,
        Err(_) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": "请先设置Cookie"
            }));
        }
    };

    match client.search_user(&keyword).await {
        Ok(api::SearchUserResult::NeedVerify { verify_url }) => Ok(serde_json::json!({
            "success": false,
            "need_verify": true,
            "verify_url": verify_url,
            "message": "需要完成滑块验证"
        })),
        Ok(api::SearchUserResult::NotFound) => Ok(serde_json::json!({
            "success": false,
            "message": "未找到用户"
        })),
        Ok(api::SearchUserResult::Single(user)) => Ok(serde_json::json!({
            "success": true,
            "type": "single",
            "user": python_user_value(&user)
        })),
        Ok(api::SearchUserResult::Multiple(users)) => Ok(serde_json::json!({
            "success": true,
            "type": "multiple",
            "users": users.iter().map(python_user_value).collect::<Vec<_>>()
        })),
        Err(e) => Ok(serde_json::json!({
            "success": false,
            "message": format!("搜索失败: {}", e)
        })),
    }
}

/// 获取用户详情
#[tauri::command]
async fn get_user_detail(
    state: State<'_, AppState>,
    sec_uid: String,
    nickname: Option<String>,
) -> Result<serde_json::Value, String> {
    let _ = nickname;
    let sec_uid = sec_uid.trim().to_string();
    if sec_uid.is_empty() {
        return Ok(serde_json::json!({
            "success": false,
            "message": "用户ID不能为空"
        }));
    }

    let client = match get_client(&state).await {
        Ok(client) => client,
        Err(_) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": "请先设置Cookie"
            }));
        }
    };

    match client.get_user_detail(&sec_uid).await {
        Ok(user_detail) => Ok(serde_json::json!({
            "success": true,
            "user": python_user_value(&user_detail.info)
        })),
        Err(e) => Ok(serde_json::json!({
            "success": false,
            "message": format!("获取用户详情失败: {}", e)
        })),
    }
}

/// 获取用户视频列表
#[tauri::command]
async fn get_user_videos(
    state: State<'_, AppState>,
    sec_uid: String,
    cursor: i64,
    count: u32,
) -> Result<serde_json::Value, String> {
    let sec_uid = sec_uid.trim().to_string();
    if sec_uid.is_empty() {
        return Ok(serde_json::json!({
            "success": false,
            "message": "用户ID不能为空"
        }));
    }

    let client = match get_client(&state).await {
        Ok(client) => client,
        Err(_) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": "请先设置Cookie"
            }));
        }
    };

    match client.get_user_videos(&sec_uid, cursor, count).await {
        Ok((videos, next_cursor, has_more)) => {
            let formatted = videos
                .iter()
                .map(|video| python_video_summary(video, true, true))
                .collect::<Vec<_>>();
            let total_count = formatted.len();

            Ok(serde_json::json!({
                "success": true,
                "videos": formatted,
                "has_more": has_more,
                "cursor": next_cursor,
                "total_count": total_count
            }))
        }
        Err(e) => Ok(serde_json::json!({
            "success": false,
            "message": format!("获取用户视频列表失败: {}", e)
        })),
    }
}

/// 获取点赞视频列表
#[tauri::command]
async fn get_liked_videos(
    state: State<'_, AppState>,
    sec_uid: String,
    cursor: i64,
    count: u32,
) -> Result<serde_json::Value, String> {
    let client = match get_client(&state).await {
        Ok(client) => client,
        Err(_) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": "请先设置Cookie"
            }));
        }
    };

    match client
        .get_liked_videos_python_style(&sec_uid, cursor, count)
        .await
    {
        Ok(videos) if !videos.is_empty() => {
            let count = videos.len();
            Ok(serde_json::json!({
                "success": true,
                "data": videos,
                "count": count
            }))
        }
        Ok(_) => Ok(serde_json::json!({
            "success": false,
            "message": "获取点赞视频失败。该接口需要登录态，请确认Cookie有效且包含完整的登录信息。如果Cookie已过期请重新获取。"
        })),
        Err(e) => Ok(serde_json::json!({
            "success": false,
            "message": format!("获取点赞视频失败: {}", e)
        })),
    }
}

/// 获取点赞作者列表
#[tauri::command]
async fn get_liked_authors(
    state: State<'_, AppState>,
    count: u32,
) -> Result<serde_json::Value, String> {
    let client = match get_client(&state).await {
        Ok(client) => client,
        Err(_) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": "请先设置Cookie"
            }));
        }
    };

    let liked_videos = match client.get_liked_videos_python_style("", 0, count).await {
        Ok(videos) => videos,
        Err(e) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": format!("获取点赞作者失败: {}", e)
            }));
        }
    };

    if liked_videos.is_empty() {
        return Ok(serde_json::json!({
            "success": false,
            "message": "获取点赞作者失败。该接口需要登录态，请确认Cookie有效且包含完整的登录信息。"
        }));
    }

    let mut seen = HashSet::new();
    let mut authors = Vec::new();

    for video in liked_videos {
        let sec_uid = video.author.sec_uid.trim().to_string();
        if sec_uid.is_empty() || !seen.insert(sec_uid.clone()) {
            continue;
        }

        if let Ok(detail) = client.get_user_detail(&sec_uid).await {
            authors.push(python_user_value(&detail.info));
        } else {
            authors.push(serde_json::json!({
                "nickname": video.author.nickname,
                "unique_id": "",
                "follower_count": 0,
                "following_count": 0,
                "total_favorited": 0,
                "aweme_count": 0,
                "signature": "",
                "sec_uid": sec_uid,
                "avatar_thumb": video.author.avatar_thumb,
            }));
        }
    }

    if authors.is_empty() {
        return Ok(serde_json::json!({
            "success": false,
            "message": "获取点赞作者失败。该接口需要登录态，请确认Cookie有效且包含完整的登录信息。"
        }));
    }

    let count = authors.len();
    Ok(serde_json::json!({
        "success": true,
        "data": authors,
        "count": count
    }))
}

/// 获取推荐视频
#[tauri::command]
async fn get_recommended(
    state: State<'_, AppState>,
    cursor: i64,
    count: u32,
) -> Result<serde_json::Value, String> {
    let client = match get_client(&state).await {
        Ok(client) => client,
        Err(_) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": "请先登录抖音账号"
            }));
        }
    };

    log::info!("get_recommended invoked: cursor={} count={}", cursor, count);

    let (videos, next_cursor, has_more) = match client.get_recommended_feed(cursor, count).await {
        Ok(result) => result,
        Err(e) => {
            log::error!(
                "get_recommended failed: cursor={} count={} error={}",
                cursor,
                count,
                e
            );
            return Ok(serde_json::json!({
                "success": false,
                "message": "获取推荐视频失败，请稍后重试"
            }));
        }
    };

    log::info!(
        "get_recommended completed: cursor={} count={} next_cursor={} has_more={} videos={}",
        cursor,
        count,
        next_cursor,
        has_more,
        videos.len()
    );

    let formatted = videos
        .iter()
        .map(python_recommended_video)
        .collect::<Vec<_>>();
    let count = formatted.len();

    Ok(serde_json::json!({
        "success": true,
        "videos": formatted,
        "cursor": next_cursor,
        "has_more": has_more,
        "count": count
    }))
}

/// 获取评论列表
#[tauri::command]
async fn get_comments(
    state: State<'_, AppState>,
    aweme_id: String,
    cursor: i64,
    count: u32,
) -> Result<serde_json::Value, String> {
    let client = get_client(&state).await?;

    let (comments, next_cursor, has_more) = client
        .get_comments(&aweme_id, cursor, count)
        .await
        .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "comments": comments,
        "cursor": next_cursor,
        "has_more": has_more
    }))
}

// ==================== Cookie API ====================

/// 验证 Cookie
#[tauri::command]
async fn verify_cookie(state: State<'_, AppState>) -> Result<CookieStatus, String> {
    let client = get_client(&state).await?;

    client.verify_cookie().await.map_err(|e| e.to_string())
}

/// 获取当前用户信息
#[tauri::command]
async fn get_current_user(state: State<'_, AppState>) -> Result<UserInfo, String> {
    let client = get_client(&state).await?;

    client.get_current_user().await.map_err(|e| e.to_string())
}

// ==================== 下载 API ====================

/// 下载单个视频
#[tauri::command]
async fn download_video(
    state: State<'_, AppState>,
    video: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let raw_media_type = download_media_type_from_payload(&video);
    let aweme_id = video
        .get("aweme_id")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let mut desc = video
        .get("desc")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let mut author_name = video
        .get("author_name")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let mut cover = video
        .get("cover_url")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let mut media_urls = parse_download_media_items(&video, &raw_media_type);
    let mut media_type = media_type_from_payload_or_items(&raw_media_type, &media_urls);

    if (media_urls.is_empty() || desc.is_empty() || author_name.is_empty()) && !aweme_id.is_empty() {
        if let Ok(client) = get_client(&state).await {
            if let Ok(fresh_video) = client.get_video_detail(&aweme_id).await {
                if desc.is_empty() {
                    desc = fresh_video.desc.clone();
                }
                if author_name.is_empty() {
                    author_name = fresh_video.author.nickname.clone();
                }
                if cover.is_empty() {
                    cover = fresh_video.video.cover.clone();
                }
                if media_urls.is_empty() {
                    media_urls = download_media_items_from_video(&fresh_video);
                    media_type = fresh_video.media_type.clone();
                }
            }
        }
    }

    if media_urls.is_empty() {
        return Ok(serde_json::json!({
            "success": false,
            "message": "没有可用的媒体URL"
        }));
    }

    if desc.is_empty() {
        desc = format!("作品_{}", aweme_id);
    }
    if author_name.is_empty() {
        author_name = "未知作者".to_string();
    }

    let downloader_guard = state.downloader.lock().await;
    let downloader = match downloader_guard.as_ref() {
        Some(downloader) => downloader,
        None => {
            return Ok(serde_json::json!({
                "success": false,
                "message": "服务未完全初始化"
            }));
        }
    };

    let task_id = match downloader
        .add_media_task(
            aweme_id.clone(),
            desc.clone(),
            author_name.clone(),
            String::new(),
            cover,
            media_type,
            media_urls,
            None,
        )
        .await
    {
        Ok(task_id) => task_id,
        Err(e) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": format!("下载启动失败: {}", e)
            }));
        }
    };

    if let Err(e) = downloader.start_download(&task_id).await {
        return Ok(serde_json::json!({
            "success": false,
            "message": format!("下载启动失败: {}", e)
        }));
    }

    Ok(serde_json::json!({
        "success": true,
        "task_id": task_id,
        "message": "下载任务已启动"
    }))
}

/// 批量下载用户视频（边获取边下载）
#[tauri::command]
async fn download_user_videos(
    state: State<'_, AppState>,
    sec_uid: String,
    nickname: String,
    aweme_count: i64,
) -> Result<serde_json::Value, String> {
    let client = match get_client(&state).await {
        Ok(client) => client,
        Err(_) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": "请先设置Cookie"
            }));
        }
    };

    let batch_task_id = uuid::Uuid::new_v4().to_string();
    let total_videos = aweme_count.max(0) as usize;

    // 获取下载器
    let downloader = {
        let downloader_guard = state.downloader.lock().await;
        match downloader_guard.as_ref() {
            Some(d) => d.clone(),
            None => {
                return Ok(serde_json::json!({
                    "success": false,
                    "message": "服务未完全初始化"
                }));
            }
        }
    };

    // 克隆需要的变量
    let batch_id = batch_task_id.clone();
    let nickname_clone = nickname.clone();
    let sec_uid_clone = sec_uid.clone();
    let client_clone = client.clone();
    let downloader_clone = downloader.clone();

    // 发送开始事件
    downloader.emit_batch_started(&batch_id, &nickname, total_videos).await;

    // 在后台任务中执行边获取边下载
    tokio::spawn(async move {
        if let Err(e) = downloader_clone.start_streaming_download(
            client_clone,
            sec_uid_clone,
            batch_id,
            nickname_clone,
            total_videos,
        ).await {
            log::error!("Streaming download error: {}", e);
        }
    });

    Ok(serde_json::json!({
        "success": true,
        "task_id": batch_task_id,
        "message": format!("开始下载 {} 个视频", total_videos),
        "nickname": nickname,
        "total_videos": total_videos
    }))
}

/// 下载点赞视频
#[tauri::command]
async fn download_liked_videos(
    state: State<'_, AppState>,
    count: u32,
) -> Result<serde_json::Value, String> {
    let client = match get_client(&state).await {
        Ok(client) => client,
        Err(_) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": "请先设置Cookie"
            }));
        }
    };

    // 获取点赞视频
    let (videos, _, _) = match client.get_liked_videos("", 0, count).await {
        Ok(result) => result,
        Err(e) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": format!("获取视频列表失败: {}", e)
            }));
        }
    };

    if videos.is_empty() {
        return Ok(serde_json::json!({
            "success": false,
            "message": "没有找到点赞视频"
        }));
    }

    let batch_task_id = uuid::Uuid::new_v4().to_string();
    let total_videos = videos.len();
    let batch_task_id_clone = batch_task_id.clone();

    // 获取下载器并启动批量下载
    {
        let downloader_guard = state.downloader.lock().await;
        let downloader = match downloader_guard.as_ref() {
            Some(d) => d,
            None => {
                return Ok(serde_json::json!({
                    "success": false,
                    "message": "服务未完全初始化"
                }));
            }
        };

        let downloader_clone = downloader.clone();
        let videos_clone = videos.clone();

        // 在后台任务中执行批量下载
        tokio::spawn(async move {
            if let Err(e) = downloader_clone.start_batch_download(videos_clone, batch_task_id_clone, "点赞视频".to_string()).await {
                log::error!("Batch download error: {}", e);
            }
        });
    }

    Ok(serde_json::json!({
        "success": true,
        "task_id": batch_task_id,
        "message": format!("开始下载 {} 个点赞视频", total_videos),
        "total_videos": total_videos
    }))
}

/// 下载点赞作者作品
#[tauri::command]
async fn download_liked_authors(
    state: State<'_, AppState>,
    count: Option<u32>,
) -> Result<serde_json::Value, String> {
    let client = match get_client(&state).await {
        Ok(client) => client,
        Err(_) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": "请先设置Cookie"
            }));
        }
    };

    let downloader_guard = state.downloader.lock().await;
    let downloader = match downloader_guard.as_ref() {
        Some(downloader) => downloader,
        None => {
            return Ok(serde_json::json!({
                "success": false,
                "message": "服务未完全初始化"
            }));
        }
    };

    let liked_videos = match client
        .get_liked_videos_python_style("", 0, count.unwrap_or(20))
        .await
    {
        Ok(videos) => videos,
        Err(e) => {
            return Ok(serde_json::json!({
                "success": false,
                "message": format!("下载失败: {}", e)
            }));
        }
    };

    let mut seen = HashSet::new();
    let mut task_ids = Vec::new();

    for video in liked_videos {
        let sec_uid = video.author.sec_uid.trim().to_string();
        if sec_uid.is_empty() || !seen.insert(sec_uid.clone()) {
            continue;
        }

        if let Ok((videos, _, _)) = client.get_user_videos(&sec_uid, 0, 100).await {
            for user_video in &videos {
                if let Ok(task_id) = downloader.add_task(user_video, None).await {
                    let _ = downloader.start_download(&task_id).await;
                    task_ids.push(task_id);
                }
            }
        }
    }
    let count = task_ids.len();

    Ok(serde_json::json!({
        "success": true,
        "task_id": uuid::Uuid::new_v4().to_string(),
        "task_ids": task_ids,
        "message": "点赞作者作品下载任务已开始",
        "count": count
    }))
}

/// 添加下载任务
#[tauri::command]
async fn add_download_task(
    state: State<'_, AppState>,
    video: VideoInfo,
    save_path: Option<String>,
) -> Result<String, String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard
        .as_ref()
        .ok_or("Downloader not initialized")?;

    let path = save_path.map(std::path::PathBuf::from);
    downloader
        .add_task(&video, path)
        .await
        .map_err(|e| e.to_string())
}

/// 开始下载
#[tauri::command]
async fn start_download(state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard
        .as_ref()
        .ok_or("Downloader not initialized")?;

    downloader
        .start_download(&task_id)
        .await
        .map_err(|e| e.to_string())
}

/// 获取下载任务列表
#[tauri::command]
async fn get_download_tasks(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard
        .as_ref()
        .ok_or("Downloader not initialized")?;

    Ok(serde_json::json!({
        "success": true,
        "tasks": downloader.get_tasks().await
    }))
}

/// 取消下载任务
#[tauri::command]
async fn cancel_download_task(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<serde_json::Value, String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard
        .as_ref()
        .ok_or("Downloader not initialized")?;

    match downloader.cancel_task(&task_id).await {
        Ok(_) => Ok(serde_json::json!({
            "success": true,
            "message": "任务已取消"
        })),
        Err(e) => Ok(serde_json::json!({
            "success": false,
            "message": format!("取消任务失败: {}", e)
        })),
    }
}

/// 删除下载任务
#[tauri::command]
async fn remove_download_task(state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard
        .as_ref()
        .ok_or("Downloader not initialized")?;

    downloader
        .remove_task(&task_id)
        .await
        .map_err(|e| e.to_string())
}

/// 暂停下载
#[tauri::command]
async fn pause_download(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<serde_json::Value, String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard
        .as_ref()
        .ok_or("Downloader not initialized")?;

    match downloader.pause_task(&task_id).await {
        Ok(_) => Ok(serde_json::json!({
            "success": true,
            "message": "任务已暂停"
        })),
        Err(e) => Ok(serde_json::json!({
            "success": false,
            "message": format!("暂停任务失败: {}", e)
        })),
    }
}

/// 恢复下载
#[tauri::command]
async fn resume_download(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<serde_json::Value, String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard
        .as_ref()
        .ok_or("Downloader not initialized")?;

    match downloader.resume_task(&task_id).await {
        Ok(_) => Ok(serde_json::json!({
            "success": true,
            "message": "任务已恢复"
        })),
        Err(e) => Ok(serde_json::json!({
            "success": false,
            "message": format!("恢复任务失败: {}", e)
        })),
    }
}

// ==================== 下载历史 API ====================

/// 获取下载历史
#[tauri::command]
async fn get_history(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let history = state.history.lock().await;
    let items = history.get_all();
    Ok(serde_json::json!({
        "success": true,
        "items": items
    }))
}

/// 清空下载历史
#[tauri::command]
async fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    let mut history = state.history.lock().await;
    history.clear().map_err(|e| e.to_string())
}

/// 删除历史记录
#[tauri::command]
async fn delete_history(state: State<'_, AppState>, aweme_id: String) -> Result<(), String> {
    let mut history = state.history.lock().await;
    history.delete(&aweme_id).map_err(|e| e.to_string())
}

/// 添加历史记录
#[tauri::command]
async fn add_history(
    state: State<'_, AppState>,
    aweme_id: String,
    title: String,
    author: String,
    author_id: String,
    cover: String,
    file_path: String,
    media_type: String,
    file_size: u64,
) -> Result<(), String> {
    let mut history = state.history.lock().await;
    history
        .add(DownloadHistory {
            aweme_id,
            title,
            author,
            author_id,
            cover,
            file_path,
            media_type,
            file_size,
            create_time: chrono::Utc::now().timestamp(),
        })
        .map_err(|e| e.to_string())
}

// ==================== 文件操作 API ====================

/// 打开文件所在目录
#[tauri::command]
async fn open_file_location(path: String) -> Result<(), String> {
    use std::process::Command;

    #[cfg(target_os = "macos")]
    Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    Command::new("explorer")
        .args(["/select,", &path])
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "linux")]
    Command::new("xdg-open")
        .arg(
            std::path::Path::new(&path)
                .parent()
                .unwrap_or(std::path::Path::new(".")),
        )
        .spawn()
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// 删除文件
#[tauri::command]
async fn delete_file(path: String) -> Result<(), String> {
    std::fs::remove_file(&path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{download_media_type_from_payload, parse_download_media_items};

    #[test]
    fn parses_flat_download_media_items() {
        let payload = serde_json::json!({
            "aweme_id": "123",
            "desc": "test",
            "raw_media_type": "video",
            "media_type": "video",
            "media_urls": [{ "type": "video", "url": "https://example.com/test.mp4" }],
        });

        let parsed = parse_download_media_items(&payload, "video");

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].r#type, "video");
        assert_eq!(parsed[0].url, "https://example.com/test.mp4");
    }

    #[test]
    fn resolves_download_media_type_from_string_and_numeric_payloads() {
        assert_eq!(
            download_media_type_from_payload(&serde_json::json!({ "raw_media_type": "live_photo" })),
            "live_photo"
        );
        assert_eq!(
            download_media_type_from_payload(&serde_json::json!({ "raw_media_type": 1 })),
            "image"
        );
        assert_eq!(
            download_media_type_from_payload(&serde_json::json!({ "media_type": "mixed" })),
            "mixed"
        );
    }
}

// ============================================================================
// 应用入口
// ============================================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let log_level = if cfg!(debug_assertions) {
                log::LevelFilter::Info
            } else {
                log::LevelFilter::Warn
            };
            app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(log_level)
                    .build(),
            )?;

            // 初始化应用状态
            let state = AppState::new();
            *state.app_handle.blocking_lock() = Some(app.handle().clone());
            tauri::async_runtime::spawn({
                let state = state.clone();
                async move {
                    if let Err(error) = media_proxy::spawn_media_proxy(state).await {
                        log::error!("failed to start media proxy: {}", error);
                    }
                }
            });
            app.manage(state);

            // 开发模式下打开开发者工具
            #[cfg(debug_assertions)]
            {
                use tauri::Manager;
                if let Some(window) = app.get_webview_window("main") {
                    window.open_devtools();
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 配置
            init_client,
            get_config,
            save_config,
            select_directory,
            // 视频/用户
            parse_url,
            parse_link,
            get_video_detail,
            search_user,
            get_user_detail,
            get_user_videos,
            get_liked_videos,
            get_liked_authors,
            get_recommended,
            get_comments,
            // Cookie
            verify_cookie,
            get_current_user,
            open_verify_browser,
            cookie_browser_login,
            cancel_cookie_browser_login,
            // 下载
            download_video,
            download_user_videos,
            download_liked_videos,
            download_liked_authors,
            add_download_task,
            start_download,
            get_download_tasks,
            cancel_download_task,
            remove_download_task,
            pause_download,
            resume_download,
            // 历史
            get_history,
            clear_history,
            delete_history,
            add_history,
            // 文件操作
            open_file_location,
            delete_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
