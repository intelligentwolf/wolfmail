//! Search query language: free words plus `from:` `to:` `subject:` `has:attachment` `is:unread` `is:flagged`
//! `is:read` `tag:` and quoted phrases. Compiled to an FTS5 MATCH (Source: SQLite FTS5 docs §3 "Full-text Query
//! Syntax" — column filters `col : term`, prefix `"term"*`, implicit AND) plus plain SQL conditions.
use rusqlite::types::Value;

#[derive(Debug, Default, PartialEq)]
pub struct ParsedQuery {
    pub fts: Vec<String>,
    pub unread: Option<bool>,
    pub flagged: bool,
    pub attachments: bool,
    pub tag: Option<String>,
    /// `after:YYYY-MM-DD` (on or after that day) and `before:YYYY-MM-DD` (before that day), local midnight in ms —
    /// Gmail's meaning of the same operators, so a query the assistant writes reads the same in both.
    pub after_ms: Option<i64>,
    pub before_ms: Option<i64>,
}

/// Local midnight at the start of `YYYY-MM-DD`, in unix ms.
fn day_start_ms(v: &str) -> Option<i64> {
    use chrono::TimeZone;
    let d = chrono::NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d").ok()?;
    let t = d.and_hms_opt(0, 0, 0)?;
    chrono::Local.from_local_datetime(&t).earliest().map(|x| x.timestamp_millis())
}

fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// One FTS5 string literal, prefix-matched unless it was a quoted phrase.
fn fts_term(raw: &str) -> Option<String> {
    let phrase = raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"');
    let inner = raw.trim_matches('"');
    let cleaned: String = inner.chars().filter(|c| *c != '"').collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return None;
    }
    Some(if phrase { format!("\"{cleaned}\"") } else { format!("\"{cleaned}\"*") })
}

pub fn parse_query(s: &str) -> ParsedQuery {
    let mut q = ParsedQuery::default();
    for t in tokens(s) {
        let lower = t.to_ascii_lowercase();
        let (key, val) = match lower.split_once(':') {
            Some((k, _)) if ["from", "to", "subject", "has", "is", "tag", "cc", "after", "before"].contains(&k) => (k.to_string(), t[k.len() + 1..].to_string()),
            _ => (String::new(), t.clone()),
        };
        match key.as_str() {
            "from" => {
                if let Some(v) = fts_term(&val) {
                    q.fts.push(format!("sender : {v}"));
                }
            }
            "to" | "cc" => {
                if let Some(v) = fts_term(&val) {
                    q.fts.push(format!("recipients : {v}"));
                }
            }
            "subject" => {
                if let Some(v) = fts_term(&val) {
                    q.fts.push(format!("subject : {v}"));
                }
            }
            "has" if val.eq_ignore_ascii_case("attachment") || val.eq_ignore_ascii_case("attachments") => q.attachments = true,
            "is" if val.eq_ignore_ascii_case("unread") => q.unread = Some(true),
            "is" if val.eq_ignore_ascii_case("read") => q.unread = Some(false),
            "is" if val.eq_ignore_ascii_case("flagged") || val.eq_ignore_ascii_case("starred") => q.flagged = true,
            "tag" if !val.is_empty() => q.tag = Some(val),
            "after" if day_start_ms(&val).is_some() => q.after_ms = day_start_ms(&val),
            "before" if day_start_ms(&val).is_some() => q.before_ms = day_start_ms(&val),
            _ => {
                if let Some(v) = fts_term(&t) {
                    q.fts.push(v);
                }
            }
        }
    }
    q
}

impl ParsedQuery {
    pub fn fts_expr(&self) -> Option<String> {
        if self.fts.is_empty() {
            None
        } else {
            Some(self.fts.join(" AND "))
        }
    }

    /// SQL over the `messages m` alias.
    pub fn to_sql(&self) -> (String, Vec<Value>) {
        let mut c = Vec::new();
        let mut p = Vec::new();
        if let Some(expr) = self.fts_expr() {
            c.push("m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)".to_string());
            p.push(Value::Text(expr));
        }
        match self.unread {
            Some(true) => c.push("m.seen=0".into()),
            Some(false) => c.push("m.seen=1".into()),
            None => {}
        }
        if self.flagged {
            c.push("m.flagged=1".into());
        }
        if self.attachments {
            c.push("m.has_attachments=1".into());
        }
        if let Some(t) = &self.tag {
            c.push("EXISTS (SELECT 1 FROM json_each(m.keywords) j JOIN tags tg ON tg.key = j.value WHERE lower(tg.name)=lower(?) OR lower(j.value)=lower(?))".into());
            p.push(Value::Text(t.clone()));
            p.push(Value::Text(t.clone()));
        }
        if let Some(a) = self.after_ms {
            c.push("m.date >= ?".into());
            p.push(Value::Integer(a));
        }
        if let Some(b) = self.before_ms {
            c.push("m.date < ?".into());
            p.push(Value::Integer(b));
        }
        (c.join(" AND "), p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_operators() {
        let q = parse_query("from:ann subject:\"march invoice\" has:attachment is:unread pay");
        assert_eq!(q.fts, vec!["sender : \"ann\"*", "subject : \"march invoice\"", "\"pay\"*"]);
        assert!(q.attachments);
        assert_eq!(q.unread, Some(true));
        let (sql, p) = q.to_sql();
        assert!(sql.contains("MATCH ?"));
        assert_eq!(p.len(), 1);
    }
    #[test]
    fn date_operators() {
        let q = parse_query("from:sarah after:2026-10-21 before:2026-10-28 quote");
        assert!(q.after_ms.is_some() && q.before_ms.is_some() && q.after_ms < q.before_ms);
        let (sql, p) = q.to_sql();
        assert!(sql.contains("m.date >= ?") && sql.contains("m.date < ?"));
        assert_eq!(p.len(), 3);
        // A malformed date is just a word.
        let w = parse_query("after:soon");
        assert!(w.after_ms.is_none() && w.fts.len() == 1);
    }

    #[test]
    fn strips_injection_quotes() {
        let q = parse_query("a\"b OR c");
        assert_eq!(q.fts, vec!["\"ab OR c\"*"]);
    }
    #[test]
    fn plain_colon_words_are_text() {
        let q = parse_query("re:meeting 10:30");
        assert_eq!(q.fts, vec!["\"re:meeting\"*", "\"10:30\"*"]);
    }
}
