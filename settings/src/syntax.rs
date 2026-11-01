// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Quoted strings shared by INI entries, list fields and the settings editor.
//! Unquoted values retain their spelling, including Windows paths.

pub fn quote(value: &str) -> String {
    if !value.is_empty()
        && value.trim() == value
        && !value
            .chars()
            .any(|c| c.is_control() || ",=;#[]\"".contains(c))
    {
        return value.to_owned();
    }
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn unquote(value: &str) -> String {
    let value = value.trim();
    let Some(inside) = value.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return value.to_owned();
    };
    let mut out = String::new();
    let mut chars = inside.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(c @ ('"' | '\\')) => out.push(c),
            Some(c) => {
                out.push('\\');
                out.push(c);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Retain quoting until delimiters and options have been interpreted.
pub fn split(text: &str, delimiter: char) -> impl Iterator<Item = &str> {
    let mut quoted = false;
    let mut escaped = false;
    text.split(move |c| {
        if escaped {
            escaped = false;
            return false;
        }
        if quoted && c == '\\' {
            escaped = true;
            return false;
        }
        if c == '"' {
            quoted = !quoted;
        }
        c == delimiter && !quoted
    })
    .map(str::trim)
}

pub fn assignment(text: &str) -> Option<(&str, &str)> {
    let text = text.trim();
    let name = split(text, '=').next()?;
    let value = text.get(name.len()..)?.trim_start().strip_prefix('=')?;
    Some((name, value.trim()))
}

pub fn list(values: &[String]) -> String {
    values
        .iter()
        .map(|s| quote(s))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strings_survive_settings_syntax() {
        for value in [
            "",
            "simple",
            "a,b=c ; # [x]",
            "a\"b\\c",
            "a\nb\r\t",
            " padded ",
            r"C:\path\file",
        ] {
            assert_eq!(unquote(&quote(value)), value);
            let written = format!("{} = {}", quote(value), quote(value));
            let (key, val) = assignment(&written).unwrap();
            assert_eq!(unquote(key), value);
            assert_eq!(unquote(val), value);
            let values = vec![value.to_owned(), "second".to_owned()];
            assert_eq!(
                split(&list(&values), ',').map(unquote).collect::<Vec<_>>(),
                values
            );
        }
    }
}
