use crate::config::get_user_agent;
use crate::AppState;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use axum::routing::get;
use axum::Router;
use futures::StreamExt;
use serde::Deserialize;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use url::Url;

pub const MEDIA_PROXY_PORT: u16 = 39143;

#[derive(Debug, Deserialize)]
struct MediaProxyQuery {
    url: String,
    filename: Option<String>,
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

fn guess_content_type(url: &str, upstream_content_type: &str, requested_media_type: &str) -> Option<&'static str> {
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
    log::info!(
        "media proxy request: media_type={} has_range={} url={}",
        query.media_type.as_deref().unwrap_or(""),
        request_headers.get("range").is_some(),
        query.url.chars().take(160).collect::<String>()
    );

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
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .danger_accept_invalid_certs(false)
        .build();

    let client = match client {
        Ok(client) => client,
        Err(error) => {
            log::error!("media proxy client init failed: {}", error);
            return build_error_response(StatusCode::BAD_GATEWAY, "Proxy error");
        }
    };

    let mut upstream = client
        .get(&query.url)
        .header("User-Agent", get_user_agent())
        .header("Referer", "https://www.douyin.com/")
        .header("Accept", "*/*")
        .header("Accept-Encoding", "identity;q=1, *;q=0");

    if !config.cookie.is_empty() {
        upstream = upstream.header("Cookie", config.cookie);
    }

    if let Some(range) = request_headers.get("range").cloned() {
        upstream = upstream.header("Range", range);
    }

    let upstream_response = match upstream.send().await {
        Ok(response) => response,
        Err(error) => {
            log::error!("media proxy upstream request failed: {}", error);
            return build_error_response(StatusCode::BAD_GATEWAY, "Proxy error");
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

    let requested_media_type = query.media_type.clone().unwrap_or_default().to_lowercase();
    let is_media = requested_media_type == "audio"
        || requested_media_type == "video"
        || upstream_content_type.to_lowercase().contains("video");

    if let Some(content_length) = upstream_response.headers().get("content-length") {
        let content_length_str = content_length.to_str().unwrap_or_default();
        if !content_length_str.is_empty() {
            match content_length_str.parse::<u64>() {
                Ok(length) => {
                    if length < 2 * 1024 * 1024 || !is_media {
                        response_headers.insert(axum::http::header::CONTENT_LENGTH, content_length.clone());
                    }
                }
                Err(_) => {
                    response_headers.insert(axum::http::header::CONTENT_LENGTH, content_length.clone());
                }
            }
        }
    }

    if let Some(content_type) = guess_content_type(&query.url, &upstream_content_type, &requested_media_type) {
        response_headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static(content_type),
        );
    }

    if (requested_media_type == "audio" || requested_media_type == "video")
        && !response_headers.contains_key(axum::http::header::ACCEPT_RANGES)
    {
        response_headers.insert(
            axum::http::header::ACCEPT_RANGES,
            HeaderValue::from_static("bytes"),
        );
    }

    response_headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response_headers.insert(
        axum::http::header::CACHE_CONTROL,
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

    log::info!("media proxy listening on http://{}", addr);

    tokio::spawn(async move {
        let app = Router::new()
            .route("/api/media/proxy", get(media_proxy))
            .with_state(state);

        if let Err(error) = axum::serve(listener, app).await {
            log::error!("media proxy server failed: {}", error);
        }
    });

    Ok(())
}
