//! Cookie 工具模块

use crate::api::UserInfo;
use crate::config::AppConfig;
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::webview::Cookie;

#[derive(Clone)]
pub struct CookieLoginSession {
    pub label: String,
    pub cancelled: Arc<AtomicBool>,
}

pub fn serialize_cookie_string(cookies: &[Cookie<'static>]) -> String {
    cookies
        .iter()
        .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
        .collect::<Vec<_>>()
        .join("; ")
}

pub fn parse_cookie_string(cookie_string: &str) -> Vec<Cookie<'static>> {
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

pub fn has_douyin_login_cookie(cookies: &[Cookie<'static>]) -> bool {
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

pub async fn verify_douyin_login_cookie(
    config: &AppConfig,
    cookie: &str,
) -> Result<UserInfo, String> {
    use crate::api::DouyinClient;
    let mut next_config = config.clone();
    next_config.cookie = cookie.to_string();

    let client = DouyinClient::new(next_config)
        .map_err(|error| format!("创建 Cookie 校验客户端失败: {}", error))?;

    client
        .get_current_user()
        .await
        .map_err(|error| format!("登录态校验失败: {}", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_cookie_string() {
        use tauri::webview::Cookie;
        let cookies = vec![
            Cookie::parse("sessionid=abc; Domain=.example.com")
                .unwrap()
                .into_owned(),
            Cookie::parse("user=test; Domain=.example.com")
                .unwrap()
                .into_owned(),
        ];
        let cookie_str = serialize_cookie_string(&cookies);
        assert!(cookie_str.contains("sessionid=abc"));
        assert!(cookie_str.contains("user=test"));
    }

    #[test]
    fn checks_login_cookie_presence() {
        use tauri::webview::Cookie;

        let cookies = vec![Cookie::parse("sessionid=abc; Domain=.example.com")
            .unwrap()
            .into_owned()];
        assert!(has_douyin_login_cookie(&cookies));

        let cookies = vec![Cookie::parse("other=value; Domain=.example.com")
            .unwrap()
            .into_owned()];
        assert!(!has_douyin_login_cookie(&cookies));

        let cookies = vec![Cookie::parse("passport_auth_status=1; Domain=.example.com")
            .unwrap()
            .into_owned()];
        assert!(has_douyin_login_cookie(&cookies));
    }
}
