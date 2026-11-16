//! Authenticated principals and per-project permissions for remote credentials.
//! The local socket is owner-only and keeps full access as `local`.
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::{collections::BTreeSet, fmt};

pub const LOCAL: &str = "local";
pub const MAX_PATTERNS: usize = 8;
/// Serialized bytes of all patterns of one credential (bounds the `guide` response).
pub const MAX_PATTERN_BYTES: usize = 2048;
pub const DENIED: &str = "project not accessible with this credential";
/// Credential bound to a user who is not invited or was disabled.
pub const INACTIVE: &str = "credential not authorized";
const INVALID: &str = "authentication file requires 1-16 unique entries: name ([a-z0-9._-], max 64) -> 64-hex credential or {\"token\",\"user\",\"read\",\"write\"} with project patterns";

/// Visible user handle: lowercase ASCII, digits and `._-`, starting alphanumeric.
pub fn valid_handle(h: &str) -> bool {
    (1..=32).contains(&h.len())
        && h.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && h.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        && h != LOCAL
}

/// JSON value that rejects repeated object keys at any depth. serde_json keeps the last
/// duplicate silently, which would let a stale line override permissions.
struct Strict(Value);
impl<'de> serde::Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_bool<E>(self, v: bool) -> Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_f64<E>(self, v: f64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_str<E>(self, v: &str) -> Result<Strict, E> {
                Ok(Strict(Value::String(v.to_owned())))
            }
            fn visit_unit<E>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
                let mut items = Vec::new();
                while let Some(Strict(v)) = seq.next_element()? {
                    items.push(v);
                }
                Ok(Strict(Value::Array(items)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
                let mut out = Map::new();
                while let Some((k, Strict(v))) = map.next_entry::<String, Strict>()? {
                    if out.insert(k, v).is_some() {
                        return Err(de::Error::custom("duplicate key"));
                    }
                }
                Ok(Strict(Value::Object(out)))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pattern {
    Any,
    Prefix(String),
    Exact(String),
}
impl Pattern {
    fn parse(raw: &str) -> Option<Self> {
        let valid = |s: &str| {
            !s.trim().is_empty()
                && s.len() <= 128
                && !s.contains('*')
                && !s.chars().any(char::is_control)
        };
        if raw == "*" {
            Some(Self::Any)
        } else if let Some(prefix) = raw.strip_suffix("/*") {
            valid(prefix).then(|| Self::Prefix(format!("{prefix}/")))
        } else {
            valid(raw).then(|| Self::Exact(raw.to_owned()))
        }
    }
    fn display(&self) -> String {
        match self {
            Self::Any => "*".into(),
            Self::Prefix(p) => format!("{p}*"),
            Self::Exact(p) => p.clone(),
        }
    }
    fn matches(&self, project: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Prefix(p) => project.starts_with(p.as_str()),
            Self::Exact(p) => project == p,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    /// Credential name: namespaces idempotency receipts.
    principal: String,
    /// User handle the credential acts for; `None` is the admin user.
    user: Option<String>,
    read: Vec<Pattern>,
    write: Vec<Pattern>,
}
impl Access {
    pub fn local() -> Self {
        Self::full(LOCAL)
    }
    fn full(principal: &str) -> Self {
        Self {
            principal: principal.to_owned(),
            user: None,
            read: vec![Pattern::Any],
            write: vec![Pattern::Any],
        }
    }
    /// Full access to one user's projects (OAuth sessions). `principal` namespaces
    /// idempotency receipts per client.
    pub fn for_user(principal: &str, handle: &str) -> Self {
        Self {
            user: Some(handle.to_owned()),
            ..Self::full(principal)
        }
    }
    pub fn principal(&self) -> &str {
        &self.principal
    }
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }
    pub fn is_local(&self) -> bool {
        self.principal == LOCAL
    }
    pub fn can_write(&self, project: &str) -> bool {
        self.write.iter().any(|p| p.matches(project))
    }
    /// Write permission implies read permission on the same projects.
    pub fn can_read(&self, project: &str) -> bool {
        self.can_write(project) || self.read.iter().any(|p| p.matches(project))
    }
    /// Configured patterns as written in auth.json, for the caller's own guide.
    pub fn read_patterns(&self) -> Vec<String> {
        self.read.iter().map(Pattern::display).collect()
    }
    pub fn write_patterns(&self) -> Vec<String> {
        self.write.iter().map(Pattern::display).collect()
    }
    pub fn reads_everything(&self) -> bool {
        self.read
            .iter()
            .chain(&self.write)
            .any(|p| *p == Pattern::Any)
    }
}

/// Parses `auth.json`. A plain string value is the legacy form with full access.
pub fn parse_auth(raw: &[u8]) -> Result<Vec<(Vec<u8>, Access)>, String> {
    let Strict(Value::Object(entries)) = serde_json::from_slice(raw).map_err(|_| INVALID)? else {
        return Err(INVALID.into());
    };
    if entries.is_empty() || entries.len() > 16 {
        return Err(INVALID.into());
    }
    let hex = |v: &str| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit());
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (name, value) in entries {
        // Visible, unambiguous author names; `local` is reserved for the owner socket.
        let valid_name = name.len() <= 64
            && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
        if !valid_name || name == LOCAL {
            return Err(INVALID.into());
        }
        let (token, access) = match value {
            Value::String(token) => (token, Access::full(&name)),
            Value::Object(fields) => {
                if fields
                    .keys()
                    .any(|k| !["token", "user", "read", "write"].contains(&k.as_str()))
                {
                    return Err(INVALID.into());
                }
                let token = fields
                    .get("token")
                    .and_then(Value::as_str)
                    .ok_or(INVALID)?
                    .to_owned();
                let patterns = |key: &str| -> Result<Vec<Pattern>, String> {
                    match fields.get(key) {
                        None => Ok(Vec::new()),
                        // Bounded so the caller's guide stays within its response budget.
                        Some(Value::Array(items)) if items.len() <= MAX_PATTERNS => items
                            .iter()
                            .map(|v| v.as_str().and_then(Pattern::parse).ok_or(INVALID.into()))
                            .collect(),
                        Some(_) => Err(INVALID.into()),
                    }
                };
                let user = match fields.get("user") {
                    None => None,
                    Some(Value::String(h)) if valid_handle(h) => Some(h.clone()),
                    Some(_) => return Err(INVALID.into()),
                };
                let access = Access {
                    principal: name.clone(),
                    user,
                    read: patterns("read")?,
                    write: patterns("write")?,
                };
                // Count escaped JSON bytes: quotes/backslashes are legal in project names.
                let shown: usize = access
                    .read_patterns()
                    .iter()
                    .chain(&access.write_patterns())
                    .map(|p| Value::String(p.clone()).to_string().len())
                    .sum();
                if (access.read.is_empty() && access.write.is_empty()) || shown > MAX_PATTERN_BYTES
                {
                    return Err(INVALID.into());
                }
                (token, access)
            }
            _ => return Err(INVALID.into()),
        };
        // Identity must be unambiguous: one token maps to exactly one principal.
        if !hex(&token) || !seen.insert(token.to_ascii_lowercase()) {
            return Err(INVALID.into());
        }
        out.push((token.into_bytes(), access));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn one(json: &str) -> Result<Access, String> {
        parse_auth(json.as_bytes()).map(|mut v| v.remove(0).1)
    }
    #[test]
    fn legacy_string_has_full_access() {
        let a = one(&format!(r#"{{"claude":"{}"}}"#, "a".repeat(64))).unwrap();
        assert_eq!(a.principal(), "claude");
        assert!(a.can_write("anything") && a.reads_everything());
    }
    #[test]
    fn patterns_and_write_implies_read() {
        let a = one(&format!(
            r#"{{"ana":{{"token":"{}","read":["equipo/*"],"write":["equipo/api"]}}}}"#,
            "b".repeat(64)
        ))
        .unwrap();
        assert!(a.can_read("equipo/web") && a.can_read("equipo/api"));
        assert!(a.can_write("equipo/api") && !a.can_write("equipo/web"));
        assert!(!a.can_read("equipo") && !a.can_read("equipox/a") && !a.can_read("personal"));
        assert!(!a.reads_everything());
        let w = one(&format!(
            r#"{{"w":{{"token":"{}","write":["x"]}}}}"#,
            "c".repeat(64)
        ))
        .unwrap();
        assert!(w.can_read("x") && !w.can_read("y"));
    }
    fn json_list(n: usize) -> String {
        serde_json::to_string(&(0..n).map(|i| format!("p{i}")).collect::<Vec<_>>()).unwrap()
    }
    #[test]
    fn rejects_invalid_configurations() {
        let t = "d".repeat(64);
        for bad in [
            "{}".to_string(),
            r#"{"a":"short"}"#.to_string(),
            format!(r#"{{"local":"{t}"}}"#),
            format!(r#"{{"a":"{t}","b":"{t}"}}"#),
            format!(r#"{{"a":"{t}","b":"{}"}}"#, t.to_uppercase()),
            format!(r#"{{"a":{{"token":"{t}"}}}}"#),
            format!(r#"{{"a":{{"token":"{t}","read":[]}}}}"#),
            format!(r#"{{"a":{{"token":"{t}","read":[""]}}}}"#),
            format!(r#"{{"a":{{"token":"{t}","read":["/*"]}}}}"#),
            format!(r#"{{"a":{{"token":"{t}","read":["a*"]}}}}"#),
            format!(r#"{{"a":{{"token":"{t}","read":"*"}}}}"#),
            format!(r#"{{"a":{{"token":"{t}","read":["*"],"admin":true}}}}"#),
            r#"{"a":7}"#.to_string(),
            format!(r#"{{"a":"{t}","a":"{}"}}"#, "e".repeat(64)),
            format!(r#"{{"a":{{"token":"{t}","read":{}}}}}"#, json_list(9)),
            format!(r#"{{"a":"{t}","\u0061":"{}"}}"#, "e".repeat(64)),
            format!(r#"{{"a":{{"token":"{t}","read":["x"],"read":["*"]}}}}"#),
            format!(r#"{{"Local":"{t}"}}"#),
            format!(r#"{{"ana\u200b":"{t}"}}"#),
            format!(r#"{{"ana ":"{t}"}}"#),
            format!(r#"{{"-a":"{t}"}}"#),
            format!(r#"["{t}"]"#),
        ] {
            assert!(parse_auth(bad.as_bytes()).is_err(), "{bad}");
        }
    }
}
