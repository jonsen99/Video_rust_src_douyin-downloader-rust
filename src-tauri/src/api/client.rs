//! API 客户端

use crate::config::{get_user_agent, AppConfig};
use crate::sign;
use anyhow::{anyhow, Result};
use rand::{distributions::Alphanumeric, Rng};
use regex::Regex;
use reqwest::redirect::Policy;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use super::types::*;

/// 抖音 API 客户端
#[derive(Clone)]
pub struct DouyinClient {
    client: reqwest::Client,
    config: AppConfig,
    webid_cache: Arc<Mutex<Option<(String, Instant)>>>,
}

impl DouyinClient {
    pub fn new(config: AppConfig) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(Policy::limited(5))
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
            webid_cache: Arc::new(Mutex::new(None)),
        })
    }

    fn build_common_params() -> HashMap<String, String> {
        let mut params = HashMap::new();
        params.insert("device_platform".to_string(), "webapp".to_string());
        params.insert("aid".to_string(), "6383".to_string());
        params.insert("channel".to_string(), "channel_pc_web".to_string());
        params.insert("update_version_code".to_string(), "0".to_string());
        params.insert("pc_client_type".to_string(), "1".to_string());
        params.insert("version_code".to_string(), "190600".to_string());
        params.insert("version_name".to_string(), "19.6.0".to_string());
        params.insert("cookie_enabled".to_string(), "true".to_string());
        params.insert("screen_width".to_string(), "1680".to_string());
        params.insert("screen_height".to_string(), "1050".to_string());
        params.insert("browser_language".to_string(), "zh-CN".to_string());
        params.insert("browser_platform".to_string(), "MacIntel".to_string());
        params.insert("browser_name".to_string(), "Edge".to_string());
        params.insert("browser_version".to_string(), "145.0.0.0".to_string());
        params.insert("browser_online".to_string(), "true".to_string());
        params.insert("engine_name".to_string(), "Blink".to_string());
        params.insert("engine_version".to_string(), "145.0.0.0".to_string());
        params.insert("os_name".to_string(), "Mac OS".to_string());
        params.insert("os_version".to_string(), "10.15.7".to_string());
        params.insert("cpu_core_num".to_string(), "8".to_string());
        params.insert("device_memory".to_string(), "8".to_string());
        params.insert("platform".to_string(), "PC".to_string());
        params.insert("downlink".to_string(), "10".to_string());
        params.insert("effective_type".to_string(), "4g".to_string());
        params.insert("round_trip_time".to_string(), "50".to_string());
        params.insert("pc_libra_divert".to_string(), "Mac".to_string());
        params.insert("support_h265".to_string(), "1".to_string());
        params.insert("support_dash".to_string(), "1".to_string());
        params.insert("disable_rs".to_string(), "0".to_string());
        params.insert("need_filter_settings".to_string(), "1".to_string());
        params.insert("list_type".to_string(), "single".to_string());
        params
    }

    fn build_common_headers(cookie: &str) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert("User-Agent".to_string(), get_user_agent().to_string());
        headers.insert("Accept".to_string(), "application/json, text/plain, */*".to_string());
        headers.insert(
            "Accept-Language".to_string(),
            "zh-CN,zh;q=0.9,en;q=0.8,en-GB;q=0.7,en-US;q=0.6".to_string(),
        );
        headers.insert("Referer".to_string(), "https://www.douyin.com/".to_string());
        headers.insert("priority".to_string(), "u=1, i".to_string());
        headers.insert("sec-fetch-site".to_string(), "same-origin".to_string());
        headers.insert("sec-fetch-mode".to_string(), "cors".to_string());
        headers.insert("sec-fetch-dest".to_string(), "empty".to_string());
        headers.insert("sec-ch-ua-platform".to_string(), "\"macOS\"".to_string());
        headers.insert("sec-ch-ua-mobile".to_string(), "?0".to_string());
        headers.insert(
            "sec-ch-ua".to_string(),
            "\"Not:A-Brand\";v=\"99\", \"Microsoft Edge\";v=\"145\", \"Chromium\";v=\"145\"".to_string(),
        );
        if !cookie.is_empty() {
            headers.insert("Cookie".to_string(), cookie.to_string());
        }
        headers
    }

    fn cookies_to_dict(cookie_str: &str) -> HashMap<String, String> {
        let mut cookie_dict = HashMap::new();

        for item in cookie_str.split(';') {
            let trimmed = item.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some((key, value)) = trimmed.split_once('=') {
                cookie_dict.insert(key.trim().to_string(), value.to_string());
            }
        }

        cookie_dict
    }

    fn generate_ms_token() -> String {
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(107)
            .map(char::from)
            .collect()
    }

    fn generate_verify_fp() -> String {
        let random_str: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(16)
            .map(char::from)
            .collect::<String>()
            .to_lowercase();
        format!("verify_0{}", random_str)
    }

    async fn get_webid(&self, headers: &HashMap<String, String>) -> Option<String> {
        {
            let cache = self.webid_cache.lock().await;
            if let Some((webid, cached_at)) = &*cache {
                if cached_at.elapsed() < Duration::from_secs(600) {
                    return Some(webid.clone());
                }
            }
        }

        let mut request_headers = headers.clone();
        request_headers.insert("sec-fetch-dest".to_string(), "document".to_string());
        request_headers.insert("sec-fetch-mode".to_string(), "navigate".to_string());
        request_headers.insert(
            "Accept".to_string(),
            "text/html,application/xhtml+xml".to_string(),
        );

        let mut req = self.client.get("https://www.douyin.com/?recommend=1");
        for (key, value) in &request_headers {
            req = req.header(key, value);
        }

        let response = req.send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }

        let html = response.text().await.ok()?;
        let patterns = [
            r#"\\"user_unique_id\\":\\"(\d+)\\""#,
            r#""user_unique_id":"(\d+)""#,
            r#""webid":"(\d+)""#,
            r#"webid=(\d+)"#,
        ];

        for pattern in patterns {
            if let Ok(re) = Regex::new(pattern) {
                if let Some(caps) = re.captures(&html) {
                    if let Some(matched) = caps.get(1) {
                        let webid = matched.as_str().to_string();
                        let mut cache = self.webid_cache.lock().await;
                        *cache = Some((webid.clone(), Instant::now()));
                        return Some(webid);
                    }
                }
            }
        }

        None
    }

    async fn enrich_request(
        &self,
        params: &mut HashMap<String, String>,
        headers: &mut HashMap<String, String>,
    ) {
        let cookie = headers
            .get("cookie")
            .or_else(|| headers.get("Cookie"))
            .cloned()
            .unwrap_or_else(|| self.config.cookie.clone());

        if cookie.is_empty() {
            return;
        }

        let cookie_dict = Self::cookies_to_dict(&cookie);

        params
            .entry("msToken".to_string())
            .or_insert_with(Self::generate_ms_token);
        params.insert(
            "screen_width".to_string(),
            cookie_dict
                .get("dy_swidth")
                .cloned()
                .unwrap_or_else(|| params.get("screen_width").cloned().unwrap_or_else(|| "1680".to_string())),
        );
        params.insert(
            "screen_height".to_string(),
            cookie_dict
                .get("dy_sheight")
                .cloned()
                .unwrap_or_else(|| params.get("screen_height").cloned().unwrap_or_else(|| "1050".to_string())),
        );
        params.insert(
            "cpu_core_num".to_string(),
            cookie_dict
                .get("device_web_cpu_core")
                .cloned()
                .unwrap_or_else(|| params.get("cpu_core_num").cloned().unwrap_or_else(|| "8".to_string())),
        );
        params.insert(
            "device_memory".to_string(),
            cookie_dict
                .get("device_web_memory_size")
                .cloned()
                .unwrap_or_else(|| params.get("device_memory").cloned().unwrap_or_else(|| "8".to_string())),
        );

        let verify_fp = cookie_dict
            .get("s_v_web_id")
            .cloned()
            .unwrap_or_else(Self::generate_verify_fp);
        params.insert("verifyFp".to_string(), verify_fp.clone());
        params.insert("fp".to_string(), verify_fp);

        if let Some(uifid) = cookie_dict.get("UIFID") {
            headers.insert("uifid".to_string(), uifid.clone());
            params.insert("uifid".to_string(), uifid.clone());
        }

        if let Some(webid) = self.get_webid(headers).await {
            params.insert("webid".to_string(), webid);
        }
    }

    async fn request_with_options<T: DeserializeOwned>(
        &self,
        url: &str,
        params: Option<HashMap<&str, String>>,
        method: &str,
        extra_headers: Option<HashMap<String, String>>,
        skip_sign: bool,
    ) -> Result<T> {
        let started_at = Instant::now();
        let mut all_params = Self::build_common_params();

        if let Some(p) = params {
            for (key, value) in p {
                all_params.insert(key.to_string(), value);
            }
        }

        let mut headers = Self::build_common_headers(&self.config.cookie);
        if let Some(extra) = extra_headers {
            headers.extend(extra);
        }

        self.enrich_request(&mut all_params, &mut headers).await;

        if !skip_sign {
            let params_str = serde_urlencoded::to_string(&all_params)?;
            let user_agent = headers
                .get("User-Agent")
                .map(String::as_str)
                .unwrap_or_else(|| get_user_agent());
            let a_bogus = if url.contains("reply") {
                sign::sign_reply(&params_str, user_agent)
            } else {
                sign::sign_detail(&params_str, user_agent)
            };
            all_params.insert("a_bogus".to_string(), a_bogus);
        }

        log::info!(
            "API request started: method={} url={} skip_sign={}",
            method,
            url,
            skip_sign
        );

        let mut req = match method {
            "GET" => self.client.get(url).query(&all_params),
            "POST" => self.client.post(url).form(&all_params),
            _ => return Err(anyhow!("Unsupported HTTP method: {}", method)),
        };

        for (key, value) in headers {
            req = req.header(&key, value);
        }

        let response = req.send().await.map_err(|e| {
            log::error!(
                "API request failed: method={} url={} elapsed_ms={} error={}",
                method,
                url,
                started_at.elapsed().as_millis(),
                e
            );
            e
        })?;

        if !response.status().is_success() {
            log::warn!(
                "API request returned non-success status: method={} url={} status={} elapsed_ms={}",
                method,
                url,
                response.status(),
                started_at.elapsed().as_millis()
            );
            return Err(anyhow!("HTTP error: {}", response.status()));
        }

        let json = response.json::<T>().await.map_err(|e| {
            log::error!(
                "API response decode failed: method={} url={} elapsed_ms={} error={}",
                method,
                url,
                started_at.elapsed().as_millis(),
                e
            );
            e
        })?;
        log::info!(
            "API request completed: method={} url={} elapsed_ms={}",
            method,
            url,
            started_at.elapsed().as_millis()
        );
        Ok(json)
    }

    /// 通用请求方法
    pub async fn request<T: DeserializeOwned>(
        &self,
        url: &str,
        params: Option<HashMap<&str, String>>,
        method: &str,
    ) -> Result<ApiResponse<T>> {
        self.request_with_options(url, params, method, None, false).await
    }

    pub async fn request_raw_json(
        &self,
        url: &str,
        params: Option<HashMap<&str, String>>,
        method: &str,
    ) -> Result<serde_json::Value> {
        self.request_with_options(url, params, method, None, false).await
    }

    pub async fn request_raw_json_with_options(
        &self,
        url: &str,
        params: Option<HashMap<&str, String>>,
        method: &str,
        extra_headers: Option<HashMap<String, String>>,
        skip_sign: bool,
    ) -> Result<serde_json::Value> {
        self.request_with_options(url, params, method, extra_headers, skip_sign).await
    }

    /// 从 URL 提取视频 ID
    pub fn extract_aweme_id(url: &str) -> Option<String> {
        // 直接是 aweme_id
        if Regex::new(r"^\d+$").unwrap().is_match(url) {
            return Some(url.to_string());
        }

        // 从分享链接提取
        let patterns = [
            r"video/(\d+)",
            r"note/(\d+)",
            r"aweme_id=(\d+)",
            r"/(\d{19})",
        ];

        for pattern in &patterns {
            if let Ok(re) = Regex::new(pattern) {
                if let Some(caps) = re.captures(url) {
                    if let Some(id) = caps.get(1) {
                        return Some(id.as_str().to_string());
                    }
                }
            }
        }

        None
    }

    /// 获取视频详情
    pub async fn get_video_detail(&self, aweme_id: &str) -> Result<VideoInfo> {
        let mut params = HashMap::new();
        params.insert("aweme_id", aweme_id.to_string());
        params.insert("aid", "1128".to_string());
        params.insert("version_name", "23.5.0".to_string());
        params.insert("device_platform", "webapp".to_string());
        params.insert("os", "windows".to_string());

        let response = match self
            .request_raw_json_with_options(
                "https://www.douyin.com/aweme/v1/web/aweme/detail/",
                Some(params.clone()),
                "GET",
                None,
                true,
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                log::warn!(
                    "video detail unsigned request failed, retrying with signature: aweme_id={} error={}",
                    aweme_id,
                    error
                );
                self.request_raw_json_with_options(
                    "https://www.douyin.com/aweme/v1/web/aweme/detail/",
                    Some(params),
                    "GET",
                    None,
                    false,
                )
                .await?
            }
        };

        let status_code = response["status_code"].as_i64().unwrap_or(-1);
        if status_code != 0 {
            let status_msg = response["status_msg"].as_str().unwrap_or("unknown error");
            return Err(anyhow!("API error: {}", status_msg));
        }

        let data = response
            .get("aweme_detail")
            .ok_or_else(|| anyhow!("No aweme_detail in response"))?;
        let video_info = self.parse_video_info(data)?;

        Ok(video_info)
    }

    /// 解析视频信息
    fn parse_video_info(&self, data: &serde_json::Value) -> Result<VideoInfo> {
        let aweme_id = data["aweme_id"].as_str().unwrap_or_default().to_string();
        let desc = data["desc"].as_str().unwrap_or_default().to_string();
        let create_time = data["create_time"].as_i64().unwrap_or(0);

        // 作者信息
        let author_data = &data["author"];
        let author = AuthorInfo {
            uid: author_data["uid"].as_str().unwrap_or_default().to_string(),
            sec_uid: author_data["sec_uid"].as_str().unwrap_or_default().to_string(),
            nickname: author_data["nickname"].as_str().unwrap_or_default().to_string(),
            avatar_thumb: self.get_first_url(&author_data["avatar_thumb"]["url_list"]),
            avatar_medium: self.get_first_url(&author_data["avatar_medium"]["url_list"]),
            signature: author_data["signature"].as_str().unwrap_or_default().to_string(),
            follower_count: author_data["follower_count"].as_i64().unwrap_or(0),
            following_count: author_data["following_count"].as_i64().unwrap_or(0),
            aweme_count: author_data["aweme_count"].as_i64().unwrap_or(0),
            favoriting_count: author_data["favoriting_count"].as_i64().unwrap_or(0),
            is_follow: author_data["is_follow"].as_bool().unwrap_or(false),
            verify_status: author_data["verify_status"].as_i64().unwrap_or(0) as i32,
            unique_id: author_data["unique_id"].as_str().unwrap_or_default().to_string(),
        };

        // 视频数据
        let video_data = &data["video"];
        let video = VideoData {
            play_addr: self.parse_video_url(&video_data["play_addr"]),
            download_addr: Some(self.parse_video_url(&video_data["download_addr"])),
            cover: self.get_first_url(&video_data["cover"]["url_list"]),
            dynamic_cover: self.get_first_url(&video_data["dynamic_cover"]["url_list"]),
            origin_cover: self.get_first_url(&video_data["origin_cover"]["url_list"]),
            width: video_data["width"].as_i64().unwrap_or(0) as i32,
            height: video_data["height"].as_i64().unwrap_or(0) as i32,
            duration: video_data["duration"].as_i64().unwrap_or(0),
            ratio: video_data["ratio"].as_str().unwrap_or_default().to_string(),
            bit_rate: video_data["bit_rate"].as_array().map(|arr| {
                arr.iter().map(|b| BitRateInfo {
                    gear_name: b["gear_name"].as_str().unwrap_or_default().to_string(),
                    bit_rate: b["bit_rate"].as_i64().unwrap_or(0),
                    width: b["width"].as_i64().unwrap_or(0) as i32,
                    height: b["height"].as_i64().unwrap_or(0) as i32,
                }).collect()
            }),
        };

        // 统计
        let stats = &data["statistics"];
        let statistics = Statistics {
            play_count: stats["play_count"].as_i64().unwrap_or(0),
            digg_count: stats["digg_count"].as_i64().unwrap_or(0),
            comment_count: stats["comment_count"].as_i64().unwrap_or(0),
            share_count: stats["share_count"].as_i64().unwrap_or(0),
            collect_count: stats["collect_count"].as_i64().unwrap_or(0),
            forward_count: stats["forward_count"].as_i64().unwrap_or(0),
        };

        // 状态
        let status_data = &data["status"];
        let status = Status {
            is_delete: status_data["is_delete"].as_bool().unwrap_or(false),
            private_status: status_data["private_status"].as_i64().unwrap_or(0) as i32,
            review_status: status_data["review_status"].as_i64().unwrap_or(0) as i32,
            with_goods: status_data["with_goods"].as_bool().unwrap_or(false),
            is_prohibited: status_data["is_prohibited"].as_bool().unwrap_or(false),
        };

        // 图片列表 (图集)
        let is_image = data["images"].as_array().map_or(false, |arr| !arr.is_empty());
        let image_urls = if is_image {
            data["images"].as_array().map(|arr| {
                arr.iter()
                    .filter_map(|img| self.get_first_url_opt(&img["url_list"]))
                    .collect()
            })
        } else {
            None
        };

        // 音乐信息
        let music = if data["music"].is_object() {
            let m = &data["music"];
            Some(MusicInfo {
                id: m["id"].as_str().unwrap_or_default().to_string(),
                title: m["title"].as_str().unwrap_or_default().to_string(),
                author: m["author"].as_str().unwrap_or_default().to_string(),
                play_url: Some(self.parse_video_url(&m["play_url"])),
                cover_thumb: self.get_first_url(&m["cover_thumb"]["url_list"]),
                duration: m["duration"].as_i64().unwrap_or(0),
            })
        } else {
            None
        };

        // 文本额外信息
        let text_extra = data["text_extra"].as_array().map(|arr| {
            arr.iter().map(|t| TextExtra {
                text: t["text"].as_str().unwrap_or_default().to_string(),
                r#type: t["type"].as_i64().unwrap_or(0) as i32,
                hashtag_name: t["hashtag_name"].as_str().map(|s| s.to_string()),
                aweme_id: t["aweme_id"].as_str().map(|s| s.to_string()),
                sec_uid: t["sec_uid"].as_str().map(|s| s.to_string()),
                user_id: t["user_id"].as_str().map(|s| s.to_string()),
            }).collect()
        });

        // 判断媒体类型
        let raw_media_type = data["raw_media_type"].as_i64().map(|v| v as i32);

        Ok(VideoInfo {
            aweme_id,
            desc,
            create_time,
            author,
            video,
            statistics,
            status,
            image_urls,
            is_image,
            music,
            raw_media_type,
            text_extra,
        })
    }

    fn parse_video_url(&self, data: &serde_json::Value) -> VideoUrl {
        VideoUrl {
            url_list: data["url_list"].as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default(),
            uri: data["uri"].as_str().unwrap_or_default().to_string(),
        }
    }

    fn get_first_url(&self, data: &serde_json::Value) -> String {
        data.as_array()
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    }

    fn get_first_url_opt(&self, data: &serde_json::Value) -> Option<String> {
        data.as_array()
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// 获取无水印视频 URL
    pub fn get_no_watermark_url(video: &VideoInfo) -> Option<String> {
        // 优先使用 download_addr
        if let Some(download_addr) = &video.video.download_addr {
            if let Some(url) = download_addr.url_list.first() {
                return Some(url.clone());
            }
        }

        // 使用 play_addr 并替换水印参数
        if let Some(url) = video.video.play_addr.url_list.first() {
            let clean_url = url
                .replace("watermark=1", "watermark=0")
                .replace("&watermark=", "")
                .replace("playwm", "play");
            return Some(clean_url);
        }
        None
    }

    /// 搜索用户
    pub async fn search_user(&self, keyword: &str) -> Result<Vec<UserInfo>> {
        let mut params = HashMap::new();
        params.insert("keyword", keyword.to_string());
        params.insert("search_channel", "aweme_user_web".to_string());
        params.insert("search_source", "normal_search".to_string());
        params.insert("query_correct_type", "1".to_string());
        params.insert("is_filter_search", "0".to_string());
        params.insert("from_group_id", "".to_string());
        params.insert("offset", "0".to_string());
        params.insert("count", "10".to_string());
        params.insert(
            "pc_search_top_1_params",
            "{\"enable_ai_search_top_1\":1}".to_string(),
        );

        let mut headers = HashMap::new();
        headers.insert(
            "Referer".to_string(),
            format!(
                "https://www.douyin.com/jingxuan/search/{}?type=user",
                keyword
            ),
        );

        let response = self
            .request_raw_json_with_options(
                "https://www.douyin.com/aweme/v1/web/discover/search/",
                Some(params),
                "GET",
                Some(headers),
                true,
            )
            .await?;

        let status_code = response["status_code"].as_i64().unwrap_or(-1);
        if status_code != 0 {
            let status_msg = response["status_msg"].as_str().unwrap_or("unknown error");
            return Err(anyhow!("API error: {}", status_msg));
        }

        let users = response["user_list"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let user = if item["user_info"].is_object() {
                            &item["user_info"]
                        } else {
                            item
                        };
                        Some(UserInfo {
                            uid: user["uid"].as_str()?.to_string(),
                            nickname: user["nickname"].as_str()?.to_string(),
                            avatar_thumb: self.get_first_url(&user["avatar_thumb"]["url_list"]),
                            avatar_medium: self.get_first_url(&user["avatar_medium"]["url_list"]),
                            signature: user["signature"].as_str().unwrap_or_default().to_string(),
                            follower_count: user["follower_count"].as_i64().unwrap_or(0),
                            following_count: user["following_count"].as_i64().unwrap_or(0),
                            aweme_count: user["aweme_count"].as_i64().unwrap_or(0),
                            favoriting_count: user["favoriting_count"].as_i64().unwrap_or(0),
                            is_follow: user["is_follow"].as_bool().unwrap_or(false),
                            sec_uid: user["sec_uid"].as_str().unwrap_or_default().to_string(),
                            unique_id: user["unique_id"].as_str().unwrap_or_default().to_string(),
                            verify_status: user["verify_status"].as_i64().unwrap_or(0) as i32,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(users)
    }

    /// 获取用户详情
    pub async fn get_user_detail(&self, sec_uid: &str) -> Result<UserDetail> {
        let mut params = HashMap::new();
        params.insert("sec_user_id", sec_uid.to_string());
        params.insert("personal_center_strategy", "1".to_string());
        params.insert("source", "channel_pc_web".to_string());

        let mut headers = HashMap::new();
        headers.insert("Referer".to_string(), "https://www.douyin.com/".to_string());

        let response = self
            .request_raw_json_with_options(
                "https://www.douyin.com/aweme/v1/web/user/profile/other/",
                Some(params),
                "GET",
                Some(headers),
                true,
            )
            .await?;

        let status_code = response["status_code"].as_i64().unwrap_or(-1);
        if status_code != 0 {
            let status_msg = response["status_msg"].as_str().unwrap_or("unknown error");
            return Err(anyhow!("API error: {}", status_msg));
        }

        let user_data = &response["user"];

        let info = UserInfo {
            uid: user_data["uid"].as_str().unwrap_or_default().to_string(),
            nickname: user_data["nickname"].as_str().unwrap_or_default().to_string(),
            avatar_thumb: self.get_first_url(&user_data["avatar_thumb"]["url_list"]),
            avatar_medium: self.get_first_url(&user_data["avatar_medium"]["url_list"]),
            signature: user_data["signature"].as_str().unwrap_or_default().to_string(),
            follower_count: user_data["follower_count"].as_i64().unwrap_or(0),
            following_count: user_data["following_count"].as_i64().unwrap_or(0),
            aweme_count: user_data["aweme_count"].as_i64().unwrap_or(0),
            favoriting_count: user_data["favoriting_count"].as_i64().unwrap_or(0),
            is_follow: user_data["is_follow"].as_bool().unwrap_or(false),
            sec_uid: user_data["sec_uid"].as_str().unwrap_or_default().to_string(),
            unique_id: user_data["unique_id"].as_str().unwrap_or_default().to_string(),
            verify_status: user_data["verify_status"].as_i64().unwrap_or(0) as i32,
        };

        Ok(UserDetail {
            info,
            is_favorite: response["is_favorite"].as_bool().unwrap_or(false),
            follow_status: response["follow_status"].as_i64().unwrap_or(0) as i32,
            story_count: response["story_count"].as_i64().unwrap_or(0),
            friend_status: response["friend_status"].as_i64().unwrap_or(0) as i32,
        })
    }

    /// 获取用户发布的视频列表
    pub async fn get_user_videos(
        &self,
        sec_uid: &str,
        max_cursor: i64,
        count: u32,
    ) -> Result<(Vec<VideoInfo>, i64, bool)> {
        let mut params = HashMap::new();
        params.insert("publish_video_strategy_type", "2".to_string());
        params.insert("sec_user_id", sec_uid.to_string());
        params.insert("max_cursor", max_cursor.to_string());
        params.insert("locate_query", "false".to_string());
        params.insert("show_live_replay_strategy", "1".to_string());
        params.insert("need_time_list", "0".to_string());
        params.insert("time_list_query", "0".to_string());
        params.insert("whale_cut_token", "".to_string());
        params.insert("count", count.to_string());

        let response = self
            .request_raw_json_with_options(
                "https://www.douyin.com/aweme/v1/web/aweme/post/",
                Some(params),
                "GET",
                None,
                true,
            )
            .await?;

        let status_code = response["status_code"].as_i64().unwrap_or(0);
        if status_code != 0 {
            let status_msg = response["status_msg"].as_str().unwrap_or("unknown error");
            return Err(anyhow!("API error: {}", status_msg));
        }

        let aweme_list = response["aweme_list"].as_array();
        let has_more = response["has_more"].as_i64().unwrap_or(0) == 1
            || response["has_more"].as_bool().unwrap_or(false);
        let cursor = response["max_cursor"].as_i64().unwrap_or(0);

        let videos = if let Some(list) = aweme_list {
            list.iter()
                .filter_map(|v| self.parse_video_info(v).ok())
                .collect()
        } else {
            vec![]
        };

        Ok((videos, cursor, has_more))
    }

    /// 获取点赞视频列表
    pub async fn get_liked_videos(
        &self,
        sec_uid: &str,
        max_cursor: i64,
        count: u32,
    ) -> Result<(Vec<VideoInfo>, i64, bool)> {
        let mut params = HashMap::new();
        params.insert("max_cursor", max_cursor.to_string());
        params.insert("count", count.to_string());
        if !sec_uid.is_empty() {
            params.insert("sec_user_id", sec_uid.to_string());
        }

        let mut headers = HashMap::new();
        headers.insert("Referer".to_string(), "https://www.douyin.com/".to_string());

        let response = self
            .request_raw_json_with_options(
                "https://www.douyin.com/aweme/v1/web/aweme/favorite/",
                Some(params),
                "GET",
                Some(headers),
                true,
            )
            .await?;

        let status_code = response["status_code"].as_i64().unwrap_or(0);
        if status_code != 0 {
            let status_msg = response["status_msg"].as_str().unwrap_or("unknown error");
            return Err(anyhow!("API error: {}", status_msg));
        }

        let aweme_list = response["aweme_list"].as_array();
        let has_more = response["has_more"].as_i64().unwrap_or(0) == 1
            || response["has_more"].as_bool().unwrap_or(false);
        let cursor = response["max_cursor"].as_i64().unwrap_or(0);

        let videos = if let Some(list) = aweme_list {
            list.iter()
                .filter_map(|v| self.parse_video_info(v).ok())
                .collect()
        } else {
            vec![]
        };

        Ok((videos, cursor, has_more))
    }

    /// 获取推荐视频
    pub async fn get_recommended_feed(
        &self,
        cursor: i64,
        count: u32,
    ) -> Result<(Vec<VideoInfo>, i64, bool)> {
        let mut params = HashMap::new();
        params.insert("module_id", "3003101".to_string());
        params.insert("count", count.to_string());
        params.insert("pull_type", "0".to_string());
        params.insert("refresh_index", "1".to_string());
        params.insert("refer_type", "10".to_string());
        params.insert("filterGids", "".to_string());
        params.insert("presented_ids", "".to_string());
        params.insert("refer_id", "".to_string());
        params.insert("tag_id", "".to_string());
        params.insert("use_lite_type", "2".to_string());
        params.insert("Seo-Flag", "0".to_string());
        params.insert("pre_log_id", "".to_string());
        params.insert("pre_item_ids", "".to_string());
        params.insert("pre_room_ids", "".to_string());
        params.insert("pre_item_from", "sati".to_string());
        params.insert("xigua_user", "0".to_string());
        params.insert(
            "awemePcRecRawData",
            "{\"is_xigua_user\":0,\"danmaku_switch_status\":0,\"is_client\":false}".to_string(),
        );
        if cursor > 0 {
            params.insert("cursor", cursor.to_string());
        }

        let mut headers = HashMap::new();
        headers.insert(
            "Referer".to_string(),
            "https://www.douyin.com/?recommend=1".to_string(),
        );

        let response = self
            .request_raw_json_with_options(
                "https://www.douyin.com/aweme/v2/web/module/feed/",
                Some(params),
                "POST",
                Some(headers),
                true,
            )
            .await?;

        let status_code = response["status_code"].as_i64().unwrap_or(-1);
        if status_code != 0 {
            let status_msg = response["status_msg"].as_str().unwrap_or("unknown error");
            return Err(anyhow!("API error: {}", status_msg));
        }

        let aweme_list = response["aweme_list"].as_array();
        let has_more = response["has_more"].as_bool().unwrap_or(false);
        let next_cursor = response["cursor"]
            .as_i64()
            .or_else(|| response["max_cursor"].as_i64())
            .or_else(|| response["min_cursor"].as_i64())
            .unwrap_or_else(|| if has_more { cursor + 1 } else { cursor });

        let videos = if let Some(list) = aweme_list {
            list.iter()
                .filter_map(|v| self.parse_video_info(v).ok())
                .collect()
        } else {
            vec![]
        };

        Ok((videos, next_cursor, has_more))
    }

    /// 获取评论列表
    pub async fn get_comments(
        &self,
        aweme_id: &str,
        cursor: i64,
        count: u32,
    ) -> Result<(Vec<CommentInfo>, i64, bool)> {
        let mut params = HashMap::new();
        params.insert("aweme_id", aweme_id.to_string());
        params.insert("cursor", cursor.to_string());
        params.insert("count", count.to_string());

        let response: ApiResponse<serde_json::Value> = self
            .request(
                "https://www.douyin.com/aweme/v1/web/comment/list/",
                Some(params),
                "GET",
            )
            .await?;

        if response.status_code != 0 {
            return Err(anyhow!("API error: {:?}", response.status_msg));
        }

        let data = response.data.ok_or_else(|| anyhow!("No data in response"))?;
        let comments_data = data["comments"].as_array();
        let has_more = data["has_more"].as_bool().unwrap_or(false);
        let cursor = data["cursor"].as_i64().unwrap_or(0);

        let comments = if let Some(list) = comments_data {
            list.iter().filter_map(|c| self.parse_comment(c)).collect()
        } else {
            vec![]
        };

        Ok((comments, cursor, has_more))
    }

    fn parse_comment(&self, data: &serde_json::Value) -> Option<CommentInfo> {
        let user = &data["user"];
        Some(CommentInfo {
            cid: data["cid"].as_str()?.to_string(),
            text: data["text"].as_str().unwrap_or_default().to_string(),
            create_time: data["create_time"].as_i64().unwrap_or(0),
            user: CommentUser {
                uid: user["uid"].as_str().unwrap_or_default().to_string(),
                nickname: user["nickname"].as_str().unwrap_or_default().to_string(),
                avatar_thumb: self.get_first_url(&user["avatar_thumb"]["url_list"]),
                sec_uid: user["sec_uid"].as_str().unwrap_or_default().to_string(),
            },
            digg_count: data["digg_count"].as_i64().unwrap_or(0),
            reply_comment_total: data["reply_comment_total"].as_i64().unwrap_or(0),
            sub_comments: None,
            status: data["status"].as_i64().unwrap_or(0) as i32,
        })
    }

    /// 解析分享链接
    pub async fn parse_share_link(&self, url: &str) -> Result<VideoInfo> {
        // 先请求获取重定向后的 URL
        let response = self.client
            .get(url)
            .header("User-Agent", get_user_agent())
            .send()
            .await?;

        let final_url = response.url().to_string();

        // 提取视频 ID
        let aweme_id = Self::extract_aweme_id(&final_url)
            .ok_or_else(|| anyhow!("Cannot extract video ID from URL"))?;

        self.get_video_detail(&aweme_id).await
    }

    /// 验证 Cookie 是否有效
    pub async fn verify_cookie(&self) -> Result<CookieStatus> {
        let response = self.get_recommended_feed(0, 1).await;

        match response {
            Ok(_) => Ok(CookieStatus {
                valid: true,
                user_name: None,
                user_id: None,
                expires_at: None,
                message: "Cookie 有效".to_string(),
            }),
            Err(e) => Ok(CookieStatus {
                valid: false,
                user_name: None,
                user_id: None,
                expires_at: None,
                message: format!("Cookie 无效: {}", e),
            }),
        }
    }

    /// 获取当前用户信息 (需要登录)
    pub async fn get_current_user(&self) -> Result<UserInfo> {
        let response = self
            .request_raw_json_with_options(
                "https://www.douyin.com/aweme/v1/web/user/profile/self/",
                None,
                "GET",
                None,
                true,
            )
            .await?;

        let status_code = response["status_code"].as_i64().unwrap_or(-1);
        if status_code != 0 {
            let status_msg = response["status_msg"].as_str().unwrap_or("unknown error");
            return Err(anyhow!("API error: {}", status_msg));
        }

        let data = response
            .get("user")
            .ok_or_else(|| anyhow!("No user in response"))?;

        Ok(UserInfo {
            uid: data["uid"].as_str().unwrap_or_default().to_string(),
            nickname: data["nickname"].as_str().unwrap_or_default().to_string(),
            avatar_thumb: self.get_first_url(&data["avatar_thumb"]["url_list"]),
            avatar_medium: self.get_first_url(&data["avatar_medium"]["url_list"]),
            signature: data["signature"].as_str().unwrap_or_default().to_string(),
            follower_count: data["follower_count"].as_i64().unwrap_or(0),
            following_count: data["following_count"].as_i64().unwrap_or(0),
            aweme_count: data["aweme_count"].as_i64().unwrap_or(0),
            favoriting_count: data["favoriting_count"].as_i64().unwrap_or(0),
            is_follow: false,
            sec_uid: data["sec_uid"].as_str().unwrap_or_default().to_string(),
            unique_id: data["unique_id"].as_str().unwrap_or_default().to_string(),
            verify_status: data["verify_status"].as_i64().unwrap_or(0) as i32,
        })
    }
}
