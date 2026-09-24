use serde::{Deserialize, Serialize};

pub const CHANNEL_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BOT_AGENT: &str = "k-Coder";
pub const MAX_INBOUND_TEXT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BaseInfo {
    pub channel_version: &'static str,
    pub bot_agent: &'static str,
}

impl Default for BaseInfo {
    fn default() -> Self {
        Self {
            channel_version: CHANNEL_VERSION,
            bot_agent: BOT_AGENT,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GetUpdatesRequest {
    pub get_updates_buf: String,
    pub base_info: BaseInfo,
}

impl GetUpdatesRequest {
    pub fn new(get_updates_buf: String) -> Self {
        Self {
            get_updates_buf,
            base_info: BaseInfo::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetUpdatesResponse {
    pub ret: Option<i32>,
    pub errcode: Option<i32>,
    pub errmsg: Option<String>,
    pub msgs: Option<Vec<IlinkMessage>>,
    pub get_updates_buf: Option<String>,
    pub longpolling_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IlinkMessage {
    pub message_id: Option<u64>,
    pub from_user_id: Option<String>,
    pub to_user_id: Option<String>,
    pub session_id: Option<String>,
    pub group_id: Option<String>,
    pub message_type: Option<u8>,
    pub context_token: Option<String>,
    pub item_list: Option<Vec<MessageItem>>,
}

impl IlinkMessage {
    /// Return user-authored text DMs only. Groups, bot echoes, media, malformed senders,
    /// missing reply context, and unbounded text are ignored by the first integration slice.
    pub fn direct_text(&self) -> Option<String> {
        if self.message_type != Some(1)
            || self.group_id.as_deref().is_some_and(|value| !value.is_empty())
            || self.from_user_id.as_deref().is_none_or(str::is_empty)
            || self.session_id.as_deref().is_none_or(str::is_empty)
            || self.context_token.as_deref().is_none_or(str::is_empty)
        {
            return None;
        }

        let items = self.item_list.as_ref()?;
        if items.is_empty() || items.iter().any(|item| item.kind != 1) {
            return None;
        }
        let mut text = String::new();
        for item in items {
            text.push_str(item.text_item.as_ref()?.text.as_str());
        }
        (!text.trim().is_empty() && text.len() <= MAX_INBOUND_TEXT_BYTES).then_some(text)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageItem {
    #[serde(rename = "type")]
    pub kind: u8,
    pub text_item: Option<TextItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TextItem {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SendMessageRequest {
    pub msg: OutboundMessage,
    pub base_info: BaseInfo,
}

impl SendMessageRequest {
    pub fn text(to_user_id: &str, context_token: &str, client_id: &str, text: &str) -> Self {
        Self {
            msg: OutboundMessage {
                from_user_id: String::new(),
                to_user_id: to_user_id.to_owned(),
                client_id: client_id.to_owned(),
                message_type: 2,
                message_state: 2,
                context_token: context_token.to_owned(),
                item_list: vec![MessageItem {
                    kind: 1,
                    text_item: Some(TextItem {
                        text: text.to_owned(),
                    }),
                }],
            },
            base_info: BaseInfo::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OutboundMessage {
    pub from_user_id: String,
    pub to_user_id: String,
    pub client_id: String,
    pub message_type: u8,
    pub message_state: u8,
    pub context_token: String,
    pub item_list: Vec<MessageItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SendMessageResponse {
    pub ret: Option<i32>,
    pub errmsg: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QrCodeResponse {
    pub qrcode: String,
    pub qrcode_img_content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QrStatusResponse {
    pub status: String,
    pub bot_token: Option<String>,
    pub ilink_bot_id: Option<String>,
    pub baseurl: Option<String>,
    pub ilink_user_id: Option<String>,
    pub redirect_host: Option<String>,
}

pub fn validate_api_base_url(value: &str) -> Result<String, String> {
    let mut url = url::Url::parse(value.trim()).map_err(|_| "invalid iLink API URL".to_owned())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("iLink API URL must be a plain HTTPS origin".to_owned());
    }
    let Some(host) = url.host_str() else {
        return Err("iLink API URL is missing a host".to_owned());
    };
    if host.parse::<std::net::IpAddr>().is_ok()
        || !(host == "weixin.qq.com"
            || host.ends_with(".weixin.qq.com")
            || host == "wechat.com"
            || host.ends_with(".wechat.com")
            || host == "wx.qq.com"
            || host.ends_with(".wx.qq.com"))
    {
        return Err("iLink API host is outside the trusted WeChat domains".to_owned());
    }
    url.set_path("");
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

/// Split outbound text on Unicode scalar boundaries and keep each request below the byte limit.
pub fn split_reply(text: &str, max_bytes: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let limit = max_bytes.max(4);
    let mut chunks = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if current.len() + character.len_utf8() > limit {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(character);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn long_poll_and_reply_payloads_match_the_documented_ilink_shapes() {
        let poll = GetUpdatesRequest::new("cursor-1".to_owned());
        assert_eq!(
            serde_json::to_value(poll).unwrap(),
            json!({
                "get_updates_buf": "cursor-1",
                "base_info": { "channel_version": CHANNEL_VERSION, "bot_agent": BOT_AGENT }
            })
        );

        let reply = SendMessageRequest::text("peer-1", "ctx-1", "test-client-id", "hello");
        assert_eq!(
            serde_json::to_value(reply).unwrap(),
            json!({
                "msg": {
                    "from_user_id": "",
                    "to_user_id": "peer-1",
                    "client_id": "test-client-id",
                    "message_type": 2,
                    "message_state": 2,
                    "context_token": "ctx-1",
                    "item_list": [{"type": 1, "text_item": {"text": "hello"}}]
                },
                "base_info": { "channel_version": CHANNEL_VERSION, "bot_agent": BOT_AGENT }
            })
        );
    }

    #[test]
    fn accepts_only_text_direct_messages_with_reply_context() {
        let direct = serde_json::from_value::<IlinkMessage>(json!({
            "message_id": 4,
            "from_user_id": "sender-1",
            "session_id": "session-1",
            "message_type": 1,
            "context_token": "context-1",
            "item_list": [{"type": 1, "text_item": {"text": "run tests"}}]
        }))
        .unwrap();
        assert_eq!(direct.direct_text().as_deref(), Some("run tests"));

        let group = serde_json::from_value::<IlinkMessage>(json!({
            "from_user_id": "sender-1",
            "session_id": "session-1",
            "group_id": "group-1",
            "message_type": 1,
            "context_token": "context-1",
            "item_list": [{"type": 1, "text_item": {"text": "run tests"}}]
        }))
        .unwrap();
        assert_eq!(group.direct_text(), None);

        let media = serde_json::from_value::<IlinkMessage>(json!({
            "from_user_id": "sender-1",
            "session_id": "session-1",
            "message_type": 1,
            "context_token": "context-1",
            "item_list": [{"type": 2, "image_item": {}}]
        }))
        .unwrap();
        assert_eq!(media.direct_text(), None);
    }

    #[test]
    fn rejects_untrusted_or_non_https_api_hosts() {
        assert!(validate_api_base_url("https://ilinkai.weixin.qq.com").is_ok());
        assert!(validate_api_base_url("https://ilink1.wechat.com").is_ok());
        assert!(validate_api_base_url("http://ilinkai.weixin.qq.com").is_err());
        assert!(validate_api_base_url("https://ilinkai.weixin.qq.com.evil.example").is_err());
        assert!(validate_api_base_url("https://127.0.0.1").is_err());
        assert!(validate_api_base_url("https://user:pass@ilinkai.weixin.qq.com").is_err());
    }

    #[test]
    fn reply_chunking_never_splits_utf8_or_exceeds_the_byte_limit() {
        let text = format!("{}{}{}", "甲".repeat(1700), "。", "乙".repeat(1700));
        let chunks = split_reply(&text, 4096);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 4096));
        assert_eq!(chunks.concat(), text);
    }
}
