//! The JSON a bot and the gateway exchange over the WebSocket: one
//! envelope each way, Discord-shaped (`op` / `t` / `d`), the words
//! README.md §2 documents. Nothing here touches the network.

use protocol::{ConvRef, UserId};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Gateway → bot: one text frame each.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Outbound {
    /// The first frame on a connection: what this gateway is and, when
    /// the server session is up, the same `READY` data a bot that
    /// connected earlier got.
    Hello { d: Value },
    /// Something happened on the server (`t` names it, README §2.2).
    Dispatch { t: &'static str, d: Value },
    /// The gateway's own session with the server came or went.
    Status { t: &'static str, d: Value },
    /// An action of the bot's was not understood or not sent.
    Error { d: Value },
}

impl Outbound {
    #[must_use]
    pub fn dispatch(t: &'static str, d: Value) -> Self {
        Self::Dispatch { t, d }
    }

    #[must_use]
    pub fn status(t: &'static str, d: Value) -> Self {
        Self::Status { t, d }
    }

    #[must_use]
    pub fn error(message: impl Into<String>, action: Option<&str>) -> Self {
        let mut d = json!({ "message": message.into() });
        if let Some(action) = action {
            d["action"] = Value::String(action.to_string());
        }
        Self::Error { d }
    }

    /// The frame's text. Serialising these types cannot fail; the
    /// fallback is for the type system's sake.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{\"op\":\"error\"}".to_string())
    }
}

/// Bot → gateway: a text frame. Binary frames are voice (PCM) and do
/// not come through here.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Inbound {
    /// Do something on the server: `t` names the action, `d` its
    /// fields (README §2.3).
    Action {
        t: String,
        #[serde(default)]
        d: Value,
    },
    /// The voice sink: `STOP` drops what is queued to play.
    Voice { t: String },
}

/// A conversation as the JSON names it: `"general"` or `{"dm": "<user id>"}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
enum ConvJson {
    Named(String),
    Dm { dm: String },
}

/// What a bot may ask the server for, parsed from an `action` frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    CreateMessage {
        text: String,
    },
    CreateDm {
        to: UserId,
        text: String,
        client_ref: u32,
    },
    EditMessage {
        id: u64,
        text: String,
    },
    DeleteMessage {
        id: u64,
    },
    JoinChannel {
        channel_id: u16,
        password: Option<String>,
    },
    Speaking(bool),
    AudioState {
        muted: bool,
        deafened: bool,
    },
    SetNickname {
        name: String,
    },
    FetchHistory {
        conv: ConvRef,
        before: Option<u64>,
        limit: u16,
    },
    MarkRead {
        conv: ConvRef,
        id: u64,
    },
}

#[derive(Deserialize)]
struct TextD {
    text: String,
}
#[derive(Deserialize)]
struct DmD {
    user_id: String,
    text: String,
    #[serde(default)]
    client_ref: u32,
}
#[derive(Deserialize)]
struct EditD {
    id: u64,
    text: String,
}
#[derive(Deserialize)]
struct IdD {
    id: u64,
}
#[derive(Deserialize)]
struct JoinD {
    channel_id: u16,
    #[serde(default)]
    password: Option<String>,
}
#[derive(Deserialize)]
struct SpeakingD {
    speaking: bool,
}
#[derive(Deserialize)]
struct AudioStateD {
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    deafened: bool,
}
#[derive(Deserialize)]
struct NameD {
    name: String,
}
#[derive(Deserialize)]
struct HistoryD {
    #[serde(default = "general")]
    conv: ConvJson,
    #[serde(default)]
    before: Option<u64>,
    #[serde(default = "fifty")]
    limit: u16,
}
#[derive(Deserialize)]
struct MarkReadD {
    #[serde(default = "general")]
    conv: ConvJson,
    id: u64,
}

fn general() -> ConvJson {
    ConvJson::Named("general".to_string())
}

const fn fifty() -> u16 {
    50
}

/// The most history one action asks for (the server pages anyway).
const MAX_HISTORY_LIMIT: u16 = 200;

fn conv_of(conv: ConvJson) -> Result<ConvRef, String> {
    match conv {
        ConvJson::Named(name) if name.eq_ignore_ascii_case("general") => Ok(ConvRef::General),
        ConvJson::Named(other) => Err(format!("unknown conversation {other:?}")),
        ConvJson::Dm { dm } => parse_user_id(&dm).map(ConvRef::Dm),
    }
}

/// A user id as the JSON carries it: 64 hex digits.
///
/// # Errors
/// Anything else, with the offending text.
pub fn parse_user_id(text: &str) -> Result<UserId, String> {
    protocol::parse_hex_id(text).ok_or_else(|| format!("not a user id (64 hex digits): {text:?}"))
}

/// A user id as the JSON carries it.
#[must_use]
pub fn hex(id: &[u8]) -> String {
    let mut out = String::with_capacity(id.len() * 2);
    for byte in id {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

impl Action {
    /// Parse an `action` frame's `t` and `d`.
    ///
    /// # Errors
    /// An unknown `t`, or `d` without the fields `t` needs — the words
    /// go back to the bot in an `error` frame.
    pub fn parse(t: &str, d: Value) -> Result<Self, String> {
        fn fields<T: serde::de::DeserializeOwned>(t: &str, d: Value) -> Result<T, String> {
            serde_json::from_value(d).map_err(|e| format!("{t}: {e}"))
        }
        Ok(match t {
            "CREATE_MESSAGE" => {
                let TextD { text } = fields(t, d)?;
                Self::CreateMessage { text }
            }
            "CREATE_DM" => {
                let DmD {
                    user_id,
                    text,
                    client_ref,
                } = fields(t, d)?;
                Self::CreateDm {
                    to: parse_user_id(&user_id)?,
                    text,
                    client_ref,
                }
            }
            "EDIT_MESSAGE" => {
                let EditD { id, text } = fields(t, d)?;
                Self::EditMessage { id, text }
            }
            "DELETE_MESSAGE" => {
                let IdD { id } = fields(t, d)?;
                Self::DeleteMessage { id }
            }
            "JOIN_CHANNEL" => {
                let JoinD {
                    channel_id,
                    password,
                } = fields(t, d)?;
                Self::JoinChannel {
                    channel_id,
                    password,
                }
            }
            "SPEAKING" => {
                let SpeakingD { speaking } = fields(t, d)?;
                Self::Speaking(speaking)
            }
            "AUDIO_STATE" => {
                let AudioStateD { muted, deafened } = fields(t, d)?;
                Self::AudioState { muted, deafened }
            }
            "SET_NICKNAME" => {
                let NameD { name } = fields(t, d)?;
                Self::SetNickname { name }
            }
            "FETCH_HISTORY" => {
                let HistoryD {
                    conv,
                    before,
                    limit,
                } = fields(t, d)?;
                Self::FetchHistory {
                    conv: conv_of(conv)?,
                    before,
                    limit: limit.clamp(1, MAX_HISTORY_LIMIT),
                }
            }
            "MARK_READ" => {
                let MarkReadD { conv, id } = fields(t, d)?;
                Self::MarkRead {
                    conv: conv_of(conv)?,
                    id,
                }
            }
            other => return Err(format!("unknown action {other:?}")),
        })
    }
}

/// A conversation as the JSON names it (the other direction).
#[must_use]
pub fn conv_json(conv: &ConvRef) -> Value {
    match conv {
        ConvRef::General => Value::String("general".to_string()),
        ConvRef::Dm(user) => json!({ "dm": hex(user) }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_parse_from_their_frames() {
        let d = json!({ "text": "hi" });
        assert_eq!(
            Action::parse("CREATE_MESSAGE", d).unwrap(),
            Action::CreateMessage {
                text: "hi".to_string()
            }
        );
        let user = "ab".repeat(32);
        let d = json!({ "user_id": user, "text": "psst" });
        assert_eq!(
            Action::parse("CREATE_DM", d).unwrap(),
            Action::CreateDm {
                to: [0xab; 32],
                text: "psst".to_string(),
                client_ref: 0
            }
        );
        let d = json!({ "conv": { "dm": user }, "limit": 1000 });
        assert_eq!(
            Action::parse("FETCH_HISTORY", d).unwrap(),
            Action::FetchHistory {
                conv: ConvRef::Dm([0xab; 32]),
                before: None,
                limit: MAX_HISTORY_LIMIT
            }
        );
        assert_eq!(
            Action::parse("FETCH_HISTORY", json!({})).unwrap(),
            Action::FetchHistory {
                conv: ConvRef::General,
                before: None,
                limit: 50
            }
        );
        assert_eq!(
            Action::parse("JOIN_CHANNEL", json!({ "channel_id": 2 })).unwrap(),
            Action::JoinChannel {
                channel_id: 2,
                password: None
            }
        );
    }

    #[test]
    fn bad_frames_say_what_is_wrong() {
        assert!(Action::parse("DANCE", json!({}))
            .unwrap_err()
            .contains("unknown action"));
        assert!(Action::parse("CREATE_MESSAGE", json!({}))
            .unwrap_err()
            .contains("text"));
        assert!(
            Action::parse("CREATE_DM", json!({ "user_id": "nope", "text": "x" }))
                .unwrap_err()
                .contains("user id")
        );
        assert!(Action::parse("FETCH_HISTORY", json!({ "conv": "lobby" }))
            .unwrap_err()
            .contains("unknown conversation"));
    }

    #[test]
    fn envelopes_have_the_shape_the_readme_shows() {
        let text = Outbound::dispatch("MESSAGE_CREATE", json!({ "text": "hi" })).to_json();
        assert_eq!(
            text,
            r#"{"op":"dispatch","t":"MESSAGE_CREATE","d":{"text":"hi"}}"#
        );
        let inbound: Inbound =
            serde_json::from_str(r#"{"op":"action","t":"CREATE_MESSAGE","d":{"text":"x"}}"#)
                .unwrap();
        assert!(matches!(inbound, Inbound::Action { t, .. } if t == "CREATE_MESSAGE"));
        let inbound: Inbound = serde_json::from_str(r#"{"op":"voice","t":"STOP"}"#).unwrap();
        assert!(matches!(inbound, Inbound::Voice { t } if t == "STOP"));
        assert_eq!(hex(&[0, 255, 16]), "00ff10");
        assert_eq!(conv_json(&ConvRef::General), json!("general"));
    }
}
