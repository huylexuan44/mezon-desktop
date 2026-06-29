//! Message domain model — the native counterpart of React's `messages.slice`
//! message entity and its derived helpers. Kept separate from `channel.rs`
//! (which mirrors `channel.slice`) so message logic does not leak into the
//! channel module.

use gpui::SharedString;
use mezon_client::transport::{ApiMessageContent, ApiMessageReaction, ContentToken};

use crate::ids::{MessageId, UserId};

#[derive(Debug, Clone, Default)]
pub struct MessageAttachment {
    pub url: String,
    pub filename: String,
    pub filetype: String,
    pub width: u32,
    pub height: u32,
    pub proxied_src: SharedString,
    pub display_width: f32,
    pub display_height: f32,
}

impl MessageAttachment {
    pub fn is_image(&self) -> bool {
        self.filetype.starts_with("image/")
            || matches!(
                self.url
                    .split(['?', '#'])
                    .next()
                    .and_then(|u| u.rsplit('.').next())
                    .map(|ext| ext.to_ascii_lowercase())
                    .as_deref(),
                Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "avif")
            )
    }
}

/// Message type/category, mirroring React `TypeMessage`. Drives how a row is
/// rendered by the UI dispatcher (normal chat vs system vs welcome, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageCode {
    Chat,
    ChatUpdate,
    ChatRemove,
    Typing,
    Indicator,
    Welcome,
    CreateThread,
    CreatePin,
    MessageBuzz,
    Topic,
    AuditLog,
    SendToken,
    Ephemeral,
    UpcomingEvent,
    UpdateEphemeralMsg,
    DeleteEphemeralMsg,
    ShareContact,
    Location,
    Poll,
    Unknown(i32),
}

impl MessageCode {
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            0 => MessageCode::Chat,
            1 => MessageCode::ChatUpdate,
            2 => MessageCode::ChatRemove,
            3 => MessageCode::Typing,
            4 => MessageCode::Indicator,
            5 => MessageCode::Welcome,
            6 => MessageCode::CreateThread,
            7 => MessageCode::CreatePin,
            8 => MessageCode::MessageBuzz,
            9 => MessageCode::Topic,
            10 => MessageCode::AuditLog,
            11 => MessageCode::SendToken,
            12 => MessageCode::Ephemeral,
            13 => MessageCode::UpcomingEvent,
            14 => MessageCode::UpdateEphemeralMsg,
            15 => MessageCode::DeleteEphemeralMsg,
            16 => MessageCode::ShareContact,
            17 => MessageCode::Location,
            18 => MessageCode::Poll,
            other => MessageCode::Unknown(other),
        }
    }

    /// True for codes rendered by `MessageWithSystem` in React (icon + text row).
    pub fn is_system(self) -> bool {
        matches!(
            self,
            MessageCode::Welcome
                | MessageCode::UpcomingEvent
                | MessageCode::CreateThread
                | MessageCode::CreatePin
                | MessageCode::AuditLog
        )
    }
}

/// A reply/reference shown above a message ("replying to …").
#[derive(Debug, Clone, Default)]
pub struct MessageReference {
    pub message_ref_id: MessageId,
    pub sender_id: UserId,
    pub sender_name: String,
    pub sender_avatar: String,
    pub content: String,
    pub has_attachment: bool,
}

/// An aggregated emoji reaction (grouped by emoji across all reacting users).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reaction {
    /// Aggregation key (emoji id when present, else shortname).
    pub key: String,
    pub emoji: String,
    pub emoji_id: String,
    pub count: u32,
    pub sender_ids: Vec<String>,
}

/// An inline rich-text span produced from a message's content tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageSpan {
    Text(String),
    Bold(String),
    Code(String),
    CodeBlock {
        language: Option<String>,
        text: String,
    },
    Link {
        text: String,
        url: String,
    },
    Mention {
        display: String,
        user_id: Option<String>,
        role_id: Option<String>,
    },
    Hashtag {
        display: String,
        channel_id: Option<String>,
    },
    Emoji {
        name: String,
        emoji_id: String,
    },
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: MessageId,
    pub content: String,
    pub sender_id: String,
    pub sender_user_id: Option<UserId>,
    pub sender_name: String,
    pub avatar_url: String,
    pub avatar_proxied: SharedString,
    pub create_time: i64,
    pub update_time: i64,
    pub timestamp_label: String,
    pub day_label: String,
    pub code: MessageCode,
    pub is_edited: bool,
    pub is_forwarded: bool,
    pub combined_with_prev: bool,
    pub spans: Vec<MessageSpan>,
    pub references: Vec<MessageReference>,
    pub reactions: Vec<Reaction>,
    pub attachments: Vec<MessageAttachment>,
}

pub const COMBINE_TIME_WINDOW: i64 = 600;

pub fn message_combined_with_prev(prev: Option<&Message>, msg: &Message) -> bool {
    if msg.code != MessageCode::Chat || !msg.references.is_empty() {
        return false;
    }
    match prev {
        Some(prev) => {
            prev.code == MessageCode::Chat
                && prev.sender_id == msg.sender_id
                && prev.day_label == msg.day_label
                && (msg.create_time - prev.create_time).abs() < COMBINE_TIME_WINDOW
        }
        None => false,
    }
}

/// Aggregate raw per-user reaction entries into per-emoji totals (cf. React
/// `combineMessageReactions`).
pub fn aggregate_reactions(raw: &[ApiMessageReaction]) -> Vec<Reaction> {
    let mut out: Vec<Reaction> = Vec::new();
    for r in raw {
        if r.action {
            continue;
        }
        let key = if !r.emoji_id.is_empty() && r.emoji_id != "0" {
            r.emoji_id.clone()
        } else {
            r.emoji.clone()
        };
        if key.is_empty() {
            continue;
        }
        let add = if r.count == 0 { 1 } else { r.count };
        if let Some(slot) = out.iter_mut().find(|x| x.key == key) {
            slot.count = slot.count.saturating_add(add);
            if !r.sender_id.is_empty() && !slot.sender_ids.contains(&r.sender_id) {
                slot.sender_ids.push(r.sender_id.clone());
            }
        } else {
            out.push(Reaction {
                key,
                emoji: r.emoji.clone(),
                emoji_id: r.emoji_id.clone(),
                count: add,
                sender_ids: if r.sender_id.is_empty() {
                    Vec::new()
                } else {
                    vec![r.sender_id.clone()]
                },
            });
        }
    }
    out.retain(|r| r.count > 0);
    out
}

/// Apply a single realtime reaction add/remove to a message's aggregated
/// reaction list (cf. React reaction socket handling). `removed` mirrors the
/// proto `action` flag.
pub fn apply_reaction_event(
    reactions: &mut Vec<Reaction>,
    emoji_id: &str,
    emoji: &str,
    sender_id: &str,
    removed: bool,
) {
    let key = if !emoji_id.is_empty() && emoji_id != "0" {
        emoji_id
    } else {
        emoji
    };
    if key.is_empty() {
        return;
    }
    if removed {
        if let Some(pos) = reactions.iter().position(|x| x.key == key) {
            let rec = &mut reactions[pos];
            rec.sender_ids.retain(|s| s != sender_id);
            rec.count = rec.count.saturating_sub(1);
            if rec.count == 0 || rec.sender_ids.is_empty() {
                reactions.remove(pos);
            }
        }
    } else if let Some(rec) = reactions.iter_mut().find(|x| x.key == key) {
        if sender_id.is_empty() {
            rec.count += 1;
        } else if !rec.sender_ids.iter().any(|s| s == sender_id) {
            rec.sender_ids.push(sender_id.to_string());
            rec.count += 1;
        }
    } else {
        reactions.push(Reaction {
            key: key.to_string(),
            emoji: emoji.to_string(),
            emoji_id: emoji_id.to_string(),
            count: 1,
            sender_ids: if sender_id.is_empty() {
                Vec::new()
            } else {
                vec![sender_id.to_string()]
            },
        });
    }
}

/// Parse a message's content tokens into ordered, non-overlapping inline spans
/// (mirrors React `MessageLine` token interleaving).
pub fn parse_spans(content: &ApiMessageContent) -> Vec<MessageSpan> {
    let text = &content.t;
    if text.is_empty() {
        return Vec::new();
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let total = units.len() as i64;
    let slice = |s: i64, e: i64| -> String {
        let s = s.clamp(0, total) as usize;
        let e = e.clamp(0, total) as usize;
        if e <= s {
            return String::new();
        }
        String::from_utf16_lossy(&units[s..e])
    };

    #[derive(Clone, Copy)]
    enum Kind {
        Mention,
        Hashtag,
        Emoji,
        Markdown,
        Link,
    }
    let mut toks: Vec<(i64, i64, Kind, ContentToken)> = Vec::new();
    let collect =
        |list: &[ContentToken], kind: Kind, toks: &mut Vec<(i64, i64, Kind, ContentToken)>| {
            for t in list {
                let s = t.s.unwrap_or(0);
                let e = t.e.unwrap_or(0);
                if e > s {
                    toks.push((s, e, kind, t.clone()));
                }
            }
        };
    collect(&content.mentions, Kind::Mention, &mut toks);
    collect(&content.hg, Kind::Hashtag, &mut toks);
    collect(&content.ej, Kind::Emoji, &mut toks);
    collect(&content.mk, Kind::Markdown, &mut toks);
    collect(&content.lk, Kind::Link, &mut toks);
    toks.sort_by_key(|t| t.0);

    let mut spans: Vec<MessageSpan> = Vec::new();
    let mut last = 0i64;
    let mut prev_end = i64::MIN;
    for (s, e, kind, tok) in toks {
        if s < prev_end {
            continue;
        }
        if last < s {
            spans.push(MessageSpan::Text(slice(last, s)));
        }
        let inner = slice(s, e);
        match kind {
            Kind::Mention => {
                let is_user = tok
                    .user_id
                    .as_deref()
                    .is_some_and(|u| u != "0" && !u.is_empty())
                    || tok.username.is_some();
                if is_user {
                    spans.push(MessageSpan::Mention {
                        display: inner,
                        user_id: tok.user_id.clone(),
                        role_id: None,
                    });
                } else if tok.role_id.as_deref().is_some_and(|r| !r.is_empty()) {
                    spans.push(MessageSpan::Mention {
                        display: inner,
                        user_id: None,
                        role_id: tok.role_id.clone(),
                    });
                } else {
                    spans.push(MessageSpan::Text(inner));
                }
            }
            Kind::Hashtag => spans.push(MessageSpan::Hashtag {
                display: inner,
                channel_id: tok.channel_id.clone(),
            }),
            Kind::Emoji => spans.push(MessageSpan::Emoji {
                name: inner,
                emoji_id: tok.emojiid.clone().unwrap_or_default(),
            }),
            Kind::Link => {
                let url = tok.url.clone().unwrap_or_else(|| inner.clone());
                spans.push(MessageSpan::Link { text: inner, url });
            }
            Kind::Markdown => {
                let ty = tok.kind.as_deref().unwrap_or("");
                match ty {
                    "b" => spans.push(MessageSpan::Bold(strip_marker(&inner, "**"))),
                    "c" | "s" => spans.push(MessageSpan::Code(strip_marker(&inner, "`"))),
                    "t" | "pre" => spans.push(MessageSpan::CodeBlock {
                        language: None,
                        text: strip_marker(&inner, "```"),
                    }),
                    "lk" | "lk_yt" | "lk_fb" | "lk_tt" => {
                        let url = tok.url.clone().unwrap_or_else(|| inner.clone());
                        spans.push(MessageSpan::Link { text: inner, url });
                    }
                    _ => spans.push(MessageSpan::Text(inner)),
                }
            }
        }
        prev_end = e;
        last = e;
    }
    if last < total {
        spans.push(MessageSpan::Text(slice(last, total)));
    }
    spans
}

fn strip_marker(s: &str, marker: &str) -> String {
    let trimmed = s
        .strip_prefix(marker)
        .and_then(|r| r.strip_suffix(marker))
        .unwrap_or(s);
    trimmed.to_string()
}

pub fn recompute_message_grouping(messages: &mut [Message]) {
    for i in 0..messages.len() {
        let prev = if i > 0 { Some(&messages[i - 1]) } else { None };
        messages[i].combined_with_prev = message_combined_with_prev(prev, &messages[i]);
    }
}

/// Ordering key mirroring React `orderMessageByIDAscending` (`messages.slice.ts`
/// `sortComparer`): the `FIRST_MESSAGE` sentinel (mapped to
/// [`MessageCode::Indicator`]) always sorts first, then by numeric (Snowflake)
/// id ascending. Snowflake ids are monotonic in time, so this is stable and
/// sub-second accurate — unlike `create_time`, which has only second
/// granularity and can mis-order (and pick the wrong newest/oldest anchor for
/// pagination). Optimistic ids occupy the high band (>= `MessageId::OPTIMISTIC_BASE`)
/// so they sort last — they are the just-sent, pending rows.
pub fn message_sort_key(m: &Message) -> (u8, i64) {
    let not_first = u8::from(m.code != MessageCode::Indicator);
    (not_first, m.id.get())
}

/// Sort a message buffer in place into React's id-ascending order.
pub fn sort_messages(messages: &mut [Message]) {
    messages.sort_by_key(message_sort_key);
}

impl Message {
    pub fn new(
        id: MessageId,
        content: impl Into<String>,
        sender_id: impl Into<String>,
        sender_name: impl Into<String>,
        create_time: i64,
    ) -> Self {
        let content: String = content.into();
        let spans = if content.is_empty() {
            Vec::new()
        } else {
            vec![MessageSpan::Text(content.clone())]
        };
        let sender_id: String = sender_id.into();
        let sender_user_id = sender_id.parse::<i64>().ok().map(UserId);
        Self {
            id,
            content,
            sender_id,
            sender_user_id,
            sender_name: sender_name.into(),
            avatar_url: String::new(),
            avatar_proxied: SharedString::default(),
            create_time,
            update_time: 0,
            timestamp_label: format_clock(create_time),
            day_label: format_day(create_time),
            code: MessageCode::Chat,
            is_edited: false,
            is_forwarded: false,
            combined_with_prev: false,
            spans,
            references: Vec::new(),
            reactions: Vec::new(),
            attachments: Vec::new(),
        }
    }

    pub fn with_attachments(mut self, attachments: Vec<MessageAttachment>) -> Self {
        self.attachments = attachments;
        self
    }

    pub fn with_code(mut self, code: MessageCode) -> Self {
        self.code = code;
        self
    }

    pub fn with_spans(mut self, spans: Vec<MessageSpan>) -> Self {
        self.spans = spans;
        self
    }

    pub fn with_references(mut self, references: Vec<MessageReference>) -> Self {
        self.references = references;
        self
    }

    pub fn with_reactions(mut self, reactions: Vec<Reaction>) -> Self {
        self.reactions = reactions;
        self
    }

    pub fn with_edited(mut self, update_time: i64, hide_editted: bool) -> Self {
        self.update_time = update_time;
        self.is_edited = update_time > 0 && update_time > self.create_time && !hide_editted;
        self
    }

    pub fn with_forwarded(mut self, forwarded: bool) -> Self {
        self.is_forwarded = forwarded;
        self
    }

    pub fn with_avatar(mut self, avatar_url: impl Into<String>) -> Self {
        self.avatar_url = avatar_url.into();
        self
    }

    pub fn with_avatar_proxied(mut self, proxied: impl Into<SharedString>) -> Self {
        self.avatar_proxied = proxied.into();
        self
    }
}

fn format_clock(ts: i64) -> String {
    let seconds_since_midnight = ts.rem_euclid(86_400);
    let hours = seconds_since_midnight / 3600;
    let minutes = (seconds_since_midnight % 3600) / 60;
    let period = if hours >= 12 { "PM" } else { "AM" };
    let display_hour = if hours == 0 {
        12
    } else if hours > 12 {
        hours - 12
    } else {
        hours
    };
    format!("{display_hour}:{minutes:02} {period}")
}

fn format_day(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.format("%B %d, %Y").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(s: i64, e: i64) -> ContentToken {
        ContentToken {
            s: Some(s),
            e: Some(e),
            ..Default::default()
        }
    }

    #[test]
    fn parse_spans_plain_text() {
        let content = ApiMessageContent {
            t: "hello world".into(),
            ..Default::default()
        };
        assert_eq!(
            parse_spans(&content),
            vec![MessageSpan::Text("hello world".into())]
        );
    }

    #[test]
    fn parse_spans_interleaves_mention_and_text() {
        let content = ApiMessageContent {
            t: "hi @bob !".into(),
            mentions: vec![ContentToken {
                user_id: Some("42".into()),
                username: Some("bob".into()),
                ..token(3, 7)
            }],
            ..Default::default()
        };
        let spans = parse_spans(&content);
        assert_eq!(
            spans,
            vec![
                MessageSpan::Text("hi ".into()),
                MessageSpan::Mention {
                    display: "@bob".into(),
                    user_id: Some("42".into()),
                    role_id: None,
                },
                MessageSpan::Text(" !".into()),
            ]
        );
    }

    #[test]
    fn parse_spans_strips_bold_markers() {
        let content = ApiMessageContent {
            t: "**hey**".into(),
            mk: vec![ContentToken {
                kind: Some("b".into()),
                ..token(0, 7)
            }],
            ..Default::default()
        };
        assert_eq!(parse_spans(&content), vec![MessageSpan::Bold("hey".into())]);
    }

    #[test]
    fn parse_spans_handles_utf16_indices() {
        let content = ApiMessageContent {
            t: "😀 @x".into(),
            mentions: vec![ContentToken {
                user_id: Some("1".into()),
                ..token(3, 5)
            }],
            ..Default::default()
        };
        let spans = parse_spans(&content);
        assert_eq!(
            spans.last(),
            Some(&MessageSpan::Mention {
                display: "@x".into(),
                user_id: Some("1".into()),
                role_id: None,
            })
        );
    }

    #[test]
    fn aggregate_reactions_groups_by_emoji() {
        let raw = vec![
            ApiMessageReaction {
                emoji_id: "10".into(),
                emoji: ":a:".into(),
                count: 1,
                sender_id: "u1".into(),
                action: false,
            },
            ApiMessageReaction {
                emoji_id: "10".into(),
                emoji: ":a:".into(),
                count: 1,
                sender_id: "u2".into(),
                action: false,
            },
            ApiMessageReaction {
                emoji_id: "20".into(),
                emoji: ":b:".into(),
                count: 1,
                sender_id: "u1".into(),
                action: true,
            },
        ];
        let agg = aggregate_reactions(&raw);
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].key, "10");
        assert_eq!(agg[0].count, 2);
        assert_eq!(agg[0].sender_ids, vec!["u1".to_string(), "u2".to_string()]);
    }

    #[test]
    fn system_message_never_combines() {
        let prev = Message::new(MessageId(1), "a", "u1", "U1", 100);
        let mut sys = Message::new(MessageId(2), "joined", "u1", "U1", 110);
        sys.code = MessageCode::Welcome;
        assert!(!message_combined_with_prev(Some(&prev), &sys));
    }

    #[test]
    fn message_precomputes_clock_and_day_labels() {
        let msg = Message::new(MessageId(1), "hi", "u", "User", 1_609_459_200 + 48_300);
        assert_eq!(msg.timestamp_label, "1:25 PM");
        assert_eq!(msg.day_label, "January 01, 2021");
    }

    #[test]
    fn message_clock_label_handles_midnight() {
        let msg = Message::new(MessageId(1), "hi", "u", "User", 1_609_459_200);
        assert_eq!(msg.timestamp_label, "12:00 AM");
    }

    #[test]
    fn sender_user_id_parsed_from_numeric_sender_id() {
        let msg = Message::new(MessageId(10), "hi", "42", "Alice", 0);
        assert_eq!(msg.sender_id, "42");
        assert_eq!(msg.sender_user_id, Some(UserId(42)));
    }

    #[test]
    fn sender_user_id_none_for_non_numeric_sender_id() {
        let msg = Message::new(MessageId::next_optimistic(), "hi", "u1", "Bob", 0);
        assert_eq!(msg.sender_id, "u1");
        assert_eq!(msg.sender_user_id, None);
    }

    #[test]
    fn sender_user_id_none_for_optimistic_temp_sender() {
        let msg = Message::new(
            MessageId::next_optimistic(),
            "hi",
            "temp-user",
            "Charlie",
            0,
        );
        assert_eq!(msg.sender_user_id, None);
    }

    #[test]
    fn optimistic_id_is_optimistic_real_id_is_not() {
        let opt = MessageId::next_optimistic();
        let real = MessageId(1_000_000_000_000_i64);
        assert!(opt.is_optimistic());
        assert!(!real.is_optimistic());
    }

    #[test]
    fn optimistic_ids_sort_after_real_ids() {
        let opt = MessageId::next_optimistic();
        let real = MessageId(i64::MAX / 2);
        assert!(real < opt);
    }

    #[test]
    fn optimistic_ids_are_unique_and_monotonic() {
        let a = MessageId::next_optimistic();
        let b = MessageId::next_optimistic();
        assert_ne!(a, b);
        assert!(a < b);
        assert!(a.is_optimistic() && b.is_optimistic());
    }

    #[test]
    fn cursor_guard_skips_optimistic_ids() {
        let optimistic = MessageId::next_optimistic();
        assert!(Some(optimistic).filter(|id| !id.is_optimistic()).is_none());
        let real = MessageId(123);
        assert_eq!(
            Some(real).filter(|id| !id.is_optimistic()),
            Some(MessageId(123))
        );
    }
}
