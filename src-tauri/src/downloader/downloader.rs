//! 下载器实现

use crate::api::types::{DownloadStatus, DownloadTask, MediaType, VideoInfo};
use crate::api::DouyinClient;
use crate::config::AppConfig;
use crate::history::HistoryManager;
use anyhow::{anyhow, Result};
use chrono::Local;
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, Mutex};

/// 下载进度消息
#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadProgress {
    pub task_id: String,
    pub status: DownloadStatus,
    pub progress: f32,
    pub downloaded_size: u64,
    pub total_size: u64,
    pub speed: u64,
    pub error_msg: Option<String>,
}

/// 下载器
pub struct Downloader {
    client: reqwest::Client,
    config: AppConfig,
    tasks: Arc<Mutex<Vec<DownloadTask>>>,
    progress_tx: Option<mpsc::Sender<DownloadProgress>>,
    cancel_tokens: Arc<Mutex<std::collections::HashMap<String, bool>>>,
    history: Arc<Mutex<HistoryManager>>,
}

impl Downloader {
    pub fn new(config: AppConfig, progress_tx: Option<mpsc::Sender<DownloadProgress>>) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .danger_accept_invalid_certs(false);

        if let Some(proxy) = &config.proxy {
            if !proxy.is_empty() {
                builder = builder.proxy(reqwest::Proxy::all(proxy)?);
            }
        }

        let client = builder.build()?;

        Ok(Self {
            client,
            config,
            tasks: Arc::new(Mutex::new(Vec::new())),
            progress_tx,
            cancel_tokens: Arc::new(Mutex::new(std::collections::HashMap::new())),
            history: Arc::new(Mutex::new(HistoryManager::load())),
        })
    }

    /// 添加下载任务
    pub async fn add_task(&self, video: &VideoInfo, save_path: Option<PathBuf>) -> Result<String> {
        let task_id = uuid::Uuid::new_v4().to_string();

        // 确定下载 URL
        let url = if video.is_image {
            video
                .image_urls
                .as_ref()
                .and_then(|urls| urls.first().cloned())
                .ok_or_else(|| anyhow!("No image URLs"))?
        } else {
            DouyinClient::get_no_watermark_url(video)
                .ok_or_else(|| anyhow!("No video URL"))?
        };

        // 确定保存路径
        let base_path = save_path.unwrap_or_else(|| PathBuf::from(&self.config.download_path));
        let filename = self.generate_filename(video);

        // 创建作者目录
        let author_dir = base_path.join(sanitize_filename(&video.author.nickname));
        let file_path = author_dir.join(&filename);

        // 创建任务
        let task = DownloadTask {
            id: task_id.clone(),
            aweme_id: video.aweme_id.clone(),
            url: url.clone(),
            title: video.desc.clone(),
            author: video.author.nickname.clone(),
            author_id: video.author.uid.clone(),
            cover: video.video.cover.clone(),
            save_path: file_path.to_string_lossy().to_string(),
            filename: filename.clone(),
            media_type: if video.is_image {
                MediaType::Image
            } else {
                MediaType::Video
            },
            status: DownloadStatus::Pending,
            progress: 0.0,
            total_size: 0,
            downloaded_size: 0,
            error_msg: None,
            create_time: Local::now().timestamp(),
            complete_time: None,
            image_urls: video.image_urls.clone(),
        };

        self.tasks.lock().await.push(task);
        Ok(task_id)
    }

    /// 生成文件名
    fn generate_filename(&self, video: &VideoInfo) -> String {
        let ext = if video.is_image { "jpg" } else { "mp4" };
        let author = sanitize_filename(&video.author.nickname);
        let title = sanitize_filename(&video.desc);

        let title = if title.len() > 50 {
            format!("{}...", &title[..47])
        } else if title.is_empty() {
            video.aweme_id.clone()
        } else {
            title
        };

        format!("{}_{}.{}", author, title, ext)
    }

    /// 开始下载
    pub async fn start_download(&self, task_id: &str) -> Result<()> {
        let tasks = self.tasks.clone();
        let progress_tx = self.progress_tx.clone();
        let client = self.client.clone();
        let history = self.history.clone();
        let task_id_owned = task_id.to_string();
        let cancel_tokens = self.cancel_tokens.clone();

        // 设置取消标志
        cancel_tokens.lock().await.insert(task_id_owned.clone(), false);

        // 更新状态为下载中
        {
            let mut tasks_lock = tasks.lock().await;
            if let Some(task) = tasks_lock.iter_mut().find(|t| t.id == task_id) {
                task.status = DownloadStatus::Downloading;
            }
        }

        tokio::spawn(async move {
            if let Err(e) = Self::download_file(
                client,
                tasks.clone(),
                task_id_owned.clone(),
                progress_tx,
                history,
                cancel_tokens,
            ).await {
                log::error!("Download error: {}", e);

                let mut tasks_lock = tasks.lock().await;
                if let Some(task) = tasks_lock.iter_mut().find(|t| t.id == task_id_owned) {
                    task.status = DownloadStatus::Failed;
                    task.error_msg = Some(e.to_string());
                }
            }
        });

        Ok(())
    }

    async fn download_file(
        client: reqwest::Client,
        tasks: Arc<Mutex<Vec<DownloadTask>>>,
        task_id: String,
        progress_tx: Option<mpsc::Sender<DownloadProgress>>,
        history: Arc<Mutex<HistoryManager>>,
        cancel_tokens: Arc<Mutex<std::collections::HashMap<String, bool>>>,
    ) -> Result<()> {
        // 更新状态为下载中
        {
            let mut tasks_lock = tasks.lock().await;
            if let Some(task) = tasks_lock.iter_mut().find(|t| t.id == task_id) {
                task.status = DownloadStatus::Downloading;
            }
        }

        // 获取任务信息
        let (url, save_path, aweme_id, title, author, author_id, cover) = {
            let tasks_lock = tasks.lock().await;
            let task = tasks_lock.iter().find(|t| t.id == task_id)
                .ok_or_else(|| anyhow!("Task not found"))?;

            (
                task.url.clone(),
                PathBuf::from(&task.save_path),
                task.aweme_id.clone(),
                task.title.clone(),
                task.author.clone(),
                task.author_id.clone(),
                task.cover.clone(),
            )
        };

        // 确保目录存在
        if let Some(parent) = save_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // 开始下载
        let response = client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(anyhow!("HTTP error: {}", response.status()));
        }

        let total_size = response.content_length().unwrap_or(0);

        // 更新总大小
        {
            let mut tasks_lock = tasks.lock().await;
            if let Some(task) = tasks_lock.iter_mut().find(|t| t.id == task_id) {
                task.total_size = total_size;
            }
        }

        let mut file = tokio::fs::File::create(&save_path).await?;
        let mut downloaded: u64 = 0;
        let mut stream = response.bytes_stream();
        let start_time = std::time::Instant::now();

        while let Some(chunk_result) = stream.next().await {
            // 检查是否取消
            if *cancel_tokens.lock().await.get(&task_id).unwrap_or(&false) {
                let _ = tokio::fs::remove_file(&save_path).await;
                return Err(anyhow!("Download cancelled"));
            }

            let chunk = chunk_result?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;

            // 更新进度
            let progress = if total_size > 0 {
                (downloaded as f32 / total_size as f32) * 100.0
            } else {
                0.0
            };

            {
                let mut tasks_lock = tasks.lock().await;
                if let Some(task) = tasks_lock.iter_mut().find(|t| t.id == task_id) {
                    task.progress = progress;
                    task.downloaded_size = downloaded;
                }
            }

            // 计算速度
            let elapsed = start_time.elapsed().as_secs();
            let speed = if elapsed > 0 { downloaded / elapsed } else { 0 };

            // 发送进度
            if let Some(tx) = &progress_tx {
                let _ = tx.send(DownloadProgress {
                    task_id: task_id.clone(),
                    status: DownloadStatus::Downloading,
                    progress,
                    downloaded_size: downloaded,
                    total_size,
                    speed,
                    error_msg: None,
                }).await;
            }
        }

        // 下载完成
        {
            let mut tasks_lock = tasks.lock().await;
            if let Some(task) = tasks_lock.iter_mut().find(|t| t.id == task_id) {
                task.status = DownloadStatus::Completed;
                task.progress = 100.0;
                task.complete_time = Some(Local::now().timestamp());
            }
        }

        // 添加到历史
        {
            let mut history_lock = history.lock().await;
            history_lock.add(crate::api::DownloadHistory {
                aweme_id,
                title,
                author,
                author_id,
                cover,
                file_path: save_path.to_string_lossy().to_string(),
                media_type: "video".to_string(),
                file_size: downloaded,
                create_time: Local::now().timestamp(),
            }).ok();
        }

        // 发送完成通知
        if let Some(tx) = &progress_tx {
            let _ = tx.send(DownloadProgress {
                task_id: task_id.clone(),
                status: DownloadStatus::Completed,
                progress: 100.0,
                downloaded_size: downloaded,
                total_size,
                speed: 0,
                error_msg: None,
            }).await;
        }

        Ok(())
    }

    /// 获取任务列表
    pub async fn get_tasks(&self) -> Vec<DownloadTask> {
        self.tasks.lock().await.clone()
    }

    /// 取消任务
    pub async fn cancel_task(&self, task_id: &str) -> Result<()> {
        let mut tokens = self.cancel_tokens.lock().await;
        tokens.insert(task_id.to_string(), true);

        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) {
            task.status = DownloadStatus::Cancelled;
        }

        Ok(())
    }

    /// 暂停任务
    pub async fn pause_task(&self, task_id: &str) -> Result<()> {
        self.cancel_task(task_id).await
    }

    /// 恢复任务
    pub async fn resume_task(&self, task_id: &str) -> Result<()> {
        self.start_download(task_id).await
    }

    /// 删除任务
    pub async fn remove_task(&self, task_id: &str) -> Result<()> {
        let mut tasks = self.tasks.lock().await;
        tasks.retain(|t| t.id != task_id);

        let mut tokens = self.cancel_tokens.lock().await;
        tokens.remove(task_id);

        Ok(())
    }
}

/// 清理文件名
fn sanitize_filename(name: &str) -> String {
    let invalid_chars = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
    let mut result = name.to_string();
    for c in invalid_chars {
        result = result.replace(c, "_");
    }
    if result.len() > 100 {
        result = result[..100].to_string();
    }
    result.trim().to_string()
}
