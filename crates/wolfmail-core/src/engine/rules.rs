//! Message filters (Thunderbird-style rules run on new Inbox mail and on demand) and snooze.
//!
//! Filters match on what the engine already stores for every message without downloading bodies — From, To, Cc,
//! Subject and "has attachment" — so they run the moment a header arrives, before the new-mail notification.
//! Actions reuse the engine's normal message operations (so IMAP, Gmail API and Graph all work), inside a
//! `QUIET` scope so automatic moves don't pop an Undo toast at the user.
//!
//! Snooze hides a message from every view except "Snoozed" until a chosen time, then brings it back unread and
//! announces it like new mail. It is kept on this computer only (no IMAP or provider standard carries it), which the
//! UI says.
use super::Engine;
use crate::error::{ApiError, Result};
use crate::db::now_ms;
use crate::model::MessageSummary;
use crate::msgstore;
use regex::RegexBuilder;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

tokio::task_local! {
    /// Set while filters run: the move/archive/trash paths skip their "undoable" toast for these automatic actions.
    pub(crate) static QUIET: bool;
}

pub(crate) fn quiet() -> bool {
    QUIET.try_with(|q| *q).unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FilterField {
    From,
    To,
    Cc,
    ToOrCc,
    AnyAddress,
    Subject,
    HasAttachment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FilterOp {
    Contains,
    NotContains,
    Is,
    IsNot,
    BeginsWith,
    EndsWith,
    MatchesRegex,
    /// For HasAttachment: value is ignored.
    IsTrue,
    IsFalse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterCondition {
    pub field: FilterField,
    pub op: FilterOp,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FilterAction {
    #[serde(rename_all = "camelCase")]
    MoveTo { folder_id: String },
    #[serde(rename_all = "camelCase")]
    CopyTo { folder_id: String },
    MarkRead,
    Star,
    Tag { tag: String },
    Archive,
    Trash,
    Junk,
    /// Don't run any later filters on this message.
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterRule {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// None = every account. Rules that move or copy must name the account their folder belongs to.
    #[serde(default)]
    pub account_id: Option<String>,
    /// true: all conditions must match; false: any one.
    #[serde(default = "yes")]
    pub match_all: bool,
    pub conditions: Vec<FilterCondition>,
    pub actions: Vec<FilterAction>,
    #[serde(default)]
    pub sort_order: i64,
}

fn yes() -> bool {
    true
}

// ── storage ────────────────────────────────────────────────────────────────────────────────────────────────────

fn load_rules(c: &rusqlite::Connection) -> Result<Vec<FilterRule>> {
    let mut st = c.prepare(
        "SELECT id, name, enabled, account_id, match_all, conditions_json, actions_json, sort_order FROM filters ORDER BY sort_order, created_at",
    )?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, bool>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, bool>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, i64>(7)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, name, enabled, account_id, match_all, cj, aj, sort_order) = row?;
        // A rule written by a newer build that this one can't read is skipped rather than failing every filter run.
        let (Ok(conditions), Ok(actions)) = (serde_json::from_str(&cj), serde_json::from_str(&aj)) else { continue };
        out.push(FilterRule { id, name, enabled, account_id, match_all, conditions, actions, sort_order });
    }
    Ok(out)
}

fn validate(rule: &FilterRule, folder_account: impl Fn(&str) -> Option<String>) -> Result<()> {
    if rule.name.trim().is_empty() {
        return Err(ApiError::invalid("name", "Give the filter a name."));
    }
    if rule.conditions.is_empty() {
        return Err(ApiError::invalid("conditions", "Add at least one condition."));
    }
    if rule.actions.iter().all(|a| *a == FilterAction::Stop) {
        return Err(ApiError::invalid("actions", "Add at least one action."));
    }
    for c in &rule.conditions {
        let needs_value = !matches!(c.field, FilterField::HasAttachment);
        if needs_value && c.value.trim().is_empty() {
            return Err(ApiError::invalid("conditions", "Every condition needs something to match."));
        }
        if c.op == FilterOp::MatchesRegex {
            RegexBuilder::new(&c.value)
                .case_insensitive(true)
                .size_limit(1 << 20)
                .build()
                .map_err(|e| ApiError::invalid("conditions", format!("That pattern isn't a valid regular expression: {e}")))?;
        }
    }
    for a in &rule.actions {
        if let FilterAction::MoveTo { folder_id } | FilterAction::CopyTo { folder_id } = a {
            let owner = folder_account(folder_id).ok_or_else(|| ApiError::invalid("actions", "The target folder no longer exists."))?;
            match &rule.account_id {
                Some(acc) if *acc == owner => {}
                _ => {
                    return Err(ApiError::invalid(
                        "actions",
                        "Filters that move or copy must be limited to the account that owns the target folder.",
                    ))
                }
            }
        }
    }
    Ok(())
}

impl Engine {
    pub async fn filters_list(&self) -> Result<Vec<FilterRule>> {
        self.store().call(|c| load_rules(c)).await
    }

    pub async fn filter_upsert(&self, mut rule: FilterRule) -> Result<FilterRule> {
        let folders: BTreeMap<String, String> = self
            .store()
            .call(|c| {
                let mut st = c.prepare("SELECT id, account_id FROM folders")?;
                let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?.to_string(), r.get::<_, String>(1)?)))?;
                Ok(rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()?)
            })
            .await?;
        validate(&rule, |f| folders.get(f).cloned())?;
        if rule.id.is_empty() {
            rule.id = uuid::Uuid::new_v4().to_string();
        }
        let r = rule.clone();
        self.store()
            .call(move |c| {
                let next: i64 = c.query_row("SELECT COALESCE(MAX(sort_order), -1) + 1 FROM filters", [], |x| x.get(0))?;
                let existing: Option<i64> =
                    c.query_row("SELECT sort_order FROM filters WHERE id=?1", params![r.id], |x| x.get(0)).optional()?;
                c.execute(
                    "INSERT INTO filters (id, name, enabled, account_id, match_all, conditions_json, actions_json, sort_order, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(id) DO UPDATE SET name=excluded.name, enabled=excluded.enabled, account_id=excluded.account_id,
                       match_all=excluded.match_all, conditions_json=excluded.conditions_json, actions_json=excluded.actions_json",
                    params![
                        r.id,
                        r.name.trim(),
                        r.enabled,
                        r.account_id,
                        r.match_all,
                        serde_json::to_string(&r.conditions)?,
                        serde_json::to_string(&r.actions)?,
                        existing.unwrap_or(next),
                        now_ms()
                    ],
                )?;
                Ok(())
            })
            .await?;
        let id = rule.id.clone();
        self.filters_list().await?.into_iter().find(|f| f.id == id).ok_or_else(|| ApiError::not_found("The filter"))
    }

    pub async fn filter_delete(&self, id: String) -> Result<()> {
        self.store().call(move |c| Ok(c.execute("DELETE FROM filters WHERE id=?1", params![id]).map(|_| ())?)).await
    }

    pub async fn filters_reorder(&self, ids: Vec<String>) -> Result<()> {
        self.store()
            .call(move |c| {
                let tx = c.unchecked_transaction()?;
                for (i, id) in ids.iter().enumerate() {
                    tx.execute("UPDATE filters SET sort_order=?1 WHERE id=?2", params![i as i64, id])?;
                }
                tx.commit()?;
                Ok(())
            })
            .await
    }

    /// Run every enabled filter over a folder now ("Run filters on this folder"). Returns how many messages a filter
    /// acted on.
    pub async fn filters_run(&self, folder_id: String) -> Result<u32> {
        let fid: i64 = folder_id.parse().map_err(|_| ApiError::not_found("The folder"))?;
        let (account_id, ids): (String, Vec<i64>) = self
            .store()
            .call(move |c| {
                let acc: String = c.query_row("SELECT account_id FROM folders WHERE id=?1", params![fid], |r| r.get(0))?;
                let mut st = c.prepare(
                    "SELECT id FROM messages WHERE folder_id=?1 OR id IN (SELECT message_rowid FROM message_labels WHERE folder_id=?1)",
                )?;
                let ids = st.query_map(params![fid], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
                Ok((acc, ids))
            })
            .await?;
        let (_, acted) = self.apply_filters(&account_id, ids).await?;
        Ok(acted)
    }

    /// Called by the sync runners with the new Inbox rows of one account, before the new-mail notification.
    /// Returns the rows still in the Inbox afterwards (the ones worth notifying about). Filter failures are logged and
    /// never stop mail from arriving.
    pub(crate) async fn filters_on_new(&self, account_id: &str, ids: Vec<i64>) -> Vec<i64> {
        match self.apply_filters(account_id, ids.clone()).await {
            Ok((left, _)) => left,
            Err(e) => {
                tracing::warn!(account = %account_id, "filters failed on new mail: {}", e.message);
                ids
            }
        }
    }

    async fn apply_filters(&self, account_id: &str, ids: Vec<i64>) -> Result<(Vec<i64>, u32)> {
        if ids.is_empty() {
            return Ok((ids, 0));
        }
        let all = ids.clone();
        let acc = account_id.to_string();
        let (rules, msgs) = self
            .store()
            .call(move |c| {
                let rules: Vec<FilterRule> = load_rules(c)?
                    .into_iter()
                    .filter(|r| r.enabled && r.account_id.as_deref().is_none_or(|a| a == acc))
                    .collect();
                let mut msgs = Vec::new();
                if !rules.is_empty() {
                    for id in &ids {
                        if let Ok(m) = msgstore::get_summary(c, *id) {
                            msgs.push(m);
                        }
                    }
                }
                Ok((rules, msgs))
            })
            .await?;
        if rules.is_empty() {
            return Ok((all, 0));
        }
        let plan = plan_actions(&rules, &msgs);
        let acted = plan.acted.len() as u32;
        let leaves: std::collections::HashSet<String> = plan.terminal.values().flatten().cloned().collect();
        QUIET.scope(true, self.execute_plan(plan)).await?;
        let left = msgs.iter().filter(|m| !leaves.contains(&m.id)).filter_map(|m| m.id.parse().ok()).collect();
        Ok((left, acted))
    }

    async fn execute_plan(&self, p: Plan) -> Result<()> {
        if !p.mark_read.is_empty() {
            self.messages_set_flags(p.mark_read, vec!["seen".into()], vec![]).await?;
        }
        if !p.star.is_empty() {
            self.messages_set_flags(p.star, vec!["flagged".into()], vec![]).await?;
        }
        for (tag, ids) in p.tags {
            self.messages_set_tags(ids, vec![tag], vec![]).await?;
        }
        for (folder, ids) in p.copies {
            self.messages_copy(ids, folder).await?;
        }
        for (t, ids) in p.terminal {
            match t {
                Terminal::Move(folder) => self.messages_move(ids, folder).await?,
                Terminal::Archive => self.messages_archive(ids).await?,
                Terminal::Trash => self.messages_trash(ids).await?,
                Terminal::Junk => self.messages_junk(ids, true).await?,
            }
        }
        Ok(())
    }

    // ── snooze ─────────────────────────────────────────────────────────────────────────────────────────────────

    pub async fn messages_snooze(&self, ids: Vec<String>, until: i64) -> Result<()> {
        if until <= now_ms() + 30_000 {
            return Err(ApiError::invalid("until", "Pick a time in the future."));
        }
        let rows: Vec<i64> = ids.iter().filter_map(|s| s.parse().ok()).collect();
        let touched = self
            .store()
            .call(move |c| {
                let tx = c.unchecked_transaction()?;
                let mut folders = Vec::new();
                for id in &rows {
                    tx.execute(
                        "INSERT INTO snoozes (message_rowid, until_ms) VALUES (?1, ?2)
                         ON CONFLICT(message_rowid) DO UPDATE SET until_ms=excluded.until_ms",
                        params![id, until],
                    )?;
                    if let Some(f) = tx.query_row("SELECT folder_id FROM messages WHERE id=?1", params![id], |r| r.get::<_, i64>(0)).optional()? {
                        folders.push(f.to_string());
                    }
                }
                tx.commit()?;
                Ok(folders)
            })
            .await?;
        self.emit("mail://changed", serde_json::json!({ "folderIds": touched }));
        self.emit_counts().await;
        self.wake_snoozer();
        Ok(())
    }

    pub async fn messages_unsnooze(&self, ids: Vec<String>) -> Result<()> {
        let rows: Vec<i64> = ids.iter().filter_map(|s| s.parse().ok()).collect();
        let touched = self
            .store()
            .call(move |c| {
                let mut folders = Vec::new();
                for id in &rows {
                    c.execute("DELETE FROM snoozes WHERE message_rowid=?1", params![id])?;
                    if let Some(f) = c.query_row("SELECT folder_id FROM messages WHERE id=?1", params![id], |r| r.get::<_, i64>(0)).optional()? {
                        folders.push(f.to_string());
                    }
                }
                Ok(folders)
            })
            .await?;
        self.emit("mail://changed", serde_json::json!({ "folderIds": touched }));
        self.emit_counts().await;
        Ok(())
    }

    fn wake_snoozer(&self) {
        self.inner.snooze_wake.notify_one();
    }

    /// Background task: bring snoozed messages back when their time comes. Sleeps until the next due time (at most
    /// a minute, so a changed clock or a resumed laptop is noticed quickly).
    pub(crate) fn spawn_snoozer(&self) {
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = engine.wake_due_snoozes().await {
                    tracing::warn!("snooze wake-up failed: {}", e.message);
                }
                let next: Option<i64> = engine
                    .store()
                    .call(|c| Ok(c.query_row("SELECT MIN(until_ms) FROM snoozes", [], |r| r.get(0))?))
                    .await
                    .unwrap_or(None);
                let wait = next.map(|t| (t - now_ms()).clamp(1_000, 60_000)).unwrap_or(60_000);
                tokio::select! {
                    _ = engine.inner.snooze_wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_millis(wait as u64)) => {}
                }
            }
        });
    }

    async fn wake_due_snoozes(&self) -> Result<()> {
        let now = now_ms();
        let due: Vec<(i64, MessageSummary)> = self
            .store()
            .call(move |c| {
                let mut st = c.prepare("SELECT message_rowid FROM snoozes WHERE until_ms <= ?1")?;
                let ids = st.query_map(params![now], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
                let mut out = Vec::new();
                for id in ids {
                    c.execute("DELETE FROM snoozes WHERE message_rowid=?1", params![id])?;
                    if let Ok(m) = msgstore::get_summary(c, id) {
                        out.push((id, m));
                    }
                }
                Ok(out)
            })
            .await?;
        if due.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = due.iter().map(|(id, _)| id.to_string()).collect();
        // Back as unread, like new mail.
        QUIET.scope(true, self.messages_set_flags(ids, vec![], vec!["seen".into()])).await?;
        let mut by_account: BTreeMap<String, Vec<MessageSummary>> = BTreeMap::new();
        for (_, mut m) in due {
            m.flags.seen = false;
            m.snoozed_until = None;
            by_account.entry(m.account_id.clone()).or_default().push(m);
        }
        for (account_id, msgs) in by_account {
            let folder = msgs[0].folder_id.clone();
            let folders: Vec<String> = msgs.iter().map(|m| m.folder_id.clone()).collect();
            self.emit("mail://changed", serde_json::json!({ "folderIds": folders }));
            self.emit("mail://new", serde_json::json!({ "accountId": account_id, "folderId": folder, "messages": msgs }));
        }
        self.emit_counts().await;
        Ok(())
    }
}

// ── matching ───────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Terminal {
    Move(String),
    Archive,
    Trash,
    Junk,
}

#[derive(Default)]
struct Plan {
    mark_read: Vec<String>,
    star: Vec<String>,
    tags: BTreeMap<String, Vec<String>>,
    copies: BTreeMap<String, Vec<String>>,
    terminal: BTreeMap<Terminal, Vec<String>>,
    acted: std::collections::BTreeSet<String>,
}

fn haystacks(m: &MessageSummary, f: FilterField) -> Vec<String> {
    let addr = |a: &crate::model::Address| format!("{} <{}>", a.name.clone().unwrap_or_default(), a.email).to_lowercase();
    match f {
        FilterField::From => vec![addr(&m.from)],
        FilterField::To => m.to.iter().map(addr).collect(),
        FilterField::Cc => m.cc.iter().map(addr).collect(),
        FilterField::ToOrCc => m.to.iter().chain(m.cc.iter()).map(addr).collect(),
        FilterField::AnyAddress => std::iter::once(&m.from).chain(m.to.iter()).chain(m.cc.iter()).map(addr).collect(),
        FilterField::Subject => vec![m.subject.to_lowercase()],
        FilterField::HasAttachment => vec![],
    }
}

fn condition_matches(m: &MessageSummary, c: &FilterCondition) -> bool {
    if c.field == FilterField::HasAttachment {
        return match c.op {
            FilterOp::IsFalse | FilterOp::IsNot | FilterOp::NotContains => !m.has_attachments,
            _ => m.has_attachments,
        };
    }
    let v = c.value.trim().to_lowercase();
    let hs = haystacks(m, c.field);
    // For address fields "is" means the bare address OR the whole "Name <addr>" form equals the value.
    let bare = |h: &str| h.rsplit_once('<').map(|(_, r)| r.trim_end_matches('>').to_string()).unwrap_or_else(|| h.to_string());
    let any = |pred: &dyn Fn(&str) -> bool| hs.iter().any(|h| pred(h));
    match c.op {
        FilterOp::Contains => any(&|h| h.contains(&v)),
        FilterOp::NotContains => !any(&|h| h.contains(&v)),
        FilterOp::Is => any(&|h| h == v || bare(h) == v),
        FilterOp::IsNot => !any(&|h| h == v || bare(h) == v),
        FilterOp::BeginsWith => any(&|h| h.starts_with(&v) || bare(h).starts_with(&v)),
        FilterOp::EndsWith => any(&|h| h.ends_with(&v) || bare(h).ends_with(&v)),
        FilterOp::MatchesRegex => match RegexBuilder::new(&c.value).case_insensitive(true).size_limit(1 << 20).build() {
            Ok(re) => any(&|h| re.is_match(h)),
            Err(_) => false,
        },
        FilterOp::IsTrue | FilterOp::IsFalse => false,
    }
}

fn rule_matches(m: &MessageSummary, r: &FilterRule) -> bool {
    if r.match_all {
        r.conditions.iter().all(|c| condition_matches(m, c))
    } else {
        r.conditions.iter().any(|c| condition_matches(m, c))
    }
}

/// Decide what happens to each message. Rules run in order; a message's first "where it goes" action (move,
/// archive, delete, junk) wins and ends its filtering, as does Stop. Flag/tag/copy actions accumulate.
fn plan_actions(rules: &[FilterRule], msgs: &[MessageSummary]) -> Plan {
    let mut p = Plan::default();
    for m in msgs {
        'rules: for r in rules {
            if !rule_matches(m, r) {
                continue;
            }
            p.acted.insert(m.id.clone());
            for a in &r.actions {
                match a {
                    FilterAction::MarkRead => push_once(&mut p.mark_read, &m.id),
                    FilterAction::Star => push_once(&mut p.star, &m.id),
                    FilterAction::Tag { tag } => push_once(p.tags.entry(tag.clone()).or_default(), &m.id),
                    FilterAction::CopyTo { folder_id } => push_once(p.copies.entry(folder_id.clone()).or_default(), &m.id),
                    FilterAction::Stop => break 'rules,
                    t => {
                        // The first "where it goes" action places the message and ends its filtering.
                        let term = match t {
                            FilterAction::MoveTo { folder_id } => Terminal::Move(folder_id.clone()),
                            FilterAction::Archive => Terminal::Archive,
                            FilterAction::Trash => Terminal::Trash,
                            _ => Terminal::Junk,
                        };
                        p.terminal.entry(term).or_default().push(m.id.clone());
                        break 'rules;
                    }
                }
            }
        }
    }
    p
}

fn push_once(v: &mut Vec<String>, id: &str) {
    if !v.iter().any(|x| x == id) {
        v.push(id.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Address, Flags};

    fn msg(id: &str, from: &str, subject: &str, att: bool) -> MessageSummary {
        MessageSummary {
            id: id.into(),
            account_id: "a".into(),
            folder_id: "1".into(),
            thread_id: id.into(),
            message_id: None,
            subject: subject.into(),
            from: Address { name: Some("Shop News".into()), email: from.into() },
            to: vec![Address { name: None, email: "me@example.test".into() }],
            cc: vec![],
            date: 0,
            snippet: String::new(),
            flags: Flags::default(),
            tags: vec![],
            has_attachments: att,
            size: 0,
            thread_count: 1,
            thread_unread: 0,
            snoozed_until: None,
            person: true,
        }
    }
    fn cond(field: FilterField, op: FilterOp, value: &str) -> FilterCondition {
        FilterCondition { field, op, value: value.into() }
    }
    fn rule(name: &str, match_all: bool, conditions: Vec<FilterCondition>, actions: Vec<FilterAction>) -> FilterRule {
        FilterRule { id: name.into(), name: name.into(), enabled: true, account_id: Some("a".into()), match_all, conditions, actions, sort_order: 0 }
    }

    #[test]
    fn conditions() {
        let m = msg("1", "news@shop.example", "Your Weekly Deals", true);
        assert!(condition_matches(&m, &cond(FilterField::From, FilterOp::Contains, "SHOP.example")));
        assert!(condition_matches(&m, &cond(FilterField::From, FilterOp::Is, "news@shop.example")));
        assert!(condition_matches(&m, &cond(FilterField::From, FilterOp::EndsWith, "@shop.example")));
        assert!(condition_matches(&m, &cond(FilterField::From, FilterOp::Contains, "shop news")), "display name is searched too");
        assert!(!condition_matches(&m, &cond(FilterField::Subject, FilterOp::NotContains, "deals")));
        assert!(condition_matches(&m, &cond(FilterField::Subject, FilterOp::MatchesRegex, r"^your\s+weekly")));
        assert!(condition_matches(&m, &cond(FilterField::HasAttachment, FilterOp::IsTrue, "")));
        assert!(!condition_matches(&m, &cond(FilterField::HasAttachment, FilterOp::IsFalse, "")));
        assert!(!condition_matches(&m, &cond(FilterField::Subject, FilterOp::MatchesRegex, "([unclosed")), "bad regex never matches");
    }

    #[test]
    fn plan_order_terminal_and_stop() {
        let rules = vec![
            rule("tag news", true, vec![cond(FilterField::From, FilterOp::EndsWith, "shop.example")], vec![FilterAction::Tag { tag: "$label5".into() }, FilterAction::MarkRead]),
            rule("move deals", false, vec![cond(FilterField::Subject, FilterOp::Contains, "deals"), cond(FilterField::Subject, FilterOp::Contains, "sale")],
                vec![FilterAction::MoveTo { folder_id: "9".into() }, FilterAction::Star]),
            rule("trash all", true, vec![cond(FilterField::Subject, FilterOp::Contains, "")], vec![FilterAction::Trash]),
        ];
        let deals = msg("1", "news@shop.example", "Weekly deals", false);
        let other = msg("2", "friend@example.com", "Lunch?", false);
        let p = plan_actions(&rules, &[deals, other]);
        assert_eq!(p.tags.get("$label5"), Some(&vec!["1".to_string()]));
        assert_eq!(p.mark_read, vec!["1".to_string()]);
        // Message 1 is moved by rule 2; the Star after the move is not reached and rule 3 never runs for it.
        assert_eq!(p.terminal.get(&Terminal::Move("9".into())), Some(&vec!["1".to_string()]));
        assert!(p.star.is_empty());
        // Message 2 only matches "trash all" (empty contains matches everything).
        assert_eq!(p.terminal.get(&Terminal::Trash), Some(&vec!["2".to_string()]));
        assert_eq!(p.acted.len(), 2);

        let stop = vec![
            rule("stop", true, vec![cond(FilterField::From, FilterOp::Contains, "friend")], vec![FilterAction::Star, FilterAction::Stop]),
            rule("trash all", true, vec![cond(FilterField::Subject, FilterOp::Contains, "")], vec![FilterAction::Trash]),
        ];
        let p = plan_actions(&stop, &[msg("2", "friend@example.com", "Lunch?", false)]);
        assert_eq!(p.star, vec!["2".to_string()]);
        assert!(p.terminal.is_empty(), "Stop prevents later rules");
    }

    #[test]
    fn validation() {
        let folders = |f: &str| if f == "9" { Some("a".to_string()) } else { None };
        let mut r = rule("x", true, vec![cond(FilterField::Subject, FilterOp::Contains, "y")], vec![FilterAction::MoveTo { folder_id: "9".into() }]);
        assert!(validate(&r, folders).is_ok());
        r.account_id = None;
        assert!(validate(&r, folders).is_err(), "move filters must be tied to the folder's account");
        r.account_id = Some("b".into());
        assert!(validate(&r, folders).is_err());
        let bad = rule("x", true, vec![cond(FilterField::Subject, FilterOp::MatchesRegex, "(")], vec![FilterAction::Star]);
        assert_eq!(validate(&bad, folders).unwrap_err().field.as_deref(), Some("conditions"));
        let empty = rule("x", true, vec![cond(FilterField::Subject, FilterOp::Contains, " ")], vec![FilterAction::Star]);
        assert!(validate(&empty, folders).is_err());
    }
}
