//! The content-line layer shared by iCalendar and vCard: unfolding, parameters, value escaping, components, and
//! the reverse (folding at 75 octets, escaping, CRLF).
//! Source: RFC 5545 §3.1 (content lines, folding), §3.1.1 (list/field separators), §3.2 (parameters, quoted
//! values), §3.3.11 (TEXT escaping: \\ \; \, \n); RFC 6350 §3.2 (vCard uses the same folding) and §3.4 (escaping).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prop {
    pub name: String,                 // upper-cased
    pub params: Vec<(String, String)>, // names upper-cased, values unquoted
    pub value: String,                // raw (still escaped); use `text()` for TEXT values
}

impl Prop {
    pub fn new(name: &str, value: impl Into<String>) -> Self {
        Prop { name: name.to_ascii_uppercase(), params: Vec::new(), value: value.into() }
    }
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn with_param(mut self, name: &str, value: impl Into<String>) -> Self {
        self.params.push((name.to_ascii_uppercase(), value.into()));
        self
    }
    /// The value as TEXT (unescaped).
    pub fn text(&self) -> String {
        unescape(&self.value)
    }
    /// A TEXT list value split on unescaped commas (e.g. CATEGORIES).
    pub fn text_list(&self) -> Vec<String> {
        split_unescaped(&self.value, ',').into_iter().map(|s| unescape(&s)).filter(|s| !s.is_empty()).collect()
    }
    /// A structured value split on unescaped semicolons (e.g. vCard N, ADR).
    pub fn fields(&self) -> Vec<String> {
        split_unescaped(&self.value, ';').into_iter().map(|s| unescape(&s)).collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Component {
    pub name: String, // VCALENDAR, VEVENT, VALARM, VTIMEZONE, VCARD …
    pub props: Vec<Prop>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn new(name: &str) -> Self {
        Component { name: name.to_ascii_uppercase(), ..Default::default() }
    }
    pub fn get(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Prop> + 'a {
        self.props.iter().filter(move |p| p.name.eq_ignore_ascii_case(name))
    }
    pub fn text(&self, name: &str) -> Option<String> {
        self.get(name).map(|p| p.text()).filter(|s| !s.is_empty())
    }
    pub fn push(&mut self, p: Prop) {
        self.props.push(p);
    }
    pub fn push_text(&mut self, name: &str, value: &str) {
        if !value.is_empty() {
            self.props.push(Prop::new(name, escape(value)));
        }
    }
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Component> + 'a {
        self.children.iter().filter(move |c| c.name.eq_ignore_ascii_case(name))
    }
}

pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => o.push_str("\\\\"),
            ';' => o.push_str("\\;"),
            ',' => o.push_str("\\,"),
            '\n' => o.push_str("\\n"),
            '\r' => {}
            c => o.push(c),
        }
    }
    o
}

pub fn unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => o.push('\n'),
                Some(x) => o.push(x),
                None => o.push('\\'),
            }
        } else {
            o.push(c);
        }
    }
    o
}

fn split_unescaped(s: &str, sep: char) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut esc = false;
    for c in s.chars() {
        if esc {
            out.last_mut().unwrap().push('\\');
            out.last_mut().unwrap().push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == sep {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out
}

/// Unfold (a CRLF or LF followed by one space/tab continues the previous line) and split into logical lines.
fn unfold(input: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in input.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

/// Parse one content line: name *(";" param) ":" value. Quoted parameter values may contain ; : ,
pub fn parse_line(line: &str) -> Option<Prop> {
    let mut name = String::new();
    let mut params = Vec::new();
    let b: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < b.len() && b[i] != ';' && b[i] != ':' {
        name.push(b[i]);
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    while i < b.len() && b[i] == ';' {
        i += 1;
        let mut pname = String::new();
        while i < b.len() && b[i] != '=' && b[i] != ';' && b[i] != ':' {
            pname.push(b[i]);
            i += 1;
        }
        let mut pval = String::new();
        if i < b.len() && b[i] == '=' {
            i += 1;
            let mut quoted = false;
            while i < b.len() {
                let c = b[i];
                if c == '"' {
                    quoted = !quoted;
                } else if !quoted && (c == ';' || c == ':') {
                    break;
                } else {
                    pval.push(c);
                }
                i += 1;
            }
        }
        // vCard 2.1-style bare parameters ("TEL;CELL:…") become TYPE=CELL.
        if pval.is_empty() && !pname.contains('=') {
            params.push(("TYPE".to_string(), pname.to_ascii_uppercase()));
        } else {
            params.push((pname.to_ascii_uppercase(), pval));
        }
    }
    if i >= b.len() || b[i] != ':' {
        return None;
    }
    let value: String = b[i + 1..].iter().collect();
    // Grouped vCard names ("item1.EMAIL") keep only the property name.
    let name = name.rsplit('.').next().unwrap_or(&name).to_ascii_uppercase();
    Some(Prop { name, params, value })
}

/// Parse a whole iCalendar/vCard stream into top-level components (lines outside BEGIN/END are ignored).
pub fn parse(input: &str) -> Vec<Component> {
    let mut stack: Vec<Component> = Vec::new();
    let mut top: Vec<Component> = Vec::new();
    for line in unfold(input) {
        let Some(p) = parse_line(&line) else { continue };
        if p.name == "BEGIN" {
            stack.push(Component::new(p.value.trim()));
        } else if p.name == "END" {
            if let Some(done) = stack.pop() {
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => top.push(done),
                }
            }
        } else if let Some(cur) = stack.last_mut() {
            cur.props.push(p);
        }
    }
    // Unterminated components (truncated files) are still returned rather than silently dropped.
    while let Some(done) = stack.pop() {
        match stack.last_mut() {
            Some(parent) => parent.children.push(done),
            None => top.push(done),
        }
    }
    top
}

fn quote_param(v: &str) -> String {
    if v.contains([';', ':', ',']) {
        format!("\"{}\"", v.replace('"', "'"))
    } else {
        v.to_string()
    }
}

/// Fold to 75 octets per line without splitting a UTF-8 character (RFC 5545 §3.1).
fn fold(line: &str, out: &mut String) {
    let mut count = 0usize;
    for ch in line.chars() {
        let n = ch.len_utf8();
        if count + n > 75 {
            out.push_str("\r\n ");
            count = 1;
        }
        out.push(ch);
        count += n;
    }
    out.push_str("\r\n");
}

pub fn write(c: &Component, out: &mut String) {
    fold(&format!("BEGIN:{}", c.name), out);
    for p in &c.props {
        let mut l = p.name.clone();
        for (k, v) in &p.params {
            l.push(';');
            l.push_str(k);
            l.push('=');
            l.push_str(&quote_param(v));
        }
        l.push(':');
        l.push_str(&p.value);
        fold(&l, out);
    }
    for ch in &c.children {
        write(ch, out);
    }
    fold(&format!("END:{}", c.name), out);
}

pub fn to_string(c: &Component) -> String {
    let mut s = String::new();
    write(c, &mut s);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfolds_params_and_escapes() {
        let src = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:Team\\, weekly\\; room\\n2\r\nATTENDEE;CN=\"Doe; Jane\";PARTSTAT=ACCEPTED:mailto:j\r\n @x.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let top = parse(src);
        let ev = &top[0].children[0];
        assert_eq!(ev.text("SUMMARY").unwrap(), "Team, weekly; room\n2");
        let a = ev.get("ATTENDEE").unwrap();
        assert_eq!(a.param("CN"), Some("Doe; Jane"));
        assert_eq!(a.value, "mailto:j@x.org");
    }

    #[test]
    fn folds_at_75_octets_and_roundtrips() {
        let mut c = Component::new("VEVENT");
        let long = "é".repeat(60) + " end";
        c.push_text("DESCRIPTION", &long);
        let s = to_string(&c);
        assert!(s.lines().all(|l| l.len() <= 75 + 1));
        let back = parse(&s);
        assert_eq!(back[0].text("DESCRIPTION").unwrap(), long);
    }

    #[test]
    fn vcard_groups_and_bare_params() {
        let top = parse("BEGIN:VCARD\nVERSION:2.1\nitem1.EMAIL;INTERNET:a@b.c\nTEL;CELL:123\nN:Doe;Jane;;;\nEND:VCARD\n");
        let v = &top[0];
        assert_eq!(v.get("EMAIL").unwrap().value, "a@b.c");
        assert_eq!(v.get("TEL").unwrap().param("TYPE"), Some("CELL"));
        assert_eq!(v.get("N").unwrap().fields()[1], "Jane");
    }
}
