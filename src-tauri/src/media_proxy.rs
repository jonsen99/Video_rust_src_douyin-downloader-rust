use crate::config::get_user_agent;
use crate::AppState;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, Response, StatusCode};
use axum::routing::get;
use axum::Router;
use futures::StreamExt;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::TcpListener;
use tower_http::services::ServeDir;
use url::Url;

pub const MEDIA_PROXY_PORT: u16 = 39143;
const INITIAL_VIDEO_RANGE: &str = "bytes=0-1048575";
const MAX_RETRIES: usize = 3;

#[derive(Debug, Deserialize)]
struct MediaProxyQuery {
    url: String,
    media_type: Option<String>,
}

fn is_allowed_media_url(url: &str) -> bool {
    let allowed = [
        "douyin",
        "douyinvod",
        "douyinpic",
        "byteimg",
        "douyinstatic",
        "ixigua",
    ];

    allowed.iter().any(|part| url.contains(part))
}

fn build_error_response(status: StatusCode, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::from(message.to_string()))
        .unwrap_or_else(|_| Response::new(Body::from(message.to_string())))
}

fn frontend_dist_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist")
}

fn resolve_redirect_target(current_url: &Url, location: &str) -> Option<String> {
    if let Ok(url) = Url::parse(location) {
        return Some(url.to_string());
    }
    current_url.join(location).ok().map(|url| url.to_string())
}

fn guess_content_type(
    url: &str,
    upstream_content_type: &str,
    requested_media_type: &str,
) -> Option<&'static str> {
    let normalized = upstream_content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase();

    if requested_media_type == "audio" {
        if normalized.starts_with("audio/") {
            return Some("audio/mpeg");
        }
        if url.ends_with(".m4a") {
            return Some("audio/mp4");
        }
        return Some("audio/mpeg");
    }

    if !normalized.is_empty() && normalized != "application/octet-stream" {
        return None;
    }

    if url.contains(".mp4") || url.contains("/play/") || requested_media_type == "video" {
        return Some("video/mp4");
    }
    if url.contains(".jpg") || url.contains(".jpeg") {
        return Some("image/jpeg");
    }
    if url.contains(".png") {
        return Some("image/png");
    }
    if url.contains(".webp") {
        return Some("image/webp");
    }

    None
}

async fn media_proxy(
    State(state): State<AppState>,
    Query(query): Query<MediaProxyQuery>,
    request_headers: HeaderMap,
) -> Response<Body> {
    let requested_media_type = query
        .media_type
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let request_range = request_headers.get(header::RANGE).cloned();
    let request_range_str = request_range
        .as_ref()
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();

    if query.url.is_empty() || !is_allowed_media_url(&query.url) {
        return build_error_response(StatusCode::BAD_REQUEST, "Invalid URL");
    }

    let parsed_url = match Url::parse(&query.url) {
        Ok(url) => url,
        Err(_) => return build_error_response(StatusCode::BAD_REQUEST, "Invalid URL"),
    };

    if parsed_url.scheme() != "https" && parsed_url.scheme() != "http" {
        return build_error_response(StatusCode::BAD_REQUEST, "Invalid URL");
    }

    let config = state.config.lock().await.clone();
    let should_seed_video_range = request_range.is_none()
        && (requested_media_type == "video" || query.url.contains("/play/"));
    let upstream_range_value = if let Some(range) = &request_range {
        range.to_str().ok().map(|value| value.to_string())
    } else if should_seed_video_range {
        Some(INITIAL_VIDEO_RANGE.to_string())
    } else {
        None
    };
    let cache_key = if query.url.contains("/aweme/v1/play/") {
        Some(query.url.clone())
    } else {
        None
    };
    let cached_url = if let Some(key) = &cache_key {
        state.media_redirect_cache.lock().await.get(key).cloned()
    } else {
        None
    };
    let mut upstream_url = cached_url.clone().unwrap_or_else(|| query.url.clone());

    let start = std::time::Instant::now();
    let mut redirect_hops = 0usize;
    let mut retry_count = 0usize;
    let upstream_response = loop {
        let mut upstream = state
            .media_http_client
            .get(&upstream_url)
            .header("User-Agent", get_user_agent())
            .header("Referer", "https://www.douyin.com/")
            .header("Accept", "*/*")
            .header("Accept-Encoding", "identity;q=1, *;q=0");

        if !config.cookie.is_empty() {
            upstream = upstream.header("Cookie", &config.cookie);
        }

        if let Some(range_value) = &upstream_range_value {
            upstream = upstream.header("Range", range_value);
        }

        match upstream.send().await {
            Ok(response) => {
                let status = response.status();

                // 处理重定向
                if status.is_redirection() {
                    let location = response
                        .headers()
                        .get(header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("");

                    if location.is_empty() || redirect_hops >= 4 {
                        break response;
                    }

                    if let Some(next_url) = resolve_redirect_target(response.url(), location) {
                        redirect_hops += 1;
                        upstream_url = next_url;
                        continue;
                    }
                }

                // 处理服务器错误 (5xx)，尝试重试
                if status.is_server_error() && retry_count < MAX_RETRIES {
                    retry_count += 1;
                    log::warn!(
                        "media proxy upstream server error: status={} retry={}/{} url={}",
                        status,
                        retry_count,
                        MAX_RETRIES,
                        upstream_url.chars().take(80).collect::<String>()
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(
                        500 * retry_count as u64,
                    ))
                    .await;
                    continue;
                }

                if let Some(key) = &cache_key {
                    if upstream_url != *key {
                        state
                            .media_redirect_cache
                            .lock()
                            .await
                            .insert(key.clone(), upstream_url.clone());
                    }
                }

                break response;
            }
            Err(error) => {
                // 网络错误，尝试重试
                if retry_count < MAX_RETRIES {
                    retry_count += 1;
                    log::warn!(
                        "media proxy network error, retrying: {:?} retry={}/{} url={}",
                        error,
                        retry_count,
                        MAX_RETRIES,
                        upstream_url.chars().take(80).collect::<String>()
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(
                        500 * retry_count as u64,
                    ))
                    .await;
                    continue;
                }

                if let Some(key) = &cache_key {
                    state.media_redirect_cache.lock().await.remove(key);
                }
                log::error!(
                    "media proxy upstream request failed: {:?} elapsed={}ms seeded_range={} range=\"{}\" url={}",
                    error,
                    start.elapsed().as_millis(),
                    should_seed_video_range,
                    request_range_str,
                    upstream_url.chars().take(120).collect::<String>()
                );
                return build_error_response(StatusCode::BAD_GATEWAY, "Proxy error");
            }
        }
    };

    let status = upstream_response.status();
    let mut response_builder = Response::builder().status(status);
    let response_headers = response_builder.headers_mut().expect("headers available");

    for header_name in ["content-type", "content-range", "accept-ranges"] {
        if let Some(value) = upstream_response.headers().get(header_name) {
            response_headers.insert(
                axum::http::header::HeaderName::from_lowercase(header_name.as_bytes())
                    .expect("valid header"),
                value.clone(),
            );
        }
    }

    let upstream_content_type = upstream_response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();

    let is_media = requested_media_type == "audio"
        || requested_media_type == "video"
        || upstream_content_type.to_lowercase().contains("video");

    if let Some(content_length) = upstream_response.headers().get("content-length") {
        let content_length_str = content_length.to_str().unwrap_or_default();
        if !content_length_str.is_empty() {
            match content_length_str.parse::<u64>() {
                Ok(length) => {
                    if length < 2 * 1024 * 1024 || !is_media {
                        response_headers.insert(header::CONTENT_LENGTH, content_length.clone());
                    }
                }
                Err(_) => {
                    response_headers.insert(header::CONTENT_LENGTH, content_length.clone());
                }
            }
        }
    }

    if let Some(content_type) =
        guess_content_type(&query.url, &upstream_content_type, &requested_media_type)
    {
        response_headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    }

    if (requested_media_type == "audio" || requested_media_type == "video")
        && !response_headers.contains_key(header::ACCEPT_RANGES)
    {
        response_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    }

    response_headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );

    let stream = upstream_response
        .bytes_stream()
        .map(|result| result.map_err(std::io::Error::other));

    response_builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| build_error_response(StatusCode::BAD_GATEWAY, "Proxy error"))
}

pub async fn spawn_media_proxy(state: AppState) -> anyhow::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], MEDIA_PROXY_PORT));
    let listener = TcpListener::bind(addr).await?;
    let dist_dir = frontend_dist_dir();

    log::info!(
        "local web server listening on http://{} (dist={})",
        addr,
        dist_dir.display()
    );

    tokio::spawn(async move {
        let app = Router::new()
            .route("/api/media/proxy", get(media_proxy))
            .fallback_service(ServeDir::new(dist_dir).append_index_html_on_directories(true))
            .with_state(state);

        if let Err(error) = axum::serve(listener, app).await {
            log::error!("local web server failed: {}", error);
        }
    });

    Ok(())
}
