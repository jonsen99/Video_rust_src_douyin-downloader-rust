//! 抖音视频下载器 - Tauri 应用

pub mod api;
pub mod config;
pub mod downloader;
pub mod media_proxy;
pub mod sign;
pub mod history;

use api::{DouyinClient, VideoInfo, UserInfo, UserDetail, CookieStatus, DownloadHistory, GenericResponse};
use config::AppConfig;
use downloader::Downloader;
use history::HistoryManager;
use std::sync::Arc;
use tauri::{Manager, State};
use tokio::sync::{mpsc, Mutex};

/// 应用状态
#[derive(Clone)]
pub struct AppState {
    pub(crate) config: Arc<Mutex<AppConfig>>,
    pub(crate) client: Arc<Mutex<Option<DouyinClient>>>,
    pub(crate) downloader: Arc<Mutex<Option<Downloader>>>,
    pub(crate) history: Arc<Mutex<HistoryManager>>,
}

impl AppState {
    pub fn new() -> Self {
        let config = AppConfig::load();
        let history = HistoryManager::load();
        Self {
            config: Arc::new(Mutex::new(config)),
            client: Arc::new(Mutex::new(None)),
            downloader: Arc::new(Mutex::new(None)),
            history: Arc::new(Mutex::new(history)),
        }
    }
}

async fn get_client(state: &State<'_, AppState>) -> Result<DouyinClient, String> {
    state.client.lock().await.clone().ok_or_else(|| "Client not initialized".to_string())
}

// ============================================================================
// Tauri Commands
// ============================================================================

/// 初始化客户端
#[tauri::command]
async fn init_client(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let config = state.config.lock().await.clone();

    let client = DouyinClient::new(config.clone()).map_err(|e| e.to_string())?;

    let (tx, _rx) = mpsc::channel(100);

    let downloader = Downloader::new(config, Some(tx)).map_err(|e| e.to_string())?;

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

    let folder = app.dialog()
        .file()
        .blocking_pick_folder();

    match folder {
        Some(path) => Ok(Some(path.to_string())),
        None => Ok(None),
    }
}

/// 验证 Cookie (简化版)
#[tauri::command]
async fn verify_cookie_simple(cookie: String) -> Result<bool, String> {
    // 简单检查 cookie 是否包含 sessionid
    Ok(cookie.contains("sessionid"))
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
    let video = client.get_video_detail(&aweme_id).await.map_err(|e| e.to_string())?;

    Ok(video)
}

/// 解析分享链接 (处理重定向)
#[tauri::command]
async fn parse_link(state: State<'_, AppState>, link: String) -> Result<VideoInfo, String> {
    let client = get_client(&state).await?;

    client.parse_share_link(&link).await.map_err(|e| e.to_string())
}

/// 获取视频详情
#[tauri::command]
async fn get_video_detail(state: State<'_, AppState>, aweme_id: String) -> Result<VideoInfo, String> {
    let client = get_client(&state).await?;

    client.get_video_detail(&aweme_id).await.map_err(|e| e.to_string())
}

/// 搜索用户
#[tauri::command]
async fn search_user(state: State<'_, AppState>, keyword: String) -> Result<Vec<UserInfo>, String> {
    let client = get_client(&state).await?;

    client.search_user(&keyword).await.map_err(|e| e.to_string())
}

/// 获取用户详情
#[tauri::command]
async fn get_user_detail(state: State<'_, AppState>, sec_uid: String, nickname: Option<String>) -> Result<UserDetail, String> {
    let client = get_client(&state).await?;

    client.get_user_detail(&sec_uid).await.map_err(|e| e.to_string())
}

/// 获取用户视频列表
#[tauri::command]
async fn get_user_videos(
    state: State<'_, AppState>,
    sec_uid: String,
    cursor: i64,
    count: u32,
) -> Result<serde_json::Value, String> {
    let client = get_client(&state).await?;

    let (videos, next_cursor, has_more) = client
        .get_user_videos(&sec_uid, cursor, count)
        .await
        .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "videos": videos,
        "cursor": next_cursor,
        "has_more": has_more
    }))
}

/// 获取点赞视频列表
#[tauri::command]
async fn get_liked_videos(
    state: State<'_, AppState>,
    sec_uid: String,
    cursor: i64,
    count: u32,
) -> Result<serde_json::Value, String> {
    let client = get_client(&state).await?;

    let (videos, next_cursor, has_more) = client
        .get_liked_videos(&sec_uid, cursor, count)
        .await
        .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "videos": videos,
        "cursor": next_cursor,
        "has_more": has_more
    }))
}

/// 获取推荐视频
#[tauri::command]
async fn get_recommended(
    state: State<'_, AppState>,
    cursor: i64,
    count: u32,
) -> Result<serde_json::Value, String> {
    let client = get_client(&state).await?;

    log::info!("get_recommended invoked: cursor={} count={}", cursor, count);

    let (videos, next_cursor, has_more) = client
        .get_recommended_feed(cursor, count)
        .await
        .map_err(|e| {
            log::error!("get_recommended failed: cursor={} count={} error={}", cursor, count, e);
            e.to_string()
        })?;

    log::info!(
        "get_recommended completed: cursor={} count={} next_cursor={} has_more={} videos={}",
        cursor,
        count,
        next_cursor,
        has_more,
        videos.len()
    );

    Ok(serde_json::json!({
        "videos": videos,
        "cursor": next_cursor,
        "has_more": has_more
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
) -> Result<GenericResponse, String> {
    // 解析视频信息
    let video_info: VideoInfo = serde_json::from_value(video).map_err(|e| e.to_string())?;

    let desc = video_info.desc.clone();

    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard.as_ref().ok_or("Downloader not initialized")?;

    downloader.add_task(&video_info, None).await.map_err(|e| e.to_string())?;

    Ok(GenericResponse::ok(&format!("已添加下载任务: {}", desc)))
}

/// 批量下载用户视频
#[tauri::command]
async fn download_user_videos(
    state: State<'_, AppState>,
    sec_uid: String,
    nickname: String,
    aweme_count: i64,
) -> Result<GenericResponse, String> {
    let client = get_client(&state).await?;

    // 获取用户视频列表
    let count = if aweme_count > 0 && aweme_count < 100 { aweme_count as u32 } else { 100 };
    let (videos, _, _) = client.get_user_videos(&sec_uid, 0, count)
        .await
        .map_err(|e| e.to_string())?;

    // TODO: 添加批量下载任务

    Ok(GenericResponse::ok(&format!("开始下载 {} 的 {} 个视频", nickname, videos.len())))
}

/// 下载点赞视频
#[tauri::command]
async fn download_liked_videos(
    state: State<'_, AppState>,
    count: u32,
) -> Result<GenericResponse, String> {
    let client = get_client(&state).await?;

    // 获取点赞视频
    let (videos, _, _) = client.get_liked_videos("", 0, count)
        .await
        .map_err(|e| e.to_string())?;

    Ok(GenericResponse::ok(&format!("获取到 {} 个点赞视频", videos.len())))
}

/// 添加下载任务
#[tauri::command]
async fn add_download_task(
    state: State<'_, AppState>,
    video: VideoInfo,
    save_path: Option<String>,
) -> Result<String, String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard.as_ref().ok_or("Downloader not initialized")?;

    let path = save_path.map(std::path::PathBuf::from);
    downloader.add_task(&video, path).await.map_err(|e| e.to_string())
}

/// 开始下载
#[tauri::command]
async fn start_download(state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard.as_ref().ok_or("Downloader not initialized")?;

    downloader.start_download(&task_id).await.map_err(|e| e.to_string())
}

/// 获取下载任务列表
#[tauri::command]
async fn get_download_tasks(state: State<'_, AppState>) -> Result<Vec<api::DownloadTask>, String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard.as_ref().ok_or("Downloader not initialized")?;

    Ok(downloader.get_tasks().await)
}

/// 取消下载任务
#[tauri::command]
async fn cancel_download_task(state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard.as_ref().ok_or("Downloader not initialized")?;

    downloader.cancel_task(&task_id).await.map_err(|e| e.to_string())
}

/// 删除下载任务
#[tauri::command]
async fn remove_download_task(state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard.as_ref().ok_or("Downloader not initialized")?;

    downloader.remove_task(&task_id).await.map_err(|e| e.to_string())
}

/// 暂停下载
#[tauri::command]
async fn pause_download(state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard.as_ref().ok_or("Downloader not initialized")?;

    downloader.pause_task(&task_id).await.map_err(|e| e.to_string())
}

/// 恢复下载
#[tauri::command]
async fn resume_download(state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    let downloader_guard = state.downloader.lock().await;
    let downloader = downloader_guard.as_ref().ok_or("Downloader not initialized")?;

    downloader.resume_task(&task_id).await.map_err(|e| e.to_string())
}

// ==================== 下载历史 API ====================

/// 获取下载历史
#[tauri::command]
async fn get_history(state: State<'_, AppState>) -> Result<Vec<DownloadHistory>, String> {
    let history = state.history.lock().await;
    Ok(history.get_all())
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
    history.add(DownloadHistory {
        aweme_id,
        title,
        author,
        author_id,
        cover,
        file_path,
        media_type,
        file_size,
        create_time: chrono::Utc::now().timestamp(),
    }).map_err(|e| e.to_string())
}

// ==================== 文件操作 API ====================

/// 打开文件所在目录
#[tauri::command]
async fn open_file_location(path: String) -> Result<(), String> {
    use std::process::Command;

    #[cfg(target_os = "macos")]
    Command::new("open").arg("-R").arg(&path).spawn().map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    Command::new("explorer").args(["/select,", &path]).spawn().map_err(|e| e.to_string())?;

    #[cfg(target_os = "linux")]
    Command::new("xdg-open").arg(std::path::Path::new(&path).parent().unwrap_or(std::path::Path::new("."))).spawn().map_err(|e| e.to_string())?;

    Ok(())
}

/// 删除文件
#[tauri::command]
async fn delete_file(path: String) -> Result<(), String> {
    std::fs::remove_file(&path).map_err(|e| e.to_string())
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
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // 初始化应用状态
            let state = AppState::new();
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
            get_recommended,
            get_comments,
            // Cookie
            verify_cookie,
            get_current_user,
            // 下载
            download_video,
            download_user_videos,
            download_liked_videos,
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
