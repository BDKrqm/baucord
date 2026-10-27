//! Moderation and administration (spec §4.1, §4.2, §8): the restriction
//! kinds, the admin commands, and what they answer with. Everything an
//! admin can do from the client goes through [`AdminCommand`], is
//! checked by the server like any other message, and is audited there.

use serde::{Deserialize, Serialize};

use crate::messages::{BlobId, ClientId, UserId};
use crate::permissions::RoleId;

/// What a restriction takes away (spec §4.2). The discriminants are the
/// database encoding; never renumber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RestrictionKind {
    /// Cannot join at all (kick, ban).
    Server = 0,
    /// Voice datagrams dropped (mute).
    Voice = 1,
    /// Writing in the general chat refused.
    GeneralText = 2,
    /// Direct messages refused, both ways.
    Dm = 3,
    /// Uploads (pictures and files) refused.
    Upload = 4,
    /// Screen sharing refused.
    Share = 5,
    /// The profile stays cleared: a `ResetProfile` with `lock`.
    Profile = 6,
}

impl RestrictionKind {
    /// The kind the database stores as `value`; `None` for a discriminant
    /// this build does not know.
    #[must_use]
    pub fn from_db(value: i64) -> Option<Self> {
        Some(match value {
            0 => Self::Server,
            1 => Self::Voice,
            2 => Self::GeneralText,
            3 => Self::Dm,
            4 => Self::Upload,
            5 => Self::Share,
            6 => Self::Profile,
            _ => return None,
        })
    }

    /// The database encoding of this kind: its discriminant.
    #[must_use]
    pub fn to_db(self) -> i64 {
        self as i64
    }

    /// The name used in commands (`/restrict name voice 30m`) and logs.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Voice => "voice",
            Self::GeneralText => "text",
            Self::Dm => "dm",
            Self::Upload => "upload",
            Self::Share => "share",
            Self::Profile => "profile",
        }
    }

    /// The inverse of [`Self::name`], forgiving about aliases.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "server" | "join" => Self::Server,
            "voice" | "mute" => Self::Voice,
            "text" | "chat" | "general_text" | "general" => Self::GeneralText,
            "dm" | "dms" => Self::Dm,
            "upload" | "uploads" => Self::Upload,
            "share" | "sharing" => Self::Share,
            "profile" => Self::Profile,
            _ => return None,
        })
    }
}

/// Whom a command is about. A client that has the person in its roster
/// sends the id; for someone offline (a ban to lift, a kick to undo)
/// it sends the name, and the server resolves it — a name of record,
/// case-insensitive, or a hex id prefix of at least 8 digits. An
/// ambiguous name is refused with the candidates listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminTarget {
    /// A user by id: someone in the client's roster.
    User(UserId),
    /// A device key hash (spec §2.1), whoever holds it.
    Key(UserId),
    /// A source IP address, in text.
    Address(String),
    /// A user by name of record or id prefix, resolved by the server.
    Name(String),
}

/// Longest `reason` an admin command keeps, in bytes (cut on a
/// character boundary). A reason travels in `Restricted`, the
/// restriction list and the audit page; one near the 17 KiB message
/// limit made those lists undeliverable.
pub const MAX_ADMIN_REASON_BYTES: usize = 500;

/// One administrative action (spec §8). `reason` texts are shown to the
/// affected user and kept in the audit log.
// The variants' docs say what their fields are and which permission
// each command needs; a line per field would restate the names.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminCommand {
    /// Every user the server knows, with their keys and roles (needs
    /// `MANAGE_USERS`).
    ListUsers,
    /// Every restriction in force (needs `RESTRICT` or `MUTE`).
    ListRestrictions,
    /// Every invite code, used up and revoked ones included (needs
    /// `MANAGE_INVITES`).
    ListInvites,
    /// Replace a user's role set. Needs `MANAGE_ROLES`; no role may
    /// rank above the actor's own, admins may edit each other, and the
    /// last admin keeps `admin`.
    SetRoles {
        target: AdminTarget,
        roles: Vec<RoleId>,
    },
    /// Add a restriction (needs `RESTRICT`, or `MUTE` for `voice`).
    /// `duration_secs: None` = until lifted. `/mute` is this with
    /// `voice` or `text`.
    Restrict {
        target: AdminTarget,
        kind: RestrictionKind,
        duration_secs: Option<u64>,
        reason: String,
    },
    /// Remove a restriction by id (needs `RESTRICT`, or `MUTE` for a
    /// `voice` one).
    Lift { restriction_id: u64 },
    /// Remove every restriction of `kind` on a user (`/unmute`). Same
    /// permission as `Restrict`.
    Unmute {
        target: AdminTarget,
        kind: RestrictionKind,
    },
    /// Close the session; access unchanged (needs `DISCONNECT`).
    Disconnect { target: AdminTarget, reason: String },
    /// Close the session and forget the person's device keys and
    /// roles: they need a new invite to come back, and start over as
    /// whatever it grants (needs `KICK`). Nothing to lift.
    Kick { target: AdminTarget, reason: String },
    /// Close the session and block the user, their key and their
    /// address until `duration_secs` runs out or `Unban` (needs `BAN`).
    Ban {
        target: AdminTarget,
        duration_secs: Option<u64>,
        reason: String,
    },
    /// Remove every `server` restriction on a user (needs `BAN`).
    Unban { target: AdminTarget },
    /// Put a user in another voice channel (needs `MANAGE_CHANNELS`).
    MoveUser {
        target: AdminTarget,
        channel_id: u16,
    },
    /// Stop a code from admitting anyone else (needs `MANAGE_INVITES`).
    RevokeInvite { code: String },
    /// Attach a device key to a user — a retired key, or one seen in the
    /// log (needs `MANAGE_USERS`).
    LinkKey {
        target: AdminTarget,
        key_hash: UserId,
    },
    /// Retire a device key: that device is refused until re-linked
    /// (needs `MANAGE_USERS`).
    UnlinkKey { key_hash: UserId },
    /// Remove anyone's message (needs `RESTRICT`).
    DeleteMessage { msg_id: u64 },
    /// Delete a person: their row, keys, roles, restrictions, progress
    /// and messages. The audit keeps a line (needs `MANAGE_USERS`).
    PurgeUser { target: AdminTarget },
    /// The last telemetry sample a user sent, summarized (needs
    /// `MANAGE_USERS`): what the roster tooltip shows.
    LastTelemetry { target: AdminTarget },
    /// Clear a user's profile (spec §7.1; needs `RESTRICT`); with
    /// `lock`, a `profile` restriction keeps it cleared until lifted.
    ResetProfile { target: AdminTarget, lock: bool },

    // ── 1.1.3: the panel (feature bit `ADMIN_PANEL`) ──────────────────
    /// The audit log, newest first, entries before id `before` (needs
    /// `ADMIN`). Answered with [`AdminOutcome::Audit`]; empty without
    /// persistence.
    AuditLog { before: Option<u64>, limit: u16 },
    /// A custom role (spec §4; needs `MANAGE_ROLES`, and the role's
    /// derived rank must stay below the actor's). Answered with
    /// [`AdminOutcome::RoleCreated`].
    CreateRole {
        name: String,
        permissions: u64,
        /// From `permissions::ROLE_ICONS`, or `""`.
        icon: String,
        /// RGB, 0 = none.
        color: u32,
    },
    /// Rename, re-permission or restyle a custom role (`None` keeps).
    EditRole {
        role: RoleId,
        name: Option<String>,
        permissions: Option<u64>,
        icon: Option<String>,
        color: Option<u32>,
    },
    /// Remove a custom role; whoever holds it loses it.
    DeleteRole { role: RoleId },
    /// Every stored blob (needs `RESTRICT`).
    ListBlobs,
    /// Remove a blob; the messages that referenced it keep their card
    /// but a fetch says it is gone (needs `RESTRICT`).
    DeleteBlob { blob_id: BlobId },
    /// The server's counters (needs `ADMIN`).
    ServerStats,

    // ── 1.2: reports (feature bit `SAFETY`) ──────────────────────────
    /// Reports, newest first (needs `RESTRICT`): the open ones, or all
    /// with `resolved`. Answered with [`AdminOutcome::Reports`]; empty
    /// without persistence.
    ListReports { resolved: bool },
    /// Mark a report handled (needs `RESTRICT`); it leaves the open list.
    ResolveReport { id: u64 },
}

impl AdminCommand {
    /// Cut the command's `reason`, if it has one, to
    /// [`MAX_ADMIN_REASON_BYTES`] on a character boundary. The server
    /// does this to every command it receives; a client may do it
    /// before sending so the user sees what is kept.
    pub fn clip_reason(&mut self) {
        if let Self::Restrict { reason, .. }
        | Self::Disconnect { reason, .. }
        | Self::Kick { reason, .. }
        | Self::Ban { reason, .. } = self
            && reason.len() > MAX_ADMIN_REASON_BYTES
        {
            *reason = crate::truncate_on_char_boundary(reason, MAX_ADMIN_REASON_BYTES);
        }
    }
}

/// One audit line (spec §8), as the panel shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// The entry's id: the paging cursor (`AuditLog::before`).
    pub id: u64,
    /// When it happened, milliseconds since the Unix epoch.
    pub ts: u64,
    /// The actor's name of record; `None` for the server itself.
    pub actor: Option<String>,
    /// What was done, as the audit names it (`kick`, `role.create`, ...).
    pub action: String,
    /// Whom or what it was done to, by name, when there is one.
    pub target: Option<String>,
    /// The reason or the details, when there are any.
    pub detail: Option<String>,
}

/// One stored blob (spec §7), as the panel's Files tab shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobSummary {
    /// The blob's identity: the SHA-256 of its bytes.
    pub blob_id: BlobId,
    /// The file name as uploaded.
    pub name: String,
    /// The MIME type as stored.
    pub mime: String,
    /// The size in bytes.
    pub size: u32,
    /// The uploader's name of record; `None` once they are gone.
    pub created_by: Option<String>,
    /// When it was stored, milliseconds since the Unix epoch.
    pub created_at: u64,
    /// Profiles and messages pointing at it; zero is evictable.
    pub refs: u32,
}

/// The server's counters, as the panel's Server tab shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsSummary {
    /// The server's version.
    pub version: String,
    /// Seconds since the server started.
    pub uptime_secs: u64,
    /// Sessions connected right now.
    pub online: u32,
    /// Users the server knows (with persistence; the roster's size
    /// without).
    pub users: u32,
    /// Messages stored.
    pub messages: u64,
    /// Blobs stored.
    pub blobs: u32,
    /// Bytes the stored blobs take.
    pub blob_bytes: u64,
    /// The blob store's cap, bytes (`files.total_cap_gb`).
    pub blob_cap_bytes: u64,
    /// Whether the server keeps a database (`persistence.enabled`).
    pub persistent: bool,
    /// Writes waiting for the store thread right now.
    pub queue_depth: u32,
    /// How many may wait before writes are dropped.
    pub queue_capacity: u32,
    /// Writes refused because the queue was full, since the server started.
    pub dropped_writes: u64,
    /// Writes the store rejected, since the server started.
    pub write_errors: u64,
}

/// One device key of a user, as `UserSummary` lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeySummary {
    /// The key's hash: how the key is named everywhere.
    pub key_hash: UserId,
    /// Retired by `UnlinkKey`: the device is refused until re-linked.
    pub retired: bool,
    /// Milliseconds since the Unix epoch.
    pub last_seen: u64,
}

/// One user as `ListUsers` reports them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSummary {
    /// The user's id.
    pub user_id: UserId,
    /// Their name of record.
    pub name: String,
    /// The roles they hold, by id.
    pub roles: Vec<RoleId>,
    /// The session id while online.
    pub client_id: Option<ClientId>,
    /// When they were last connected, milliseconds since the Unix epoch.
    pub last_seen: u64,
    /// Their device keys.
    pub keys: Vec<KeySummary>,
}

/// One restriction as `ListRestrictions` reports them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestrictionSummary {
    /// The restriction's id (`Lift` takes it).
    pub id: u64,
    /// The restricted user, when the restriction is on a user.
    pub user_id: Option<UserId>,
    /// The restricted user's name of record, when there is one.
    pub user_name: Option<String>,
    /// The restricted key, when it is on a device key.
    pub key_hash: Option<UserId>,
    /// The restricted address, in text, when it is on an address.
    pub address: Option<String>,
    /// What it takes away.
    pub kind: RestrictionKind,
    /// When it ends, milliseconds since the Unix epoch; `None` until
    /// lifted.
    pub until: Option<u64>,
    /// The reason given, shown to the affected user.
    pub reason: String,
    /// Who did it, by name; `server` when automatic.
    pub by: String,
    /// When it was made, milliseconds since the Unix epoch.
    pub created_at: u64,
}

/// One invite code as `ListInvites` reports them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteSummary {
    /// The code, canonical (`invite::normalize_code`).
    pub code: String,
    /// The role a newcomer admitted by it gets.
    pub grants_role: RoleId,
    /// Who minted it, by name; `None` for the command line.
    pub created_by: Option<String>,
    /// When it was minted, milliseconds since the Unix epoch.
    pub created_at: u64,
    /// When it stops admitting, milliseconds since the Unix epoch; `None`
    /// for never.
    pub expires_at: Option<u64>,
    /// How many newcomers it may admit; `None` for no cap.
    pub max_uses: Option<u32>,
    /// How many it has admitted.
    pub uses: u32,
    /// Whether an admin revoked it.
    pub revoked: bool,
}

/// What a command produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminOutcome {
    /// The command took effect and has nothing to report.
    Done,
    /// The answer to `ListUsers`.
    Users(Vec<UserSummary>),
    /// The answer to `ListRestrictions`.
    Restrictions(Vec<RestrictionSummary>),
    /// The answer to `ListInvites`.
    Invites(Vec<InviteSummary>),
    /// `None`: nothing received from that user this run.
    Telemetry(Option<TelemetrySummary>),
    // 1.1.3 (answers to the panel's commands):
    /// The answer to `AuditLog`: a page of entries, newest first.
    Audit(Vec<AuditEntry>),
    /// The answer to `ListBlobs`.
    Blobs(Vec<BlobSummary>),
    /// The answer to `ServerStats`.
    Stats(StatsSummary),
    /// The answer to `CreateRole`: the new role's id.
    RoleCreated(RoleId),
    // 1.2 (feature bit `SAFETY`):
    /// The answer to `ListReports`.
    Reports(Vec<crate::safety::ReportSummary>),
}

impl AdminOutcome {
    /// A listing as the messages that carry it, in order: each holds
    /// the rows that fit `budget`. One page for what is not a listing,
    /// and for a listing with no rows.
    #[must_use]
    pub fn into_pages(self, budget: usize) -> Vec<Self> {
        use crate::messages::split_to_fit as split;
        fn pages<T>(runs: Vec<Vec<T>>, wrap: fn(Vec<T>) -> AdminOutcome) -> Vec<AdminOutcome> {
            if runs.is_empty() {
                return vec![wrap(Vec::new())];
            }
            runs.into_iter().map(wrap).collect()
        }
        match self {
            Self::Users(rows) => pages(split(rows, budget), Self::Users),
            Self::Restrictions(rows) => pages(split(rows, budget), Self::Restrictions),
            Self::Invites(rows) => pages(split(rows, budget), Self::Invites),
            Self::Audit(rows) => pages(split(rows, budget), Self::Audit),
            Self::Blobs(rows) => pages(split(rows, budget), Self::Blobs),
            Self::Reports(rows) => pages(split(rows, budget), Self::Reports),
            other @ (Self::Done | Self::Telemetry(_) | Self::Stats(_) | Self::RoleCreated(_)) => {
                vec![other]
            }
        }
    }

    /// How many rows a listing holds; 0 for what is not one.
    #[must_use]
    pub fn rows(&self) -> usize {
        match self {
            Self::Users(rows) => rows.len(),
            Self::Restrictions(rows) => rows.len(),
            Self::Invites(rows) => rows.len(),
            Self::Audit(rows) => rows.len(),
            Self::Blobs(rows) => rows.len(),
            Self::Reports(rows) => rows.len(),
            Self::Done | Self::Telemetry(_) | Self::Stats(_) | Self::RoleCreated(_) => 0,
        }
    }

    /// Put the rows of `earlier` (pages that arrived before this one)
    /// in front of this listing's. `false`, and nothing changed, when
    /// the two are not the same listing.
    pub fn prepend(&mut self, earlier: Self) -> bool {
        fn join<T>(rows: &mut Vec<T>, mut earlier: Vec<T>) -> bool {
            earlier.append(rows);
            *rows = earlier;
            true
        }
        match (self, earlier) {
            (Self::Users(rows), Self::Users(earlier)) => join(rows, earlier),
            (Self::Restrictions(rows), Self::Restrictions(earlier)) => join(rows, earlier),
            (Self::Invites(rows), Self::Invites(earlier)) => join(rows, earlier),
            (Self::Audit(rows), Self::Audit(earlier)) => join(rows, earlier),
            (Self::Blobs(rows), Self::Blobs(earlier)) => join(rows, earlier),
            (Self::Reports(rows), Self::Reports(earlier)) => join(rows, earlier),
            _ => false,
        }
    }
}

/// A user's last telemetry, as the server logs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelemetrySummary {
    /// Milliseconds since the Unix epoch.
    pub ts: u64,
    /// The sample, summarized on one line as the server logs it.
    pub line: String,
}

/// Why a command was refused. The client renders one line each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminError {
    /// The actor lacks the permission the command needs.
    NoPermission,
    /// The target names nobody the server knows.
    NoSuchUser,
    /// No restriction has that id (lifted already, or never existed).
    NoSuchRestriction,
    /// No invite has that code.
    NoSuchInvite,
    /// No role has that id (a custom one deleted meanwhile, say).
    NoSuchRole,
    /// No channel has that id.
    NoSuchChannel,
    /// The target ranks at or above the actor (spec §8: nobody acts on
    /// an equal or higher rank), or a role would outrank the actor.
    Outranked,
    /// The last admin keeps `admin`.
    LastAdmin,
    /// You cannot do that to yourself (kick, ban, disconnect).
    Yourself,
    /// A malformed command (reason attached).
    Invalid(String),
}

impl AdminError {
    /// Human-readable form for the chat line.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::NoPermission => "you may not do that".to_string(),
            Self::NoSuchUser => "no such user".to_string(),
            Self::NoSuchRestriction => "no such restriction".to_string(),
            Self::NoSuchInvite => "no such invite".to_string(),
            Self::NoSuchRole => "no such role".to_string(),
            Self::NoSuchChannel => "no such channel".to_string(),
            Self::Outranked => "they rank at or above you".to_string(),
            Self::LastAdmin => "the last admin keeps admin".to_string(),
            Self::Yourself => "not on yourself".to_string(),
            Self::Invalid(why) => why.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review P0-1: a listing of 60 KiB (four times what one message
    /// holds) leaves as pages that each encode, and joins back whole.
    #[test]
    fn a_long_listing_splits_into_pages_that_join_back_in_order() {
        use crate::messages::{
            decode_server_message, encode_server_message, ServerMessage, LIST_ITEMS_BUDGET,
            MAX_CONTROL_MESSAGE_BYTES,
        };
        let users: Vec<UserSummary> = (0..700u32)
            .map(|i| UserSummary {
                user_id: [u8::try_from(i % 251).expect("under 251"); 32],
                name: format!("user-{i:04}"),
                roles: vec![crate::permissions::ROLE_MEMBER],
                client_id: None,
                last_seen: 1_700_000_000_000 + u64::from(i),
                keys: vec![KeySummary {
                    key_hash: [7u8; 32],
                    retired: false,
                    last_seen: 1_700_000_000_000,
                }],
            })
            .collect();
        let whole = AdminOutcome::Users(users);
        let bytes = postcard::to_allocvec(&whole).expect("encodes").len();
        assert!(bytes > 60 * 1024, "the listing is {bytes} bytes");

        let mut pages = whole.clone().into_pages(LIST_ITEMS_BUDGET);
        assert!(pages.len() >= 4, "{} pages", pages.len());
        assert_eq!(
            pages.iter().map(AdminOutcome::rows).sum::<usize>(),
            whole.rows()
        );
        // Every page crosses the wire: the rows ahead as `AdminRows`,
        // the last as the result.
        let mut joined = pages.pop().expect("a last page");
        let closing = ServerMessage::AdminResult {
            request_id: 3,
            result: Ok(joined.clone()),
        };
        let wire = encode_server_message(&closing).expect("the result fits");
        assert!(wire.len() <= MAX_CONTROL_MESSAGE_BYTES);
        let mut earlier = AdminOutcome::Users(Vec::new());
        for page in pages {
            let msg = ServerMessage::AdminRows {
                request_id: 3,
                rows: page,
            };
            let wire = encode_server_message(&msg).expect("a page fits");
            assert!(wire.len() <= MAX_CONTROL_MESSAGE_BYTES);
            let Some(ServerMessage::AdminRows { rows, .. }) = decode_server_message(&wire) else {
                panic!("a page decodes");
            };
            let mut next = rows;
            assert!(next.prepend(earlier));
            earlier = next;
        }
        assert!(joined.prepend(earlier));
        assert_eq!(joined, whole);
    }

    #[test]
    fn what_is_not_a_listing_is_one_page_and_joins_nothing() {
        assert_eq!(AdminOutcome::Done.into_pages(16), vec![AdminOutcome::Done]);
        assert_eq!(
            AdminOutcome::Invites(Vec::new()).into_pages(16),
            vec![AdminOutcome::Invites(Vec::new())]
        );
        let mut done = AdminOutcome::Done;
        assert!(!done.prepend(AdminOutcome::Audit(Vec::new())));
        let mut audit = AdminOutcome::Audit(Vec::new());
        assert!(!audit.prepend(AdminOutcome::Invites(Vec::new())));
        assert_eq!(audit, AdminOutcome::Audit(Vec::new()));
    }

    #[test]
    fn restriction_kinds_round_trip_the_db_encoding_and_names() {
        for kind in [
            RestrictionKind::Server,
            RestrictionKind::Voice,
            RestrictionKind::GeneralText,
            RestrictionKind::Dm,
            RestrictionKind::Upload,
            RestrictionKind::Share,
            RestrictionKind::Profile,
        ] {
            assert_eq!(RestrictionKind::from_db(kind.to_db()), Some(kind));
            assert_eq!(RestrictionKind::parse(kind.name()), Some(kind));
        }
        assert_eq!(RestrictionKind::from_db(7), None);
        assert_eq!(RestrictionKind::parse("mute"), Some(RestrictionKind::Voice));
        assert_eq!(RestrictionKind::parse("nope"), None);
    }

    #[test]
    fn a_reason_is_cut_on_a_character_boundary() {
        let mut ban = AdminCommand::Ban {
            target: AdminTarget::Name("x".into()),
            duration_secs: None,
            reason: "é".repeat(MAX_ADMIN_REASON_BYTES),
        };
        ban.clip_reason();
        let AdminCommand::Ban { reason, .. } = &ban else {
            unreachable!()
        };
        assert!(reason.len() <= MAX_ADMIN_REASON_BYTES);
        assert_eq!(reason.chars().count(), MAX_ADMIN_REASON_BYTES / 2);
        let mut short = AdminCommand::Kick {
            target: AdminTarget::Name("x".into()),
            reason: "spam".into(),
        };
        short.clip_reason();
        assert!(matches!(short, AdminCommand::Kick { reason, .. } if reason == "spam"));
    }
}
