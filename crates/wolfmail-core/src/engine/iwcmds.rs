//! IntelligentWolf commands. Only the one message the user acted on is ever sent to IntelligentWolf.
// Source: docs/IW-INTEGRATION.md (routes and bodies) and docs/API.md iw_* commands.
use super::Engine;
use crate::error::{ApiError, Result};
use crate::iw::{clip, IwClient, IwStatus, PairStartPublic};
use crate::mime;
use crate::msgstore;
use serde_json::{json, Value};

/// docs/IW-INTEGRATION.md: "text truncated client-side to 20 000 chars".
const AI_TEXT_LIMIT: usize = 20_000;

impl Engine {
    fn iw(&self) -> IwClient<'_> {
        IwClient { http: &self.inner.http, secrets: self.inner.secrets.as_ref() }
    }

    pub async fn iw_status(&self) -> Result<IwStatus> {
        let s = self.settings_get().await?;
        let mut st = self.iw().status(&s.iw_base_url)?;
        if st.connected {
            // Refresh the name/workspace shown in Settings; offline is fine — the cached values stay.
            if let Ok(me) = self.iw().me().await {
                if let Ok(u) = serde_json::from_value(me["user"].clone()) {
                    st.user = Some(u);
                }
                if let Ok(o) = serde_json::from_value(me["org"].clone()) {
                    st.org = Some(o);
                }
            }
            st.connected = self.iw().device()?.is_some();
        }
        Ok(st)
    }

    pub async fn iw_pair_start(&self, base_url: Option<String>) -> Result<PairStartPublic> {
        let base = match base_url.filter(|b| !b.trim().is_empty()) {
            Some(b) => b,
            None => self.settings_get().await?.iw_base_url,
        };
        let device_name = format!("WolfMail on {}", crate::iw::platform());
        let p = self.iw().pair_start(&base, &device_name, &self.inner.cfg.app_version).await?;
        let public = PairStartPublic { code: p.code.clone(), verify_url: p.verify_url.clone(), expires_at: p.expires_at.clone() };
        (self.inner.opener)(&p.verify_url)?;
        *self.inner.pair.lock() = Some((base, p));
        Ok(public)
    }

    pub async fn iw_pair_poll(&self) -> Result<String> {
        let pending = self.inner.pair.lock().clone();
        let Some((base, p)) = pending else {
            return Err(ApiError::invalid("pair", "Start connecting IntelligentWolf first."));
        };
        let state = self.iw().pair_poll(&base, &p).await?;
        if state == "done" || state == "expired" {
            *self.inner.pair.lock() = None;
        }
        Ok(state)
    }

    pub async fn iw_disconnect(&self) -> Result<()> {
        *self.inner.pair.lock() = None;
        self.iw().disconnect().await
    }

    pub async fn iw_person(&self, email: String) -> Result<Value> {
        if !email.contains('@') {
            return Err(ApiError::invalid("email", "Not an email address."));
        }
        self.iw().person(&email.to_ascii_lowercase()).await
    }

    /// Subject/from/to/date/text of one message, from the cache or the server.
    async fn message_payload(&self, rid: i64) -> Result<(Value, String, String)> {
        let raw = self.raw_bytes(rid).await?;
        let summary = self.store().call(move |c| msgstore::get_summary(c, rid)).await?;
        let body = mime::parse_body(&raw, false);
        let text = body.text.clone().unwrap_or_else(|| body.html.as_deref().map(mime::html_to_text).unwrap_or_default());
        let to: Vec<String> = summary.to.iter().map(|a| a.email.clone()).collect();
        let v = json!({
            "subject": summary.subject,
            "from": summary.from.email,
            "to": to.join(", "),
            "date": summary.date,
            "text": clip(&text, AI_TEXT_LIMIT),
        });
        Ok((v, summary.thread_id, summary.account_id))
    }

    pub async fn iw_ai(&self, action: String, message_id: String, instructions: Option<String>) -> Result<Value> {
        if !["summarise", "draftReply", "triage", "extractContact"].contains(&action.as_str()) {
            return Err(ApiError::invalid("action", "Unknown AI action."));
        }
        let rid: i64 = message_id.parse().map_err(|_| ApiError::not_found("The message"))?;
        let (message, thread_id, account_id) = self.message_payload(rid).await?;
        // Earlier messages of the same conversation whose bodies are already cached (no extra downloads), oldest
        // first, sharing the same 20 000-character budget.
        let earlier = self.thread_get(thread_id, account_id).await?;
        let mut budget = AI_TEXT_LIMIT.saturating_sub(message["text"].as_str().map(|s| s.chars().count()).unwrap_or(0));
        let mut thread = Vec::new();
        for m in earlier.iter().filter(|m| m.id != message_id) {
            let id: i64 = m.id.parse().unwrap_or(-1);
            let raw = self.store().call(move |c| msgstore::body_raw(c, id)).await?;
            let Some(raw) = raw else { continue };
            let b = mime::parse_body(&raw, false);
            let t = b.text.unwrap_or_else(|| b.html.as_deref().map(mime::html_to_text).unwrap_or_default());
            if budget == 0 {
                break;
            }
            let t = clip(&t, budget);
            budget = budget.saturating_sub(t.chars().count());
            thread.push(json!({ "from": m.from.email, "date": m.date, "text": t }));
        }
        let mut body = json!({ "action": action, "message": message });
        if !thread.is_empty() {
            body["thread"] = Value::Array(thread);
        }
        if let Some(i) = instructions.filter(|s| !s.trim().is_empty()) {
            body["instructions"] = Value::String(clip(&i, 2000));
        }
        body["assistantName"] = Value::String(self.settings_get().await?.assistant_name);
        let r = self.iw().ai(body).await?;
        let mut out = json!({ "text": r["text"].as_str().unwrap_or(""), "creditsUsed": r["creditsUsed"].clone() });
        if action == "extractContact" {
            out["fields"] = if r["fields"].is_object() { r["fields"].clone() } else { json!({}) };
            out["note"] = Value::String(r["note"].as_str().unwrap_or("").to_string());
        }
        Ok(out)
    }

    /// Put the email on the person's IW timeline. `include_text` sends the whole text (the "update them with the email
    /// content" right-click); without it only the snippet goes, as before.
    pub async fn iw_log_email(&self, message_id: String, lead_id: Option<String>, contact_id: Option<String>, include_text: bool) -> Result<Value> {
        let rid: i64 = message_id.parse().map_err(|_| ApiError::not_found("The message"))?;
        let s = self.store().call(move |c| msgstore::get_summary(c, rid)).await?;
        let text = if include_text { Some(self.message_payload(rid).await?.0["text"].as_str().unwrap_or("").to_string()) } else { None };
        let acct = self.account(&s.account_id).await?;
        let outgoing = acct.identities.iter().any(|i| i.email.eq_ignore_ascii_case(&s.from.email));
        let to: Vec<String> = s.to.iter().map(|a| a.email.clone()).collect();
        let mut body = json!({
            "direction": if outgoing { "out" } else { "in" },
            "messageId": s.message_id.clone().unwrap_or_default(),
            "subject": s.subject,
            "from": s.from.email,
            "to": to.join(", "),
            "date": s.date,
            "snippet": s.snippet,
        });
        if let Some(l) = lead_id {
            body["leadId"] = Value::String(l);
        }
        if let Some(c) = contact_id {
            body["contactId"] = Value::String(c);
        }
        if let Some(t) = text {
            body["text"] = Value::String(t);
        }
        if let Some(n) = s.from.name.clone().filter(|n| !n.trim().is_empty()) {
            body["fromName"] = Value::String(n);
        }
        let r = self.iw().log_email(body).await?;
        Ok(json!({ "leadId": r["leadId"].clone(), "contactId": r["contactId"].clone() }))
    }

    pub async fn iw_create_task(&self, message_id: String, title: String, due: Option<Value>) -> Result<Value> {
        if title.trim().is_empty() {
            return Err(ApiError::invalid("title", "Give the task a title."));
        }
        let rid: i64 = message_id.parse().map_err(|_| ApiError::not_found("The message"))?;
        let s = self.store().call(move |c| msgstore::get_summary(c, rid)).await?;
        let mut body = json!({
            "title": title.trim(),
            "email": { "messageId": s.message_id.clone().unwrap_or_default(), "subject": s.subject, "from": s.from.email },
        });
        if let Some(d) = due.filter(|d| !d.is_null()) {
            body["due"] = d;
        }
        let r = self.iw().create_task(body).await?;
        Ok(json!({ "taskId": r["taskId"].clone(), "url": r["url"].clone() }))
    }

    pub async fn iw_booking_link(&self) -> Result<Value> {
        self.iw().booking_links().await
    }

    pub async fn iw_business_mailboxes(&self) -> Result<Value> {
        self.iw().business_mailboxes().await
    }

    /// Which of these addresses are people in IntelligentWolf (for highlighting the list). Only addresses are sent.
    pub async fn iw_people(&self, emails: Vec<String>) -> Result<Value> {
        let mut clean: Vec<String> = emails.into_iter().map(|e| e.trim().to_ascii_lowercase()).filter(|e| e.contains('@')).collect();
        clean.sort();
        clean.dedup();
        if clean.is_empty() {
            return Ok(json!({}));
        }
        let mut out = serde_json::Map::new();
        // Source: IntelligentWolf's WolfMail routes: WOLFMAIL_PEOPLE_MAX = 200 per request.
        for chunk in clean.chunks(200) {
            let r = self.iw().people(chunk).await?;
            if let Some(m) = r["matches"].as_object() {
                out.extend(m.clone());
            }
        }
        Ok(Value::Object(out))
    }

    /// Ask: the assistant turns a question into a search; WolfMail runs it locally (search.rs). `today` is the user's
    /// local date (YYYY-MM-DD) so "last week" means their week.
    pub async fn iw_search_query(&self, question: String, today: String) -> Result<Value> {
        let q = question.trim();
        if q.is_empty() {
            return Err(ApiError::invalid("question", "Ask a question first."));
        }
        let body = json!({ "question": clip(q, 500), "today": today, "assistantName": self.settings_get().await?.assistant_name });
        self.iw().search_query(body).await
    }

    pub async fn iw_contact_update(&self, kind: String, id: String, fields: Value, note: Option<String>) -> Result<Value> {
        if kind != "lead" && kind != "contact" {
            return Err(ApiError::invalid("kind", "Unknown kind of person."));
        }
        let mut body = json!({ "kind": kind, "id": id, "fields": if fields.is_object() { fields } else { json!({}) } });
        if let Some(n) = note.filter(|n| !n.trim().is_empty()) {
            body["note"] = Value::String(clip(n.trim(), 20_000));
        }
        self.iw().contact_update(body).await
    }

    /// Add the other party of this message to IntelligentWolf as a lead.
    pub async fn iw_contact_create(&self, email: String, fields: Value) -> Result<Value> {
        if !email.contains('@') {
            return Err(ApiError::invalid("email", "Not an email address."));
        }
        let body = json!({ "email": email.trim().to_ascii_lowercase(), "fields": if fields.is_object() { fields } else { json!({}) } });
        self.iw().contact_create(body).await
    }

    /// One message to the assistant. With `message_id`, the email the user has open goes with it (they choose).
    pub async fn iw_chat_send(&self, message: String, message_id: Option<String>, new_thread: bool) -> Result<Value> {
        if message.trim().is_empty() {
            return Err(ApiError::invalid("message", "Type a message first."));
        }
        let mut body = json!({
            "message": clip(message.trim(), 8000),
            "assistantName": self.settings_get().await?.assistant_name,
            "newThread": new_thread,
        });
        if let Some(id) = message_id {
            let rid: i64 = id.parse().map_err(|_| ApiError::not_found("The message"))?;
            let (m, _, _) = self.message_payload(rid).await?;
            body["context"] = json!({ "subject": m["subject"], "from": m["from"], "date": m["date"], "text": m["text"] });
        }
        self.iw().chat_send(body).await
    }

    pub async fn iw_chat_poll(&self, turn: String) -> Result<Value> {
        self.iw().chat_turn(&turn).await
    }

    pub async fn iw_chat_history(&self) -> Result<Value> {
        self.iw().chat_history().await
    }
}
