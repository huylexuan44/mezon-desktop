use anyhow::{Context, Result};
use mezon_proto::api;
use prost::Message as _;

pub const INBOX_PAGE_LIMIT: i32 = 50;
pub const DIRECTION_BEFORE_TIMESTAMP: i32 = 3;
pub const DIRECTION_AROUND_TIMESTAMP: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum InboxCategory {
    Mentions = 1,
    Messages = 2,
    ForYou = 3,
}

impl InboxCategory {
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            1 => Some(Self::Mentions),
            2 => Some(Self::Messages),
            3 => Some(Self::ForYou),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxMessagePreview {
    pub message_id: String,
    pub channel_id: String,
    pub clan_id: String,
    pub sender_id: String,
    pub content: String,
    pub avatar: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxNotification {
    pub id: String,
    pub category: InboxCategory,
    pub subject: String,
    pub sender_id: String,
    pub clan_id: String,
    pub channel_id: String,
    pub topic_id: Option<String>,
    pub channel_type: i32,
    pub avatar_url: String,
    pub create_time_seconds: u32,
    pub code: i32,
    pub message: Option<InboxMessagePreview>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicDiscussion {
    pub id: String,
    pub message_id: String,
    pub clan_id: String,
    pub channel_id: String,
    pub content: String,
    pub last_message_preview: String,
    pub last_message_timestamp: u32,
}

fn id_str(value: i64) -> String {
    value.to_string()
}

fn optional_id_str(value: i64) -> Option<String> {
    if value == 0 {
        None
    } else {
        Some(value.to_string())
    }
}

pub fn display_text_from_message_content(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Some(text) = value.get("t").and_then(|v| v.as_str())
    {
        return text.to_string();
    }
    trimmed.to_string()
}

pub fn message_content_is_attachment(content: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(content.trim())
        .ok()
        .is_some_and(|value| {
            value.get("embed").is_some()
                && value
                    .get("t")
                    .and_then(|t| t.as_str())
                    .is_none_or(str::is_empty)
        })
}

fn parse_notification_content(bytes: &[u8]) -> Option<InboxMessagePreview> {
    if bytes.is_empty() {
        return None;
    }
    let text = std::str::from_utf8(bytes).unwrap_or("");
    let first = bytes[0];
    if first == b'[' || first == b'{' {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
            if value.get("message_id").is_some() || value.get("sender_id").is_some() {
                return parse_message_preview_json(bytes);
            }
            let content = display_text_from_message_content(text);
            if !content.is_empty() || message_content_is_attachment(text) {
                return Some(InboxMessagePreview {
                    message_id: String::new(),
                    channel_id: String::new(),
                    clan_id: String::new(),
                    sender_id: String::new(),
                    content,
                    avatar: String::new(),
                });
            }
        }
        return parse_message_preview_json(bytes);
    }
    if let Ok(fcm) = api::DirectFcmProto::decode(bytes) {
        return Some(InboxMessagePreview {
            message_id: id_str(fcm.message_id),
            channel_id: id_str(fcm.channel_id),
            clan_id: id_str(fcm.clan_id),
            sender_id: id_str(fcm.sender_id),
            content: display_text_from_message_content(&fcm.content),
            avatar: fcm.avatar,
        });
    }
    parse_message_preview_json(bytes)
}

fn parse_message_preview_json(bytes: &[u8]) -> Option<InboxMessagePreview> {
    #[derive(serde::Deserialize)]
    struct Raw {
        #[serde(default)]
        message_id: serde_json::Value,
        #[serde(default)]
        channel_id: serde_json::Value,
        #[serde(default)]
        clan_id: serde_json::Value,
        #[serde(default)]
        sender_id: serde_json::Value,
        #[serde(default)]
        content: String,
        #[serde(default)]
        avatar: String,
    }

    fn json_id(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            _ => String::new(),
        }
    }

    let raw: Raw = serde_json::from_slice(bytes).ok()?;
    Some(InboxMessagePreview {
        message_id: json_id(&raw.message_id),
        channel_id: json_id(&raw.channel_id),
        clan_id: json_id(&raw.clan_id),
        sender_id: json_id(&raw.sender_id),
        content: display_text_from_message_content(&raw.content),
        avatar: raw.avatar,
    })
}

impl InboxNotification {
    pub fn preview_text(&self) -> String {
        if let Some(message) = &self.message
            && !message.content.is_empty()
        {
            return message.content.clone();
        }
        if !self.subject.is_empty() {
            return self.subject.clone();
        }
        String::new()
    }
}

impl TopicDiscussion {
    pub fn reply_preview_text(&self) -> String {
        let raw = if self.last_message_preview.is_empty() {
            self.content.as_str()
        } else {
            self.last_message_preview.as_str()
        };
        if message_content_is_attachment(raw) {
            return String::new();
        }
        let text = display_text_from_message_content(raw);
        if !text.is_empty() {
            return text;
        }
        display_text_from_message_content(&self.content)
    }

    pub fn reply_is_attachment(&self) -> bool {
        let raw = if self.last_message_preview.is_empty() {
            self.content.as_str()
        } else {
            self.last_message_preview.as_str()
        };
        message_content_is_attachment(raw)
    }
}

pub fn inbox_notification_from_api(n: api::Notification) -> Result<InboxNotification> {
    let category = InboxCategory::from_i32(n.category)
        .with_context(|| format!("unknown notification category {}", n.category))?;
    let mut message = parse_notification_content(&n.content);
    if message.is_none() && !n.content.is_empty() {
        let raw = std::str::from_utf8(&n.content).unwrap_or("");
        let content = display_text_from_message_content(raw);
        if !content.is_empty() || message_content_is_attachment(raw) {
            message = Some(InboxMessagePreview {
                message_id: String::new(),
                channel_id: id_str(n.channel_id),
                clan_id: id_str(n.clan_id),
                sender_id: id_str(n.sender_id),
                content,
                avatar: n.avatar_url.clone(),
            });
        }
    }
    if let Some(message) = message.as_mut() {
        if message.channel_id.is_empty() {
            message.channel_id = id_str(n.channel_id);
        }
        if message.clan_id.is_empty() {
            message.clan_id = id_str(n.clan_id);
        }
        if message.sender_id.is_empty() {
            message.sender_id = id_str(n.sender_id);
        }
        if message.avatar.is_empty() {
            message.avatar = n.avatar_url.clone();
        }
        if message.content.is_empty() {
            let raw = std::str::from_utf8(&n.content).unwrap_or("");
            message.content = display_text_from_message_content(raw);
        }
    }
    Ok(InboxNotification {
        id: id_str(n.id),
        category,
        subject: n.subject,
        sender_id: id_str(n.sender_id),
        clan_id: id_str(n.clan_id),
        channel_id: id_str(n.channel_id),
        topic_id: optional_id_str(n.topic_id),
        channel_type: n.channel_type,
        avatar_url: n.avatar_url,
        create_time_seconds: n.create_time_seconds,
        code: n.code,
        message,
    })
}

pub fn inbox_notifications_from_list(
    list: api::NotificationList,
) -> Result<Vec<InboxNotification>> {
    list.notifications
        .into_iter()
        .map(inbox_notification_from_api)
        .collect()
}

pub fn topic_discussion_from_api(t: api::SdTopic) -> TopicDiscussion {
    let last_raw = t
        .last_sent_message
        .as_ref()
        .map(|m| m.content.as_str())
        .unwrap_or("");
    let last_message_preview = if message_content_is_attachment(last_raw) {
        String::new()
    } else {
        display_text_from_message_content(last_raw)
    };
    let last_message_timestamp = t
        .last_sent_message
        .as_ref()
        .map(|m| m.timestamp_seconds)
        .unwrap_or(t.update_time_seconds);
    TopicDiscussion {
        id: id_str(t.id),
        message_id: id_str(t.message_id),
        clan_id: id_str(t.clan_id),
        channel_id: id_str(t.channel_id),
        content: display_text_from_message_content(&t.content),
        last_message_preview,
        last_message_timestamp,
    }
}

pub fn topics_from_list(list: api::SdTopicList) -> Vec<TopicDiscussion> {
    list.topics
        .into_iter()
        .map(topic_discussion_from_api)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_from_i32() {
        assert_eq!(InboxCategory::from_i32(1), Some(InboxCategory::Mentions));
        assert_eq!(InboxCategory::from_i32(2), Some(InboxCategory::Messages));
        assert_eq!(InboxCategory::from_i32(3), Some(InboxCategory::ForYou));
        assert_eq!(InboxCategory::from_i32(99), None);
    }

    #[test]
    fn display_text_extracts_t_field() {
        assert_eq!(
            display_text_from_message_content(r#"{"t":"@user","mk":[]}"#),
            "@user"
        );
    }

    #[test]
    fn parse_mention_content_json() {
        let bytes = br#"{"t":"@gia.chuvan","mk":[{"s":0,"e":11,"type":"pre"}]}"#;
        let preview = parse_notification_content(bytes).expect("preview");
        assert_eq!(preview.content, "@gia.chuvan");
    }

    #[test]
    fn parse_json_content() {
        let bytes = br#"{"message_id":"42","channel_id":"7","clan_id":"1","sender_id":"9","content":"hello","avatar":"a.png"}"#;
        let preview = parse_notification_content(bytes).expect("preview");
        assert_eq!(preview.message_id, "42");
        assert_eq!(preview.content, "hello");
    }

    #[test]
    fn parse_invalid_content_returns_none_for_empty() {
        assert!(parse_notification_content(&[]).is_none());
    }

    #[test]
    fn topic_discussion_maps_fields() {
        let topic = topic_discussion_from_api(api::SdTopic {
            id: 10,
            message_id: 20,
            clan_id: 1,
            channel_id: 5,
            content: "topic title".into(),
            update_time_seconds: 100,
            ..Default::default()
        });
        assert_eq!(topic.id, "10");
        assert_eq!(topic.message_id, "20");
        assert_eq!(topic.content, "topic title");
    }
}
