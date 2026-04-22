//! 下载历史管理

use crate::api::DownloadHistory;
use anyhow::Result;
use serde_json;
use std::fs;
use std::path::PathBuf;

/// 历史管理器
pub struct HistoryManager {
    history: Vec<DownloadHistory>,
    file_path: PathBuf,
}

impl HistoryManager {
    /// 加载历史记录
    pub fn load() -> Self {
        let file_path = Self::get_history_path();

        let history = if file_path.exists() {
            fs::read_to_string(&file_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            vec![]
        };

        Self { history, file_path }
    }

    /// 获取历史文件路径
    fn get_history_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("douyin-downloader")
            .join("history.json")
    }

    /// 保存到文件
    fn save(&self) -> Result<()> {
        if let Some(parent) = self.file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(&self.history)?;
        fs::write(&self.file_path, json)?;

        Ok(())
    }

    /// 获取所有历史
    pub fn get_all(&self) -> Vec<DownloadHistory> {
        self.history.clone()
    }

    /// 添加历史记录
    pub fn add(&mut self, record: DownloadHistory) -> Result<()> {
        // 检查是否已存在
        if let Some(pos) = self
            .history
            .iter()
            .position(|h| h.aweme_id == record.aweme_id)
        {
            self.history.remove(pos);
        }

        // 添加到开头
        self.history.insert(0, record);

        // 限制数量
        if self.history.len() > 1000 {
            self.history.truncate(1000);
        }

        self.save()
    }

    /// 删除历史记录
    pub fn delete(&mut self, aweme_id: &str) -> Result<()> {
        self.history.retain(|h| h.aweme_id != aweme_id);
        self.save()
    }

    /// 清空历史
    pub fn clear(&mut self) -> Result<()> {
        self.history.clear();
        self.save()
    }

    /// 检查是否已下载
    pub fn is_downloaded(&self, aweme_id: &str) -> bool {
        self.history.iter().any(|h| h.aweme_id == aweme_id)
    }

    /// 获取记录
    pub fn get(&self, aweme_id: &str) -> Option<&DownloadHistory> {
        self.history.iter().find(|h| h.aweme_id == aweme_id)
    }
}
