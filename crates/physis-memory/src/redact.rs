// SPDX-License-Identifier: Apache-2.0
// Extracted from physis-next@64ae52fe4, physis-store/redact.rs.
//! Shape-based redaction at the append boundary; not a universal secret detector.

use crate::Observation;
pub const MARKER: &str = "<redacted:credential-shaped>";
const PREFIXES: [&str; 6] = ["sk-", "ghp_", "gho_", "AKIA", "xoxb-", "AIza"];
const NAMES: [&str; 4] = ["_KEY", "_TOKEN", "_SECRET", "_PASSWORD"];
const MIN_VALUE: usize = 8;

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn scrub_prefixed(text: &str, out: &mut String, hits: &mut usize) {
    let mut rest = text;
    while !rest.is_empty() {
        let Some((at, prefix)) = PREFIXES
            .iter()
            .filter_map(|p| rest.find(p).map(|i| (i, *p)))
            .min_by_key(|(i, _)| *i)
        else {
            out.push_str(rest);
            return;
        };
        let after = at + prefix.len();
        let run = rest[after..]
            .char_indices()
            .take_while(|(_, c)| token_char(*c))
            .map(|(i, c)| i + c.len_utf8())
            .last()
            .unwrap_or(0);
        out.push_str(&rest[..at]);
        if prefix.len() + run >= 20 {
            out.push_str(MARKER);
            *hits += 1;
        } else {
            out.push_str(&rest[at..after + run]);
        }
        rest = &rest[after + run..];
    }
}

fn token_end(s: &str) -> usize {
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c.is_whitespace() {
                    return i;
                }
                if c == '"' || c == '\'' {
                    quote = Some(c);
                }
            }
        }
    }
    s.len()
}

fn scrub_assignments(line: &str, hits: &mut usize) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while !rest.is_empty() {
        let end = token_end(rest);
        let token = &rest[..end];
        match token.split_once('=') {
            Some((name, value))
                if NAMES.iter().any(|n| name.ends_with(n))
                    && value.trim() != MARKER
                    && value
                        .trim_matches(|c| c == '"' || c == '\'')
                        .chars()
                        .count()
                        >= MIN_VALUE =>
            {
                out.push_str(name);
                out.push('=');
                out.push_str(MARKER);
                *hits += 1;
            }
            _ => out.push_str(token),
        }
        rest = &rest[end..];
        let ws = rest
            .char_indices()
            .take_while(|(_, c)| c.is_whitespace())
            .map(|(i, c)| i + c.len_utf8())
            .last()
            .unwrap_or(0);
        out.push_str(&rest[..ws]);
        rest = &rest[ws..];
    }
    out
}

pub fn scrub_text(text: &str, hits: &mut usize) -> String {
    let mut prefixed = String::with_capacity(text.len());
    scrub_prefixed(text, &mut prefixed, hits);
    scrub_assignments(&prefixed, hits)
}

pub fn scrub(obs: &mut Observation) -> usize {
    let mut hits = 0;
    obs.body = scrub_text(&obs.body, &mut hits);
    obs.subject = scrub_text(&obs.subject, &mut hits);
    if let Some(by) = obs.produced_by.as_mut() {
        *by = scrub_text(by, &mut hits);
    }
    for value in obs.props.values_mut() {
        *value = scrub_text(value, &mut hits);
    }
    hits
}
