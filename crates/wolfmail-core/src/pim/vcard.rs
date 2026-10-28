//! vCard 3.0 (RFC 2426) / 4.0 (RFC 6350) ↔ `Contact`.
//! Sources: RFC 6350 §6.2.1 FN, §6.2.2 N (family;given;additional;prefix;suffix), §6.2.5 BDAY ("--MMDD" when the
//! year is unknown; RFC 2426 uses YYYY-MM-DD), §6.3.1 ADR (pobox;ext;street;locality;region;code;country),
//! §6.4.1 TEL, §6.4.2 EMAIL, §6.6.4 ORG, §6.6.1 TITLE, §6.7.1 CATEGORIES, §6.7.2 NOTE, §6.2.4 PHOTO (data: URI in 4.0,
//! ENCODING=b + TYPE in 3.0), TYPE parameter values (home, work, cell/mobile …). WolfMail writes 3.0 because every
//! CardDAV server accepts it (RFC 6352 §5.1 requires 3.0 support).
use super::lines::{self, escape, Component, Prop};
use super::model::{Contact, Labeled};

fn label_of(p: &Prop) -> String {
    let mut types: Vec<String> = p
        .params
        .iter()
        .filter(|(k, _)| k == "TYPE")
        .flat_map(|(_, v)| v.split(',').map(|s| s.trim().to_ascii_lowercase()).collect::<Vec<_>>())
        .filter(|t| !matches!(t.as_str(), "pref" | "internet" | "voice" | "x400"))
        .collect();
    types.dedup();
    match types.first().map(String::as_str) {
        Some("cell") => "mobile".into(),
        Some(t) => t.to_string(),
        None => String::new(),
    }
}

fn type_param(label: &str, kind: &str) -> Option<String> {
    let l = label.trim().to_ascii_lowercase();
    if l.is_empty() {
        return None;
    }
    Some(match (kind, l.as_str()) {
        ("TEL", "mobile") => "CELL".into(),
        _ => l.to_ascii_uppercase(),
    })
}

fn bday_from(v: &str) -> Option<String> {
    let v = v.trim();
    let digits: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
    if let Some(rest) = v.strip_prefix("--") {
        let d: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
        if d.len() >= 4 {
            return Some(format!("--{}-{}", &d[0..2], &d[2..4]));
        }
        return None;
    }
    if digits.len() >= 8 {
        return Some(format!("{}-{}-{}", &digits[0..4], &digits[4..6], &digits[6..8]));
    }
    None
}

fn photo_from(p: &Prop) -> Option<String> {
    let v = p.value.trim();
    if v.starts_with("data:") {
        return Some(v.to_string());
    }
    let enc = p.param("ENCODING").unwrap_or("");
    if enc.eq_ignore_ascii_case("b") || enc.eq_ignore_ascii_case("BASE64") {
        let t = p.param("TYPE").unwrap_or("JPEG").to_ascii_lowercase();
        let mime = if t.contains('/') { t } else { format!("image/{t}") };
        return Some(format!("data:{mime};base64,{}", v.replace([' ', '\r', '\n'], "")));
    }
    None
}

pub fn contacts_from_vcf(vcf: &str, book_id: &str) -> Vec<Contact> {
    lines::parse(vcf).into_iter().filter(|c| c.name == "VCARD").map(|v| contact_from_vcard(&v, book_id)).collect()
}

pub fn contact_from_vcard(v: &Component, book_id: &str) -> Contact {
    let n = v.get("N").map(|p| p.fields()).unwrap_or_default();
    let family = n.first().cloned().unwrap_or_default();
    let given = n.get(1).cloned().unwrap_or_default();
    let full = v.text("FN").unwrap_or_else(|| [given.as_str(), family.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" "));
    let labeled = |name: &str| -> Vec<Labeled> {
        v.all(name)
            .map(|p| Labeled { label: label_of(p), value: p.text().trim().trim_start_matches("mailto:").trim_start_matches("tel:").to_string() })
            .filter(|l| !l.value.is_empty())
            .collect()
    };
    let addresses = v
        .all("ADR")
        .map(|p| {
            let f = p.fields();
            // pobox;ext;street;locality;region;code;country → one readable line set
            let parts: Vec<String> = [2usize, 1, 0, 3, 4, 5, 6].iter().filter_map(|i| f.get(*i).cloned()).filter(|s| !s.trim().is_empty()).collect();
            Labeled { label: label_of(p), value: parts.join(", ") }
        })
        .filter(|l| !l.value.is_empty())
        .collect();
    Contact {
        id: String::new(),
        book_id: book_id.to_string(),
        uid: v.text("UID").unwrap_or_default(),
        full_name: full,
        given_name: given,
        family_name: family,
        org: v.get("ORG").map(|p| p.fields().into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
        title: v.text("TITLE").unwrap_or_default(),
        emails: labeled("EMAIL"),
        phones: labeled("TEL"),
        addresses,
        notes: v.text("NOTE").unwrap_or_default(),
        birthday: v.get("BDAY").and_then(|p| bday_from(&p.value)),
        categories: v.get("CATEGORIES").map(|p| p.text_list()).unwrap_or_default(),
        photo: v.get("PHOTO").and_then(photo_from),
        read_only: false,
    }
}

pub fn vcard(c: &Contact) -> String {
    let mut v = Component::new("VCARD");
    v.push(Prop::new("VERSION", "3.0"));
    v.push(Prop::new("PRODID", escape(super::ical::PRODID)));
    v.push(Prop::new("UID", escape(&c.uid)));
    let full = if c.full_name.trim().is_empty() {
        let n = format!("{} {}", c.given_name, c.family_name).trim().to_string();
        if n.is_empty() { c.emails.first().map(|e| e.value.clone()).unwrap_or_default() } else { n }
    } else {
        c.full_name.clone()
    };
    v.push(Prop::new("FN", escape(&full)));
    v.push(Prop::new("N", format!("{};{};;;", escape(&c.family_name), escape(&c.given_name))));
    if !c.org.is_empty() {
        v.push(Prop::new("ORG", escape(&c.org)));
    }
    v.push_text("TITLE", &c.title);
    for (name, list) in [("EMAIL", &c.emails), ("TEL", &c.phones)] {
        for l in list {
            let mut p = Prop::new(name, escape(&l.value));
            if name == "EMAIL" {
                p = p.with_param("TYPE", "INTERNET");
            }
            if let Some(t) = type_param(&l.label, name) {
                p = p.with_param("TYPE", t);
            }
            v.push(p);
        }
    }
    for a in &c.addresses {
        // Free-text address goes in the street field (RFC 6350 allows a single component).
        let mut p = Prop::new("ADR", format!(";;{};;;;", escape(&a.value)));
        if let Some(t) = type_param(&a.label, "ADR") {
            p = p.with_param("TYPE", t);
        }
        v.push(p);
    }
    v.push_text("NOTE", &c.notes);
    if let Some(b) = &c.birthday {
        v.push(Prop::new("BDAY", b.clone()));
    }
    if !c.categories.is_empty() {
        v.push(Prop::new("CATEGORIES", c.categories.iter().map(|s| escape(s)).collect::<Vec<_>>().join(",")));
    }
    if let Some(ph) = &c.photo {
        // data:image/jpeg;base64,XXXX → 3.0 form
        if let Some((meta, data)) = ph.strip_prefix("data:").and_then(|r| r.split_once(',')) {
            let t = meta.split(';').next().unwrap_or("image/jpeg").trim_start_matches("image/").to_ascii_uppercase();
            v.push(Prop::new("PHOTO", data.to_string()).with_param("ENCODING", "b").with_param("TYPE", t));
        }
    }
    lines::to_string(&v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vcard4_and_3_parse() {
        let src = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:u1\r\nFN:Sarah Mitchell\r\nN:Mitchell;Sarah;;;\r\nEMAIL;TYPE=work:sarah@brightbloom.example\r\nTEL;TYPE=cell:+44 7700 900123\r\nADR;TYPE=work:;;1 High St;Leeds;;LS1 1AA;UK\r\nORG:Bright Bloom;Shop\r\nBDAY:--0412\r\nCATEGORIES:Clients,VIP\r\nNOTE:Prefers calls\\, not texts\r\nEND:VCARD\r\nBEGIN:VCARD\r\nVERSION:3.0\r\nFN:Tom\r\nEMAIL;TYPE=INTERNET;TYPE=HOME:tom@x.org\r\nBDAY:1980-05-01\r\nEND:VCARD\r\n";
        let cs = contacts_from_vcf(src, "b");
        assert_eq!(cs.len(), 2);
        let s = &cs[0];
        assert_eq!(s.given_name, "Sarah");
        assert_eq!(s.emails[0], Labeled { label: "work".into(), value: "sarah@brightbloom.example".into() });
        assert_eq!(s.phones[0].label, "mobile");
        assert_eq!(s.addresses[0].value, "1 High St, Leeds, LS1 1AA, UK");
        assert_eq!(s.org, "Bright Bloom, Shop");
        assert_eq!(s.birthday.as_deref(), Some("--04-12"));
        assert_eq!(s.categories, vec!["Clients", "VIP"]);
        assert_eq!(s.notes, "Prefers calls, not texts");
        assert_eq!(cs[1].emails[0].label, "home");
        assert_eq!(cs[1].birthday.as_deref(), Some("1980-05-01"));
    }

    #[test]
    fn roundtrip() {
        let c = Contact {
            book_id: "b".into(),
            uid: "abc".into(),
            full_name: "Jane Doe".into(),
            given_name: "Jane".into(),
            family_name: "Doe".into(),
            org: "Acme".into(),
            emails: vec![Labeled { label: "work".into(), value: "jane@acme.test".into() }],
            phones: vec![Labeled { label: "mobile".into(), value: "+1 555".into() }],
            categories: vec!["A, B".into()],
            birthday: Some("1990-02-03".into()),
            photo: Some("data:image/png;base64,iVBORw0KGgo=".into()),
            ..Default::default()
        };
        let back = &contacts_from_vcf(&vcard(&c), "b")[0];
        assert_eq!(back.full_name, c.full_name);
        assert_eq!(back.emails, c.emails);
        assert_eq!(back.phones, c.phones);
        assert_eq!(back.categories, c.categories);
        assert_eq!(back.birthday, c.birthday);
        assert_eq!(back.photo, c.photo);
        assert_eq!(back.uid, "abc");
    }
}
