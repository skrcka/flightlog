//! Secret redaction (spec §4): each distinct secret becomes a stable
//! placeholder `[REDACTED:<kind>:<n>]`; the report never holds the value.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use regex::{Captures, Regex};
use serde_json::{json, Value};

struct Rule {
    kind: &'static str,
    re: Regex,
    /// Capture group holding the secret (0 = whole match).
    group: usize,
}

fn rules() -> &'static [Rule] {
    static R: OnceLock<Vec<Rule>> = OnceLock::new();
    R.get_or_init(|| {
        let r = |kind, re: &str, group| Rule {
            kind,
            re: Regex::new(re).expect("rule"),
            group,
        };
        vec![
            r(
                "private_key",
                r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |ENCRYPTED |PGP )?PRIVATE KEY(?: BLOCK)?-----[\s\S]*?-----END (?:RSA |EC |DSA |OPENSSH |ENCRYPTED |PGP )?PRIVATE KEY(?: BLOCK)?-----",
                0,
            ),
            r(
                "private_key",
                r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |ENCRYPTED |PGP )?PRIVATE KEY(?: BLOCK)?-----",
                0,
            ),
            // URL credentials: only the password part.
            r(
                "password",
                r#"\b[a-z][a-z0-9+.\-]{1,20}://[^\s/:@"'\\]{1,64}:([^\s@"'\\\[]{1,256})@"#,
                1,
            ),
            r("api_key", r"\bAKIA[0-9A-Z]{16}\b", 0),
            r("api_key", r"\bsk-(?:ant-|proj-)?[A-Za-z0-9_\-]{32,}", 0),
            r("api_key", r"\bsk_live_[0-9A-Za-z]{24,}\b", 0),
            r("api_key", r"\bAIza[0-9A-Za-z_\-]{35}\b", 0),
            r("token", r"\bgh[pousr]_[A-Za-z0-9]{36,}\b", 0),
            r("token", r"\bgithub_pat_[A-Za-z0-9_]{60,}\b", 0),
            r("token", r"\bglpat-[A-Za-z0-9_\-]{20,}\b", 0),
            r("token", r"\bxox[abprs]-[A-Za-z0-9\-]{10,}", 0),
            r("token", r"\bhf_[A-Za-z0-9]{30,}\b", 0),
            r("token", r"\bnpm_[A-Za-z0-9]{36}\b", 0),
            r("token", r"\b(?:mcp|api)_[0-9a-f]{32,}\b", 0),
            r(
                "token",
                r"\beyJ[A-Za-z0-9_\-]{10,}\.eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
                0,
            ),
            r("token", r"(?i)\bbearer\s+([A-Za-z0-9._\-]{24,})", 1),
            // .env style: FOO_SECRET=value, API_KEY=value, DB_PASSWORD=value
            r(
                "password",
                r#"(?m)\b[A-Z0-9_]*(?:SECRET|PASSWORD|PASSWD|TOKEN|API_KEY|PRIVATE_KEY)[A-Z0-9_]*\s*=\s*["']?([^\s"'\[]{8,})"#,
                1,
            ),
        ]
    })
}

/// A redaction finding (spec §4 `manifest.redaction.findings[]`).
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub placeholder: String,
    pub kind: &'static str,
    pub count: usize,
    pub files: BTreeSet<String>,
}

#[derive(Default)]
pub struct Redactor {
    values: HashMap<String, String>,
    per_kind: HashMap<&'static str, usize>,
    findings: BTreeMap<String, Finding>,
}

impl Redactor {
    fn placeholder(&mut self, kind: &'static str, secret: &str) -> String {
        if let Some(p) = self.values.get(secret) {
            return p.clone();
        }
        let n = self.per_kind.entry(kind).or_default();
        *n += 1;
        let p = format!("[REDACTED:{kind}:{n}]");
        self.values.insert(secret.to_string(), p.clone());
        p
    }

    fn hit(&mut self, placeholder: &str, kind: &'static str, file: &str) {
        let f = self
            .findings
            .entry(placeholder.to_string())
            .or_insert_with(|| Finding {
                placeholder: placeholder.to_string(),
                kind,
                count: 0,
                files: BTreeSet::new(),
            });
        f.count += 1;
        f.files.insert(file.to_string());
    }

    /// Redact one string; `file` names where it lives, for the report.
    pub fn text(&mut self, s: &str, file: &str) -> String {
        let mut out = s.to_string();
        for rule in rules() {
            if !rule.re.is_match(&out) {
                continue;
            }
            let mut hits: Vec<(String, String)> = Vec::new();
            let replaced = rule
                .re
                .replace_all(&out, |c: &Captures| {
                    let whole = c.get(0).map(|m| m.as_str()).unwrap_or("");
                    let Some(secret) = c.get(rule.group) else {
                        return whole.to_string();
                    };
                    if secret.as_str().starts_with("[REDACTED:") {
                        return whole.to_string();
                    }
                    let ph = self.placeholder(rule.kind, secret.as_str());
                    hits.push((ph.clone(), secret.as_str().to_string()));
                    // Replace only the secret inside the match.
                    let start = secret.start() - c.get(0).unwrap().start();
                    let end = start + secret.as_str().len();
                    format!("{}{}{}", &whole[..start], ph, &whole[end..])
                })
                .into_owned();
            for (ph, _) in hits {
                self.hit(&ph, rule.kind, file);
            }
            out = replaced;
        }
        out
    }

    /// Redact every string in a JSON value.
    pub fn value(&mut self, v: &Value, file: &str) -> Value {
        match v {
            Value::String(s) => Value::String(self.text(s, file)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(x, file)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| (k.clone(), self.value(x, file)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    pub fn findings(&self) -> Vec<Finding> {
        self.findings.values().cloned().collect()
    }

    pub fn findings_json(&self) -> Value {
        Value::Array(
            self.findings
                .values()
                .map(|f| {
                    json!({ "placeholder": f.placeholder, "kind": f.kind, "count": f.count,
                            "files": f.files.iter().collect::<Vec<_>>() })
                })
                .collect(),
        )
    }
}

/// Kinds and counts of secrets still present in `text` (for validation).
pub fn scan(text: &str) -> Vec<(&'static str, usize)> {
    let mut out: BTreeMap<&'static str, usize> = BTreeMap::new();
    for rule in rules() {
        for c in rule.re.captures_iter(text) {
            if let Some(m) = c.get(rule.group) {
                if !m.as_str().starts_with("[REDACTED:") {
                    *out.entry(rule.kind).or_default() += 1;
                }
            }
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_secret_same_placeholder_and_only_the_secret_is_replaced() {
        let mut r = Redactor::default();
        let a = r.text(
            "db postgres://app:hunter22@db:5432/x and again postgres://app:hunter22@db/y",
            "t",
        );
        assert_eq!(
            a,
            "db postgres://app:[REDACTED:password:1]@db:5432/x and again postgres://app:[REDACTED:password:1]@db/y"
        );
        let b = r.text("other postgres://u:s3cr3t99@h/z", "u");
        assert!(b.contains("[REDACTED:password:2]"));
        let f = r.findings();
        assert_eq!(f[0].count, 2);
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn recognizes_common_shapes() {
        let mut r = Redactor::default();
        let s = r.text(
            "AKIAABCDEFGHIJKLMNOP ghp_abcdefghijklmnopqrstuvwxyz0123456789 \
             -----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY----- \
             Authorization: Bearer abcdefghijklmnopqrstuvwxyz12 \
             STRIPE_SECRET_KEY=abcdefgh12345 xoxb-1234567890-abc",
            "t",
        );
        assert!(!s.contains("AKIA"), "{s}");
        assert!(!s.contains("ghp_"), "{s}");
        assert!(!s.contains("abc\n-----END"), "{s}");
        assert!(s.contains("Bearer [REDACTED:token:"), "{s}");
        assert!(s.contains("STRIPE_SECRET_KEY=[REDACTED:password:"), "{s}");
        assert!(scan(&s).is_empty(), "{:?}", scan(&s));
    }

    #[test]
    fn leaves_ordinary_code_alone() {
        let mut r = Redactor::default();
        let code = "let token: String = get(); password.len() > 8; https://example.com/a:b@c";
        let out = r.text(code, "t");
        assert!(
            r.findings().is_empty() || !out.contains("let token: [REDACTED"),
            "{out}"
        );
        assert!(out.starts_with("let token: String"));
    }
}
