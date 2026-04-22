//! 配置模块

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// 应用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// 下载目录
    #[serde(alias = "download_dir")]
    pub download_path: String,
    /// Cookie
    pub cookie: String,
    /// 代理设置
    pub proxy: Option<String>,
    /// 最大并发下载数
    pub max_concurrent: usize,
    /// 下载质量
    #[serde(default = "default_download_quality")]
    pub download_quality: String,
    /// 文件名模板
    #[serde(default)]
    pub filename_template: String,
    /// 自动创建文件夹
    #[serde(default = "default_true")]
    pub auto_create_folder: bool,
    /// 文件夹名模板
    #[serde(default)]
    pub folder_name_template: String,
    /// 保存元数据
    #[serde(default = "default_true")]
    pub save_metadata: bool,
    /// 主题
    #[serde(default)]
    pub theme: String,
    /// 语言
    #[serde(default)]
    pub language: String,
}

fn default_true() -> bool {
    true
}
fn default_download_quality() -> String {
    "auto".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        let download_path = dirs::download_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        Self {
            download_path,
            cookie: String::new(),
            proxy: None,
            max_concurrent: 3,
            download_quality: default_download_quality(),
            filename_template: "{author}_{title}_{date}".to_string(),
            auto_create_folder: true,
            folder_name_template: "{author}".to_string(),
            save_metadata: true,
            theme: "dark".to_string(),
            language: "zh-CN".to_string(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Self {
        let config_path = Self::config_path();

        // 确保配置目录存在
        if let Some(parent) = config_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if config_path.exists() {
            match fs::read_to_string(&config_path) {
                Ok(content) => match serde_json::from_str(&content) {
                    Ok(config) => return config,
                    Err(e) => {
                        log::warn!("Failed to parse config file: {}, using default", e);
                    }
                },
                Err(e) => {
                    log::warn!("Failed to read config file: {}, using default", e);
                }
            }
        }

        Self::default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let config_path = Self::config_path();

        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(self)?;
        fs::write(&config_path, content)?;

        Ok(())
    }

    fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("douyin-downloader")
            .join("config.json")
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;

    #[test]
    fn deserializes_partial_config_with_defaults() {
        let config: AppConfig = serde_json::from_str(
            r#"{
            "download_dir": "/tmp/downloads",
            "cookie": "sessionid=test"
        }"#,
        )
        .expect("partial config should deserialize");

        assert_eq!(config.download_path, "/tmp/downloads");
        assert_eq!(config.cookie, "sessionid=test");
        assert_eq!(config.max_concurrent, 3);
        assert_eq!(config.download_quality, "auto");
    }
}

/// 抖音通用请求参数
pub fn get_common_params() -> HashMap<&'static str, &'static str> {
    let mut params = HashMap::new();
    params.insert("device_platform", "webapp");
    params.insert("aid", "6383");
    params.insert("channel", "channel_pc_web");
    params.insert("update_version_code", "0");
    params.insert("pc_client_type", "1");
    params.insert("version_code", "190600");
    params.insert("version_name", "19.6.0");
    params.insert("cookie_enabled", "true");
    params.insert("browser_language", "zh-CN");
    params.insert("browser_platform", "MacIntel");
    params.insert("browser_name", "Edge");
    params.insert("browser_version", "145.0.0.0");
    params.insert("browser_online", "true");
    params.insert("engine_name", "Blink");
    params.insert("engine_version", "145.0.0.0");
    params.insert("os_name", "Mac OS");
    params.insert("os_version", "10.15.7");
    params.insert("cpu_core_num", "8");
    params.insert("device_memory", "8");
    params.insert("platform", "PC");
    params.insert("screen_width", "1680");
    params.insert("screen_height", "1050");
    params.insert("downlink", "10");
    params.insert("effective_type", "4g");
    params.insert("round_trip_time", "50");
    params.insert("pc_libra_divert", "Mac");
    params.insert("support_h265", "1");
    params.insert("support_dash", "1");
    params.insert("disable_rs", "0");
    params.insert("need_filter_settings", "1");
    params.insert("list_type", "single");
    params
}

/// 通用请求头
pub fn get_common_headers(cookie: &str) -> HashMap<&'static str, String> {
    let mut headers = HashMap::new();
    headers.insert("Accept", "application/json, text/plain, */*".to_string());
    headers.insert(
        "Accept-Language",
        "zh-CN,zh;q=0.9,en;q=0.8,en-GB;q=0.7,en-US;q=0.6".to_string(),
    );
    headers.insert("Referer", "https://www.douyin.com/".to_string());
    headers.insert("priority", "u=1, i".to_string());
    headers.insert("sec-fetch-site", "same-origin".to_string());
    headers.insert("sec-fetch-mode", "cors".to_string());
    headers.insert("sec-fetch-dest", "empty".to_string());
    headers.insert("sec-ch-ua-platform", "\"macOS\"".to_string());
    headers.insert("sec-ch-ua-mobile", "?0".to_string());
    headers.insert(
        "sec-ch-ua",
        "\"Not:A-Brand\";v=\"99\", \"Microsoft Edge\";v=\"145\", \"Chromium\";v=\"145\""
            .to_string(),
    );
    headers.insert("User-Agent", get_user_agent().to_string());
    if !cookie.is_empty() {
        headers.insert("Cookie", cookie.to_string());
    }
    headers
}

/// User-Agent
pub fn get_user_agent() -> &'static str {
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36 Edg/145.0.0.0"
}
