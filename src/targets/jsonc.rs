use std::{collections::BTreeMap, path::Path};

use serde_json::{Map, Value};

use crate::diagnostics::{McpdError, Result};

#[derive(Debug)]
struct Property {
    key: String,
    key_start: usize,
    value_start: usize,
    value_end: usize,
    comma_after: Option<usize>,
}

#[derive(Debug)]
struct Edit {
    start: usize,
    end: usize,
    replacement: String,
}

pub fn parse(bytes: &[u8], label: &str, path: &Path) -> Result<(String, Value)> {
    let source = std::str::from_utf8(bytes).map_err(|_| McpdError::InvalidInput {
        message: format!("{label} configuration {} is not UTF-8", path.display()),
        hint: "repair the file; mcpd never overwrites non-UTF-8 target configuration".into(),
    })?;
    let sanitized = sanitize(source).map_err(|message| McpdError::InvalidInput {
        message: format!(
            "{label} configuration {} is malformed JSONC: {message}",
            path.display()
        ),
        hint: "repair the file; mcpd never overwrites malformed target configuration".into(),
    })?;
    let value = serde_json::from_str(&sanitized).map_err(|error| McpdError::InvalidInput {
        message: format!(
            "{label} configuration {} is malformed JSONC: {error}",
            path.display()
        ),
        hint: "repair the file; mcpd never overwrites malformed target configuration".into(),
    })?;
    Ok((source.to_owned(), value))
}

pub fn patch_servers(
    source: &str,
    servers_path: &[String],
    final_servers: &Map<String, Value>,
    changed: &[String],
    removed: &[String],
) -> Result<Vec<u8>> {
    let sanitized = sanitize(source).map_err(internal_jsonc_error)?;
    let root = value_range(&sanitized, 0)?;
    let (object_range, missing_at) = locate_path(&sanitized, root, servers_path)?;
    let mut edits = Vec::new();

    if let Some(index) = missing_at {
        let suffix = &servers_path[index..];
        let mut value = Value::Object(final_servers.clone());
        for key in suffix.iter().skip(1).rev() {
            value = Value::Object(Map::from_iter([(key.clone(), value)]));
        }
        insert_property(
            source,
            &sanitized,
            object_range,
            &suffix[0],
            &value,
            &mut edits,
        )?;
    } else {
        let properties = object_properties(&sanitized, object_range)?;
        let by_name = properties
            .iter()
            .enumerate()
            .map(|(index, property)| (property.key.as_str(), index))
            .collect::<BTreeMap<_, _>>();

        let mut additions = Vec::new();
        for name in changed {
            let value = final_servers
                .get(name)
                .ok_or_else(|| McpdError::Operational {
                    message: format!("rendered JSONC server `{name}` disappeared"),
                    hint: "report this as an mcpd bug".into(),
                })?;
            let rendered =
                serde_json::to_string(value).map_err(|error| McpdError::Operational {
                    message: format!("could not serialize JSONC server `{name}`: {error}"),
                    hint: "report this as an mcpd bug".into(),
                })?;
            if let Some(index) = by_name.get(name.as_str()) {
                let property = &properties[*index];
                edits.push(Edit {
                    start: property.value_start,
                    end: property.value_end,
                    replacement: rendered,
                });
            } else {
                additions.push((name.as_str(), value));
            }
        }
        if !additions.is_empty() {
            insert_properties(source, &sanitized, object_range, &additions, &mut edits)?;
        }

        for name in removed {
            if let Some(index) = by_name.get(name.as_str()) {
                let property = &properties[*index];
                edits.push(Edit {
                    start: property.key_start,
                    end: property.value_end,
                    replacement: String::new(),
                });
                if let Some(comma) = property.comma_after {
                    edits.push(Edit {
                        start: comma,
                        end: comma + 1,
                        replacement: String::new(),
                    });
                } else if *index > 0
                    && let Some(comma) = properties[*index - 1].comma_after
                {
                    edits.push(Edit {
                        start: comma,
                        end: comma + 1,
                        replacement: String::new(),
                    });
                }
            }
        }
    }

    edits.sort_by_key(|edit| (std::cmp::Reverse(edit.start), std::cmp::Reverse(edit.end)));
    edits.dedup_by(|left, right| {
        left.start == right.start && left.end == right.end && left.replacement == right.replacement
    });
    let mut rendered = source.to_owned();
    let mut last_start = source.len();
    for edit in edits {
        if edit.end > last_start || edit.start > edit.end || edit.end > rendered.len() {
            return Err(internal_jsonc_error("overlapping JSONC edits".into()));
        }
        rendered.replace_range(edit.start..edit.end, &edit.replacement);
        last_start = edit.start;
    }
    // Reparse before any filesystem write. This also catches edit bugs and duplicate keys.
    let reparsed = sanitize(&rendered).map_err(internal_jsonc_error)?;
    serde_json::from_str::<Value>(&reparsed).map_err(|error| McpdError::Operational {
        message: format!("rendered JSONC did not validate: {error}"),
        hint: "report this as an mcpd bug; no target file was modified".into(),
    })?;
    Ok(rendered.into_bytes())
}

fn locate_path(
    source: &str,
    mut object: (usize, usize),
    path: &[String],
) -> Result<((usize, usize), Option<usize>)> {
    for (index, key) in path.iter().enumerate() {
        let properties = object_properties(source, object)?;
        let Some(property) = properties.iter().find(|property| &property.key == key) else {
            return Ok((object, Some(index)));
        };
        if source.as_bytes().get(property.value_start) != Some(&b'{') {
            return Err(McpdError::InvalidInput {
                message: format!("JSONC path component `{key}` must be an object"),
                hint: "repair the native MCP server collection before syncing".into(),
            });
        }
        object = (property.value_start, property.value_end);
    }
    Ok((object, None))
}

fn insert_property(
    source: &str,
    sanitized: &str,
    object: (usize, usize),
    key: &str,
    value: &Value,
    edits: &mut Vec<Edit>,
) -> Result<()> {
    insert_properties(source, sanitized, object, &[(key, value)], edits)
}

fn insert_properties(
    source: &str,
    sanitized: &str,
    object: (usize, usize),
    values: &[(&str, &Value)],
    edits: &mut Vec<Edit>,
) -> Result<()> {
    let properties = object_properties(sanitized, object)?;
    let close = object
        .1
        .checked_sub(1)
        .ok_or_else(|| internal_jsonc_error("empty object range".into()))?;
    let closing_indent = line_indent(source, close);
    let child_indent = format!("{closing_indent}  ");
    let rendered_values = values
        .iter()
        .map(|(key, value)| {
            let key = serde_json::to_string(key)
                .map_err(|error| internal_jsonc_error(error.to_string()))?;
            let value = serde_json::to_string(value)
                .map_err(|error| internal_jsonc_error(error.to_string()))?;
            Ok(format!("{child_indent}{key}: {value}"))
        })
        .collect::<Result<Vec<_>>>()?
        .join(",\n");
    let trailing_comma = properties
        .last()
        .is_some_and(|property| source.as_bytes()[property.value_end..close].contains(&b','));
    let comma = if !properties.is_empty() && !trailing_comma {
        ","
    } else {
        ""
    };
    let insertion = if properties.is_empty() {
        format!("\n{rendered_values}\n{closing_indent}")
    } else {
        format!("{comma}\n{rendered_values}\n{closing_indent}")
    };
    edits.push(Edit {
        start: close,
        end: close,
        replacement: insertion,
    });
    Ok(())
}

fn object_properties(source: &str, object: (usize, usize)) -> Result<Vec<Property>> {
    let bytes = source.as_bytes();
    if bytes.get(object.0) != Some(&b'{') || bytes.get(object.1.saturating_sub(1)) != Some(&b'}') {
        return Err(internal_jsonc_error("expected an object range".into()));
    }
    let mut cursor = object.0 + 1;
    let end = object.1 - 1;
    let mut result = Vec::new();
    let mut seen = BTreeMap::new();
    while {
        cursor = skip_ws(bytes, cursor, end);
        cursor < end
    } {
        if bytes[cursor] == b',' {
            cursor += 1;
            continue;
        }
        let key_start = cursor;
        let key_end = string_end(bytes, cursor, end)?;
        let key: String = serde_json::from_str(&source[key_start..key_end]).map_err(|error| {
            McpdError::InvalidInput {
                message: format!("invalid JSONC object key: {error}"),
                hint: "repair duplicate or malformed keys before syncing".into(),
            }
        })?;
        if seen.insert(key.clone(), ()).is_some() {
            return Err(McpdError::InvalidInput {
                message: format!("JSONC object contains duplicate key `{key}`"),
                hint: "remove the duplicate key; mcpd will not guess which value to preserve"
                    .into(),
            });
        }
        cursor = skip_ws(bytes, key_end, end);
        if bytes.get(cursor) != Some(&b':') {
            return Err(internal_jsonc_error(format!("missing colon after `{key}`")));
        }
        cursor = skip_ws(bytes, cursor + 1, end);
        let value_start = cursor;
        let value_end = value_range(source, value_start)?.1;
        if value_end > end {
            return Err(internal_jsonc_error(format!(
                "value for `{key}` exceeds its object"
            )));
        }
        cursor = skip_ws(bytes, value_end, end);
        let comma_after = (bytes.get(cursor) == Some(&b',')).then_some(cursor);
        if comma_after.is_some() {
            cursor += 1;
        }
        result.push(Property {
            key,
            key_start,
            value_start,
            value_end,
            comma_after,
        });
    }
    Ok(result)
}

fn value_range(source: &str, start: usize) -> Result<(usize, usize)> {
    let bytes = source.as_bytes();
    let start = skip_ws(bytes, start, bytes.len());
    let Some(first) = bytes.get(start).copied() else {
        return Err(internal_jsonc_error("missing JSONC value".into()));
    };
    let end = match first {
        b'"' => string_end(bytes, start, bytes.len())?,
        b'{' | b'[' => {
            let (open, close) = if first == b'{' {
                (b'{', b'}')
            } else {
                (b'[', b']')
            };
            let mut depth = 0_u32;
            let mut cursor = start;
            loop {
                let byte = *bytes
                    .get(cursor)
                    .ok_or_else(|| internal_jsonc_error("unterminated JSONC container".into()))?;
                if byte == b'"' {
                    cursor = string_end(bytes, cursor, bytes.len())?;
                    continue;
                }
                if byte == open {
                    depth += 1;
                } else if byte == close {
                    depth -= 1;
                    if depth == 0 {
                        break cursor + 1;
                    }
                }
                cursor += 1;
            }
        }
        _ => {
            let mut cursor = start;
            while cursor < bytes.len() && !matches!(bytes[cursor], b',' | b'}' | b']') {
                cursor += 1;
            }
            let mut end = cursor;
            while end > start && bytes[end - 1].is_ascii_whitespace() {
                end -= 1;
            }
            end
        }
    };
    Ok((start, end))
}

fn string_end(bytes: &[u8], start: usize, end: usize) -> Result<usize> {
    if bytes.get(start) != Some(&b'"') {
        return Err(internal_jsonc_error("expected a quoted key".into()));
    }
    let mut cursor = start + 1;
    let mut escaped = false;
    while cursor < end {
        let byte = bytes[cursor];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return Ok(cursor + 1);
        }
        cursor += 1;
    }
    Err(internal_jsonc_error("unterminated JSON string".into()))
}

fn skip_ws(bytes: &[u8], mut cursor: usize, end: usize) -> usize {
    while cursor < end && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

fn line_indent(source: &str, at: usize) -> String {
    let line = &source[..at];
    let start = line.rfind('\n').map_or(0, |index| index + 1);
    line[start..]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect()
}

fn sanitize(source: &str) -> std::result::Result<String, String> {
    let bytes = source.as_bytes();
    let mut output = bytes.to_vec();
    let mut cursor = 0;
    let mut in_string = false;
    let mut escaped = false;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            cursor += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            cursor += 1;
        } else if byte == b'/' && bytes.get(cursor + 1) == Some(&b'/') {
            output[cursor] = b' ';
            output[cursor + 1] = b' ';
            cursor += 2;
            while cursor < bytes.len() && bytes[cursor] != b'\n' {
                output[cursor] = b' ';
                cursor += 1;
            }
        } else if byte == b'/' && bytes.get(cursor + 1) == Some(&b'*') {
            output[cursor] = b' ';
            output[cursor + 1] = b' ';
            cursor += 2;
            let mut closed = false;
            while cursor < bytes.len() {
                if bytes[cursor] == b'*' && bytes.get(cursor + 1) == Some(&b'/') {
                    output[cursor] = b' ';
                    output[cursor + 1] = b' ';
                    cursor += 2;
                    closed = true;
                    break;
                }
                if bytes[cursor] != b'\n' && bytes[cursor] != b'\r' {
                    output[cursor] = b' ';
                }
                cursor += 1;
            }
            if !closed {
                return Err("unterminated block comment".into());
            }
        } else {
            cursor += 1;
        }
    }
    if in_string {
        return Err("unterminated string".into());
    }
    for index in 0..output.len() {
        if output[index] != b',' {
            continue;
        }
        let next = skip_ws(&output, index + 1, output.len());
        if matches!(output.get(next), Some(b'}' | b']')) {
            output[index] = b' ';
        }
    }
    String::from_utf8(output).map_err(|_| "configuration is not UTF-8".into())
}

fn internal_jsonc_error(message: String) -> McpdError {
    McpdError::Operational {
        message: format!("could not safely edit JSONC: {message}"),
        hint: "report this as an mcpd bug; no target file was modified".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_only_selected_jsonc_properties() {
        let source = r#"{
  // outer comment
  "mcpServers": {
    // unmanaged comment
    "manual": { "command": "manual", },
    "owned": { "command": "old" }
  },
}"#;
        let (_, parsed) = parse(source.as_bytes(), "test", Path::new("test.jsonc")).unwrap();
        assert_eq!(parsed["mcpServers"]["manual"]["command"], "manual");
        let final_servers = Map::from_iter([
            ("manual".into(), parsed["mcpServers"]["manual"].clone()),
            ("owned".into(), serde_json::json!({"command":"new"})),
            (
                "added".into(),
                serde_json::json!({"url":"https://example.test/mcp"}),
            ),
        ]);
        let rendered = patch_servers(
            source,
            &["mcpServers".into()],
            &final_servers,
            &["owned".into(), "added".into()],
            &[],
        )
        .unwrap();
        let rendered = String::from_utf8(rendered).unwrap();
        assert!(rendered.contains("// outer comment"));
        assert!(rendered.contains("// unmanaged comment"));
        assert!(rendered.contains(r#""owned": {"command":"new"}"#));
        assert!(rendered.contains(r#""added": {"url":"https://example.test/mcp"}"#));
    }

    #[test]
    fn removes_adjacent_properties_without_overlapping_edits() {
        let source = r#"{
  "mcpServers": {
    "owned-one": { "command": "one" },
    "owned-two": { "command": "two" },
    // this entry is unmanaged
    "manual": { "command": "manual", },
  },
}"#;
        let (_, parsed) = parse(source.as_bytes(), "test", Path::new("test.jsonc")).unwrap();
        let final_servers =
            Map::from_iter([("manual".into(), parsed["mcpServers"]["manual"].clone())]);
        let rendered = patch_servers(
            source,
            &["mcpServers".into()],
            &final_servers,
            &[],
            &["owned-one".into(), "owned-two".into()],
        )
        .unwrap();
        let rendered = String::from_utf8(rendered).unwrap();
        assert!(!rendered.contains("owned-one"));
        assert!(!rendered.contains("owned-two"));
        assert!(rendered.contains("// this entry is unmanaged"));
        assert!(rendered.contains(r#""manual": { "command": "manual", }"#));
        let (_, parsed) = parse(rendered.as_bytes(), "test", Path::new("test.jsonc")).unwrap();
        assert_eq!(parsed["mcpServers"]["manual"]["command"], "manual");
    }
}
