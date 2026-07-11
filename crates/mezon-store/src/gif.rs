use std::time::Duration;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, SharedString, Task};
use serde::Deserialize;

use crate::config::AppConfig;

const GIF_LIMIT: u32 = 30;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone)]
pub struct GifCategory {
    pub name: SharedString,
    pub searchterm: SharedString,
    pub image: SharedString,
}

#[derive(Debug, Clone)]
pub struct Gif {
    pub url: SharedString,
    pub preview_url: SharedString,
}

#[derive(Debug, Clone)]
pub enum GifEvent {
    Changed,
}

#[derive(Deserialize)]
struct TenorCategoriesResponse {
    #[serde(default)]
    tags: Vec<TenorCategory>,
}

#[derive(Deserialize)]
struct TenorCategory {
    #[serde(default)]
    name: String,
    #[serde(default)]
    searchterm: String,
    #[serde(default)]
    image: String,
}

#[derive(Deserialize)]
struct TenorGifsResponse {
    #[serde(default)]
    results: Vec<TenorGif>,
}

#[derive(Deserialize)]
struct TenorGif {
    #[serde(default)]
    media_formats: TenorMediaFormats,
}

#[derive(Deserialize, Default)]
struct TenorMediaFormats {
    gif: Option<TenorMedia>,
    tinygif: Option<TenorMedia>,
}

#[derive(Deserialize)]
struct TenorMedia {
    #[serde(default)]
    url: String,
}

struct TenorConfig {
    key: String,
    client_key: String,
    base_categories: String,
    base_search: String,
    base_featured: String,
}

impl TenorConfig {
    fn from_app(cx: &App) -> Self {
        let config = AppConfig::global(cx);
        Self {
            key: config.tenor_key.clone(),
            client_key: config.api_client_key_custom.clone(),
            base_categories: config.tenor_url_categories.clone(),
            base_search: config.tenor_url_search.clone(),
            base_featured: config.tenor_url_featured.clone(),
        }
    }

    fn categories_url(&self) -> String {
        format!(
            "{}{}&client_key={}&limit={GIF_LIMIT}",
            self.base_categories, self.key, self.client_key
        )
    }

    fn featured_url(&self) -> String {
        format!(
            "{}{}&client_key={}&limit={GIF_LIMIT}",
            self.base_featured, self.key, self.client_key
        )
    }

    fn search_url(&self, query: &str) -> String {
        format!(
            "{}{}&key={}&client_key={}&limit={GIF_LIMIT}",
            self.base_search,
            encode_query(query),
            self.key,
            self.client_key
        )
    }
}

pub struct GifStore {
    categories: Vec<GifCategory>,
    featured: Vec<Gif>,
    search_results: Vec<Gif>,
    loaded: bool,
    loading: bool,
    searching: bool,
    search_generation: u64,
    config: TenorConfig,
    _base_task: Option<Task<()>>,
}

struct GlobalGifStore(Entity<GifStore>);
impl Global for GlobalGifStore {}

impl EventEmitter<GifEvent> for GifStore {}

impl GifStore {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(Self::new);
        cx.set_global(GlobalGifStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalGifStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalGifStore>().map(|g| g.0.clone())
    }

    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            categories: Vec::new(),
            featured: Vec::new(),
            search_results: Vec::new(),
            loaded: false,
            loading: false,
            searching: false,
            search_generation: 0,
            config: TenorConfig::from_app(cx),
            _base_task: None,
        }
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.loaded && !self.loading {
            self.fetch_base(cx);
        }
    }

    fn fetch_base(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let categories_url = self.config.categories_url();
        let featured_url = self.config.featured_url();
        self._base_task = Some(cx.spawn(async move |this, cx| {
            let categories = fetch_categories(&categories_url).await;
            let featured = fetch_gifs(&featured_url).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                if let Some(categories) = categories {
                    this.categories = categories;
                }
                if let Some(featured) = featured {
                    this.featured = featured;
                }
                this.loaded = !this.categories.is_empty() || !this.featured.is_empty();
                cx.emit(GifEvent::Changed);
                cx.notify();
            });
        }));
    }

    pub fn search(&mut self, query: String, cx: &mut Context<Self>) {
        let trimmed = query.trim().to_string();
        self.search_generation = self.search_generation.wrapping_add(1);
        if trimmed.is_empty() {
            self.searching = false;
            if !self.search_results.is_empty() {
                self.search_results.clear();
                cx.emit(GifEvent::Changed);
            }
            cx.notify();
            return;
        }
        self.searching = true;
        cx.notify();
        let generation = self.search_generation;
        let url = self.config.search_url(&trimmed);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            if this
                .update(cx, |this, _| this.search_generation != generation)
                .unwrap_or(true)
            {
                return;
            }
            let results = fetch_gifs(&url).await;
            let _ = this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                this.searching = false;
                match results {
                    Some(results) => this.search_results = results,
                    None => this.search_results.clear(),
                }
                cx.emit(GifEvent::Changed);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn categories(&self) -> &[GifCategory] {
        &self.categories
    }

    pub fn featured(&self) -> &[Gif] {
        &self.featured
    }

    pub fn search_results(&self) -> &[Gif] {
        &self.search_results
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_searching(&self) -> bool {
        self.searching
    }
}

async fn fetch_categories(url: &str) -> Option<Vec<GifCategory>> {
    let bytes = match mezon_client::transport_runtime::fetch_bytes(url).await {
        Ok((bytes, _)) => bytes,
        Err(e) => {
            tracing::error!("tenor categories fetch failed: {e}");
            return None;
        }
    };
    match serde_json::from_slice::<TenorCategoriesResponse>(&bytes) {
        Ok(response) => Some(
            response
                .tags
                .into_iter()
                .filter_map(category_from_tenor)
                .collect(),
        ),
        Err(e) => {
            tracing::error!("tenor categories parse failed: {e}");
            None
        }
    }
}

async fn fetch_gifs(url: &str) -> Option<Vec<Gif>> {
    let bytes = match mezon_client::transport_runtime::fetch_bytes(url).await {
        Ok((bytes, _)) => bytes,
        Err(e) => {
            tracing::error!("tenor gifs fetch failed: {e}");
            return None;
        }
    };
    match serde_json::from_slice::<TenorGifsResponse>(&bytes) {
        Ok(response) => Some(
            response
                .results
                .into_iter()
                .filter_map(gif_from_tenor)
                .collect(),
        ),
        Err(e) => {
            tracing::error!("tenor gifs parse failed: {e}");
            None
        }
    }
}

fn category_from_tenor(category: TenorCategory) -> Option<GifCategory> {
    if category.searchterm.is_empty() && category.name.is_empty() {
        return None;
    }
    Some(GifCategory {
        name: category.name.into(),
        searchterm: category.searchterm.into(),
        image: category.image.into(),
    })
}

fn gif_from_tenor(gif: TenorGif) -> Option<Gif> {
    let url = gif.media_formats.gif?.url;
    if url.is_empty() {
        return None;
    }
    let preview = gif
        .media_formats
        .tinygif
        .map(|media| media.url)
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| url.clone());
    Some(Gif {
        url: url.into(),
        preview_url: preview.into(),
    })
}

fn encode_query(query: &str) -> String {
    let mut encoded = String::with_capacity(query.len());
    for byte in query.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> TenorConfig {
        TenorConfig {
            key: "KEY".into(),
            client_key: "mezon.ai".into(),
            base_categories: "https://tenor.googleapis.com/v2/categories?key=".into(),
            base_search: "https://tenor.googleapis.com/v2/search?q=".into(),
            base_featured: "https://tenor.googleapis.com/v2/featured?key=".into(),
        }
    }

    #[test]
    fn builds_tenor_urls_with_expected_params() {
        let config = config();
        assert_eq!(
            config.categories_url(),
            "https://tenor.googleapis.com/v2/categories?key=KEY&client_key=mezon.ai&limit=30"
        );
        assert_eq!(
            config.featured_url(),
            "https://tenor.googleapis.com/v2/featured?key=KEY&client_key=mezon.ai&limit=30"
        );
        assert_eq!(
            config.search_url("happy cat"),
            "https://tenor.googleapis.com/v2/search?q=happy%20cat&key=KEY&client_key=mezon.ai&limit=30"
        );
    }

    #[test]
    fn encodes_reserved_query_characters() {
        assert_eq!(encode_query("a b&c"), "a%20b%26c");
        assert_eq!(encode_query("plain-text_1.0~"), "plain-text_1.0~");
    }

    #[test]
    fn parses_gifs_and_prefers_tinygif_preview() {
        let json = r#"{"results":[
            {"media_formats":{"gif":{"url":"https://media.tenor.com/a.gif"},"tinygif":{"url":"https://media.tenor.com/a-tiny.gif"}}},
            {"media_formats":{"gif":{"url":"https://media.tenor.com/b.gif"}}},
            {"media_formats":{"tinygif":{"url":"https://media.tenor.com/c-tiny.gif"}}}
        ]}"#;
        let response: TenorGifsResponse = serde_json::from_str(json).unwrap();
        let gifs: Vec<Gif> = response
            .results
            .into_iter()
            .filter_map(gif_from_tenor)
            .collect();
        assert_eq!(gifs.len(), 2);
        assert_eq!(gifs[0].url, "https://media.tenor.com/a.gif");
        assert_eq!(gifs[0].preview_url, "https://media.tenor.com/a-tiny.gif");
        assert_eq!(gifs[1].url, "https://media.tenor.com/b.gif");
        assert_eq!(gifs[1].preview_url, "https://media.tenor.com/b.gif");
    }

    #[test]
    fn parses_categories_and_skips_empty() {
        let json = r##"{"tags":[
            {"name":"#excited","searchterm":"excited","image":"https://img/excited.gif"},
            {"name":"","searchterm":"","image":""}
        ]}"##;
        let response: TenorCategoriesResponse = serde_json::from_str(json).unwrap();
        let categories: Vec<GifCategory> = response
            .tags
            .into_iter()
            .filter_map(category_from_tenor)
            .collect();
        assert_eq!(categories.len(), 1);
        assert_eq!(categories[0].name, "#excited");
        assert_eq!(categories[0].searchterm, "excited");
    }
}
