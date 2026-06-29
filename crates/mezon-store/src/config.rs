//! Runtime configuration loaded from environment variables (typically via `.env`).
//!
//! Variable names match the legacy Electron desktop app (`NX_*` prefix) for parity.
//! Values are read at startup and are not persisted to `settings.json`.

use gpui::{App, Global};
use std::sync::Arc;
// No `Debug` derive: AppConfig holds secrets (api_key, imgproxy_key, fcm/tenor/treasury keys,
// webrtc credential). Deny `{:?}` so they can't leak into logs; log specific non-secret fields.
#[derive(Clone)]
pub struct AppConfig {
    // ── REST API (bootstrap, pre-auth) ──────────────────────────────────────
    pub api_host: String,
    pub api_port: u16,
    pub api_secure: bool,
    pub api_key: String,
    pub api_gw_host: String,
    pub api_gw_port: u16,

    // ── WebSocket / streaming ─────────────────────────────────────────────────
    pub tcp_port: Option<u16>,
    pub stream_ws_url: String,
    pub meet_ws_url: String,
    pub notification_ws_url: String,

    // ── OAuth2 ────────────────────────────────────────────────────────────────
    pub oauth2_authorize_url: String,
    pub oauth2_client_id: String,
    pub oauth2_redirect_uri: String,
    pub oauth2_response_type: String,
    pub oauth2_scope: String,
    pub oauth2_code_challenge_method: String,
    pub oauth2_log_out: String,
    pub oauth2_log_out_callback: String,
    pub google_client_id: String,

    // ── CDN / media ───────────────────────────────────────────────────────────
    pub domain_url: String,
    pub redirect_uri: String,
    pub logo_mezon: String,
    pub base_img_url: String,
    pub profile_img_url: String,
    pub imgproxy_base_url: String,
    pub imgproxy_key: String,

    // ── Tenor (GIF search) ────────────────────────────────────────────────────
    pub tenor_key: String,
    pub tenor_url_categories: String,
    pub tenor_url_search: String,
    pub tenor_url_featured: String,

    // ── Treasury / blockchain ─────────────────────────────────────────────────
    pub mezon_treasury_url: String,
    pub mezon_treasury_key: String,
    pub contract_address: String,
    pub mezon_treasury_url_network: String,

    // ── WebRTC (voice/video) ──────────────────────────────────────────────────
    pub webrtc_ice_servers_url: String,
    pub webrtc_ice_servers_username: String,
    pub webrtc_ice_servers_credential: String,

    // ── Firebase / FCM ────────────────────────────────────────────────────────
    pub fcm_api_key: String,
    pub fcm_auth_domain: String,
    pub fcm_project_id: String,
    pub fcm_storage_bucket: String,
    pub fcm_messaging_sender_id: String,
    pub fcm_app_id: String,
    pub fcm_measurement_id: String,
    pub fcm_vapid_key: String,

    // ── Misc ──────────────────────────────────────────────────────────────────
    pub api_client_key_custom: String,
    pub sentry_dsn: String,
    pub anonymous_user_id: String,
    pub max_length_name_allowed: u32,
    pub update_url: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self::dev_defaults()
    }
}

impl AppConfig {
    /// Development defaults (matches pre-env hardcoded values).
    pub fn dev_defaults() -> Self {
        Self {
            api_host: "dev-mezon.nccsoft.vn".into(),
            api_port: 8088,
            api_secure: true,
            api_key: "defaultkey".into(),
            api_gw_host: "dev-mezon.nccsoft.vn".into(),
            api_gw_port: 8088,

            tcp_port: Some(7349),
            stream_ws_url: "wss://stn.nccsoft.vn".into(),
            meet_ws_url: "wss://meet.nccsoft.vn".into(),
            notification_ws_url: "wss://gotify.mezon.ai".into(),

            oauth2_authorize_url: "https://oauth2.mezon.ai/oauth2/auth".into(),
            oauth2_client_id: "f049f29e-12a9-464c-938f-0a2f60c3210b".into(),
            oauth2_redirect_uri: "https://dev-mezon.nccsoft.vn/login/callback".into(),
            oauth2_response_type: "code".into(),
            oauth2_scope: "openid+offline".into(),
            oauth2_code_challenge_method: "S256".into(),
            oauth2_log_out: "https://oauth2.mezon.ai/oauth2/sessions/logout".into(),
            oauth2_log_out_callback: "https://mezon.ai/logout/callback".into(),
            google_client_id:
                "391688022389-1k9kb377ea6dccpqii7m5pifjj0agsjc.apps.googleusercontent.com".into(),

            domain_url: "https://mezon.ai".into(),
            redirect_uri: "https://mezon.ai".into(),
            logo_mezon: "https://cdn.mezon.ai/images/mezon_logo.png".into(),
            base_img_url: "https://cdn.mezon.ai".into(),
            profile_img_url: "https://profile.mezon.ai".into(),
            imgproxy_base_url: "https://dev-imgproxy.nccsoft.vn".into(),
            imgproxy_key: "_AEhOrrckkG-NjqIdVLtzc-dtLFuE4u6ClM0P46ICEY".into(),

            tenor_key: String::new(),
            tenor_url_categories: "https://tenor.googleapis.com/v2/categories?key=".into(),
            tenor_url_search: "https://tenor.googleapis.com/v2/search?q=".into(),
            tenor_url_featured: "https://tenor.googleapis.com/v2/featured?key=".into(),

            mezon_treasury_url: "https://withdraw-api.nccsoft.vn".into(),
            mezon_treasury_key: String::new(),
            contract_address: String::new(),
            mezon_treasury_url_network: "https://polygonscan.com".into(),

            webrtc_ice_servers_url: "turn:relay.mezon.vn:5349".into(),
            webrtc_ice_servers_username: "turnmezon".into(),
            webrtc_ice_servers_credential: String::new(),

            fcm_api_key: String::new(),
            fcm_auth_domain: "mezon-772fa.firebaseapp.com".into(),
            fcm_project_id: "mezon-772fa".into(),
            fcm_storage_bucket: "mezon-772fa.appspot.com".into(),
            fcm_messaging_sender_id: "285548761692".into(),
            fcm_app_id: String::new(),
            fcm_measurement_id: String::new(),
            fcm_vapid_key: String::new(),

            api_client_key_custom: "mezon.ai".into(),
            sentry_dsn: String::new(),
            anonymous_user_id: String::new(),
            max_length_name_allowed: 64,
            update_url: "https://cdn.mezon.ai/release/".into(),
        }
    }

    // pub fn prod_defaults() -> Self {
    //     Self {
    //     }
    // }

    /// Load configuration from environment variables, falling back to [`dev_defaults`].
    pub fn from_env() -> Self {
        let defaults = Self::dev_defaults();
        Self {
            api_host: opt_str(option_env!("NX_CHAT_APP_API_HOST"), &defaults.api_host),
            api_port: opt_u16(option_env!("NX_CHAT_APP_API_PORT"), defaults.api_port),
            api_secure: opt_bool(option_env!("NX_CHAT_APP_API_SECURE"), defaults.api_secure),
            api_key: opt_str(option_env!("NX_CHAT_APP_API_KEY"), &defaults.api_key),
            api_gw_host: opt_str(
                option_env!("NX_CHAT_APP_API_GW_HOST"),
                &defaults.api_gw_host,
            ),
            api_gw_port: opt_u16(option_env!("NX_CHAT_APP_API_GW_PORT"), defaults.api_gw_port),

            tcp_port: opt_opt_u16(option_env!("NX_CHAT_APP_TCP_PORT")),
            stream_ws_url: opt_str(
                option_env!("NX_CHAT_APP_STREAM_WS_URL"),
                &defaults.stream_ws_url,
            ),
            meet_ws_url: opt_str(
                option_env!("NX_CHAT_APP_MEET_WS_URL"),
                &defaults.meet_ws_url,
            ),
            notification_ws_url: opt_str(
                option_env!("NX_CHAT_APP_NOTIFICATION_WS_URL"),
                &defaults.notification_ws_url,
            ),

            oauth2_authorize_url: opt_str(
                option_env!("NX_CHAT_APP_OAUTH2_AUTHORIZE_URL"),
                &defaults.oauth2_authorize_url,
            ),
            oauth2_client_id: opt_str(
                option_env!("NX_CHAT_APP_OAUTH2_CLIENT_ID"),
                &defaults.oauth2_client_id,
            ),
            oauth2_redirect_uri: opt_str(
                option_env!("NX_CHAT_APP_OAUTH2_REDIRECT_URI"),
                &defaults.oauth2_redirect_uri,
            ),
            oauth2_response_type: opt_str(
                option_env!("NX_CHAT_APP_OAUTH2_RESPONSE_TYPE"),
                &defaults.oauth2_response_type,
            ),
            oauth2_scope: opt_str(
                option_env!("NX_CHAT_APP_OAUTH2_SCOPE"),
                &defaults.oauth2_scope,
            ),
            oauth2_code_challenge_method: opt_str(
                option_env!("NX_CHAT_APP_OAUTH2_CODE_CHALLENGE_METHOD"),
                &defaults.oauth2_code_challenge_method,
            ),
            oauth2_log_out: opt_str(
                option_env!("NX_CHAT_APP_OAUTH2_LOG_OUT"),
                &defaults.oauth2_log_out,
            ),
            oauth2_log_out_callback: opt_str(
                option_env!("NX_CHAT_APP_OAUTH2_LOG_OUT_CALLBACK"),
                &defaults.oauth2_log_out_callback,
            ),
            google_client_id: opt_str(
                option_env!("NX_CHAT_APP_GOOGLE_CLIENT_ID"),
                &defaults.google_client_id,
            ),

            domain_url: opt_str(option_env!("NX_DOMAIN_URL"), &defaults.domain_url),
            redirect_uri: opt_str(
                option_env!("NX_CHAT_APP_REDIRECT_URI"),
                &defaults.redirect_uri,
            ),
            logo_mezon: opt_str(option_env!("NX_LOGO_MEZON"), &defaults.logo_mezon),
            base_img_url: opt_str(option_env!("NX_BASE_IMG_URL"), &defaults.base_img_url),
            profile_img_url: opt_str(option_env!("NX_PROFILE_IMG_URL"), &defaults.profile_img_url),
            imgproxy_base_url: opt_str(
                option_env!("NX_IMGPROXY_BASE_URL"),
                &defaults.imgproxy_base_url,
            ),
            imgproxy_key: opt_str(option_env!("NX_IMGPROXY_KEY"), &defaults.imgproxy_key),

            tenor_key: opt_str(
                option_env!("NX_CHAT_APP_API_TENOR_KEY"),
                &defaults.tenor_key,
            ),
            tenor_url_categories: opt_str(
                option_env!("NX_CHAT_APP_API_TENOR_URL_CATEGORIES"),
                &defaults.tenor_url_categories,
            ),
            tenor_url_search: opt_str(
                option_env!("NX_CHAT_APP_API_TENOR_URL_SEARCH"),
                &defaults.tenor_url_search,
            ),
            tenor_url_featured: opt_str(
                option_env!("NX_CHAT_APP_API_TENOR_URL_FEATURED"),
                &defaults.tenor_url_featured,
            ),

            mezon_treasury_url: opt_str(
                option_env!("NX_CHAT_APP_MEZON_TREASURY_URL"),
                &defaults.mezon_treasury_url,
            ),
            mezon_treasury_key: opt_str(
                option_env!("NX_CHAT_APP_API_MEZONTREASURY_KEY"),
                &defaults.mezon_treasury_key,
            ),
            contract_address: opt_str(
                option_env!("NX_CHAT_APP_CONTRACT_ADDRESS"),
                &defaults.contract_address,
            ),
            mezon_treasury_url_network: opt_str(
                option_env!("NX_CHAT_APP_MEZON_TREASURY_URL_NETWORK"),
                &defaults.mezon_treasury_url_network,
            ),

            webrtc_ice_servers_url: opt_str(
                option_env!("NX_WEBRTC_ICESERVERS_URL"),
                &defaults.webrtc_ice_servers_url,
            ),
            webrtc_ice_servers_username: opt_str(
                option_env!("NX_WEBRTC_ICESERVERS_USERNAME"),
                &defaults.webrtc_ice_servers_username,
            ),
            webrtc_ice_servers_credential: opt_str(
                option_env!("NX_WEBRTC_ICESERVERS_CREDENTIAL"),
                &defaults.webrtc_ice_servers_credential,
            ),

            fcm_api_key: opt_str(
                option_env!("NX_CHAT_APP_FCM_API_KEY"),
                &defaults.fcm_api_key,
            ),
            fcm_auth_domain: opt_str(
                option_env!("NX_CHAT_APP_FCM_AUTH_DOMAIN"),
                &defaults.fcm_auth_domain,
            ),
            fcm_project_id: opt_str(
                option_env!("NX_CHAT_APP_FCM_PROJECT_ID"),
                &defaults.fcm_project_id,
            ),
            fcm_storage_bucket: opt_str(
                option_env!("NX_CHAT_APP_FCM_STORAGE_BUCKET"),
                &defaults.fcm_storage_bucket,
            ),
            fcm_messaging_sender_id: opt_str(
                option_env!("NX_CHAT_APP_FCM_MESSAGING_SENDER_ID"),
                &defaults.fcm_messaging_sender_id,
            ),
            fcm_app_id: opt_str(option_env!("NX_CHAT_APP_FCM_APP_ID"), &defaults.fcm_app_id),
            fcm_measurement_id: opt_str(
                option_env!("NX_CHAT_APP_FCM_MEASUREMENT_ID"),
                &defaults.fcm_measurement_id,
            ),
            fcm_vapid_key: opt_str(
                option_env!("NX_CHAT_APP_FCM_VAPID_KEY"),
                &defaults.fcm_vapid_key,
            ),

            api_client_key_custom: opt_str(
                option_env!("NX_CHAT_APP_API_CLIENT_KEY_CUSTOM"),
                &defaults.api_client_key_custom,
            ),
            sentry_dsn: opt_str(
                option_env!("NX_CHAT_SENTRY_DSN").or(option_env!("NX_CHAT_SENTRY_DNS")),
                &defaults.sentry_dsn,
            ),
            anonymous_user_id: opt_str(
                option_env!("NX_CHAT_APP_ANNONYMOUS_USER_ID"),
                &defaults.anonymous_user_id,
            ),
            max_length_name_allowed: opt_u32(
                option_env!("NX_MAX_LENGTH_NAME_ALLOWED"),
                defaults.max_length_name_allowed,
            ),
            update_url: opt_str(option_env!("NX_UPDATE_URL"), &defaults.update_url),
        }
    }

    /// REST client bootstrap host — mirrors `getMezonConfig()` in the web app
    /// (`NX_CHAT_APP_API_GW_HOST`, not `NX_CHAT_APP_API_HOST`).
    pub fn client_host(&self) -> &str {
        &self.api_gw_host
    }

    /// REST client bootstrap port — mirrors `getMezonConfig()` in the web app.
    pub fn client_port(&self) -> u16 {
        self.api_gw_port
    }

    pub fn init_global(config: Arc<AppConfig>, cx: &mut App) {
        cx.set_global(GlobalAppConfig(config));
    }

    pub fn global(cx: &App) -> &AppConfig {
        cx.global::<GlobalAppConfig>().0.as_ref()
    }

    pub fn try_global(cx: &App) -> Option<&AppConfig> {
        cx.try_global::<GlobalAppConfig>().map(|g| g.0.as_ref())
    }

    pub fn is_dev_imgproxy(&self) -> bool {
        self.imgproxy_base_url.contains("dev-imgproxy")
    }

    pub fn imgproxy_url(
        &self,
        source_image_url: &str,
        width: u32,
        height: u32,
        resize_type: &str,
    ) -> String {
        if source_image_url.is_empty() {
            return String::new();
        }
        if !source_image_url.starts_with("https://cdn.mezon")
            && !source_image_url.starts_with("https://profile.mezon")
        {
            return source_image_url.to_string();
        }
        if self.is_dev_imgproxy() {
            return source_image_url.to_string();
        }
        let processing_options = format!("rs:{}:{}:{}:1/mb:2097152", resize_type, width, height);
        let path = format!("/{}/plain/{}@webp", processing_options, source_image_url);
        let base = self.imgproxy_base_url.trim_end_matches('/');
        format!("{}/{}{}", base, self.imgproxy_key, path)
    }

    pub fn avatar_proxy(&self, source: &str) -> String {
        self.imgproxy_url(source, 100, 100, "fill")
    }

    pub fn profile_proxy(&self, source: &str) -> String {
        self.imgproxy_url(source, 300, 300, "fit")
    }

    pub fn attachment_proxy(
        &self,
        source: &str,
        real_width: u32,
        real_height: u32,
    ) -> (String, f32, f32) {
        let (display_w, display_h) = attachment_display_dimensions(real_width, real_height);
        if source.is_empty() {
            return (String::new(), display_w, display_h);
        }
        let proxy_w = (display_w * 2.0).ceil().max(1.0) as u32;
        let proxy_h = (display_h * 2.0).ceil().max(1.0) as u32;
        let resize = if real_width == 0 || real_height == 0 {
            "fill"
        } else if real_width < proxy_w || real_height < proxy_h {
            "fill-down"
        } else {
            "fill"
        };
        (
            self.imgproxy_url(source, proxy_w, proxy_h, resize),
            display_w,
            display_h,
        )
    }
}

pub const ATTACHMENT_MAX_W: f32 = 400.0;
pub const ATTACHMENT_MAX_H: f32 = 300.0;

pub fn attachment_display_dimensions(real_width: u32, real_height: u32) -> (f32, f32) {
    let w = real_width as f32;
    let h = real_height as f32;
    if w <= 0.0 || h <= 0.0 {
        return (280.0, 150.0);
    }
    let scale = (ATTACHMENT_MAX_W / w).min(ATTACHMENT_MAX_H / h).min(1.0);
    (w * scale, h * scale)
}

struct GlobalAppConfig(Arc<AppConfig>);
impl Global for GlobalAppConfig {}

fn normalize(value: Option<&'static str>) -> Option<&'static str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn opt_str(value: Option<&'static str>, default: &str) -> String {
    normalize(value)
        .map(str::to_owned)
        .unwrap_or_else(|| default.to_owned())
}

fn opt_u16(value: Option<&'static str>, default: u16) -> u16 {
    normalize(value)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn opt_opt_u16(value: Option<&'static str>) -> Option<u16> {
    normalize(value).and_then(|v| v.parse().ok())
}

fn opt_u32(value: Option<&'static str>, default: u32) -> u32 {
    normalize(value)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn opt_bool(value: Option<&'static str>, default: bool) -> bool {
    match normalize(value) {
        Some(v) => matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes"),
        None => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_defaults_match_legacy_constants() {
        let cfg = AppConfig::dev_defaults();
        assert_eq!(cfg.api_host, "api.mezon.ai");
        assert_eq!(cfg.api_port, 443);
        assert!(cfg.api_secure);
        assert_eq!(cfg.tcp_port, None);
        assert_eq!(cfg.client_host(), "gw.mezon.ai");
        assert_eq!(cfg.client_port(), 443);
    }

    #[test]
    fn dev_defaults_use_dev_tcp_port() {
        let cfg = AppConfig::dev_defaults();
        assert_eq!(cfg.tcp_port, Some(7349));
        assert_eq!(cfg.client_host(), "dev-mezon.nccsoft.vn");
    }

    #[test]
    fn opt_helpers_fall_back_when_unset_or_blank() {
        assert_eq!(opt_str(None, "def"), "def");
        assert_eq!(opt_str(Some("  "), "def"), "def");
        assert_eq!(opt_str(Some(" val "), "def"), "val");
        assert_eq!(opt_u16(None, 8088), 8088);
        assert_eq!(opt_u16(Some("443"), 8088), 443);
        assert_eq!(opt_u16(Some("nope"), 8088), 8088);
        assert_eq!(opt_u32(Some("128"), 64), 128);
        assert_eq!(opt_opt_u16(None), None);
        assert_eq!(opt_opt_u16(Some("7349")), Some(7349));
        assert!(opt_bool(Some("true"), false));
        assert!(opt_bool(Some("1"), false));
        assert!(!opt_bool(None, false));
    }

    #[test]
    fn imgproxy_url_rewrites_cdn_urls() {
        let cfg = AppConfig {
            imgproxy_base_url: "https://imgproxy.example".into(),
            imgproxy_key: "sig".into(),
            ..AppConfig::dev_defaults()
        };
        let src = "https://cdn.mezon.ai/images/avatar.png";
        let out = cfg.imgproxy_url(src, 100, 100, "fit");
        assert!(out.starts_with("https://imgproxy.example/sig/rs:fit:100:100:1/mb:2097152/plain/"));
        assert!(out.ends_with("@webp"));
        assert!(out.contains(src));
    }

    #[test]
    fn imgproxy_url_skips_external_urls() {
        let cfg = AppConfig::dev_defaults();
        let src = "https://example.com/avatar.png";
        assert_eq!(cfg.imgproxy_url(src, 100, 100, "fit"), src);
    }

    #[test]
    fn imgproxy_url_skips_dev_proxy() {
        let cfg = AppConfig::dev_defaults();
        let src = "https://cdn.mezon.ai/images/avatar.png";
        assert_eq!(cfg.imgproxy_url(src, 100, 100, "fit"), src);
    }

    #[test]
    fn imgproxy_url_empty_returns_empty() {
        let cfg = AppConfig::dev_defaults();
        assert_eq!(cfg.imgproxy_url("", 100, 100, "fit"), "");
    }
}
