use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn read_document_text(path: &Path) -> Result<String, String> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());

    if matches!(extension.as_deref(), Some("rtf")) {
        if let Ok(text) = convert_rtf_with_textutil(path) {
            return Ok(text);
        }
    }

    fs::read_to_string(path)
        .map(|raw| decode_document_text(&raw))
        .map_err(|err| format!("Could not open file: {err}"))
}

fn decode_document_text(raw: &str) -> String {
    if raw.trim_start().starts_with("{\\rtf") {
        strip_basic_rtf(raw)
    } else {
        raw.to_owned()
    }
}

fn convert_rtf_with_textutil(path: &Path) -> Result<String, String> {
    let output = Command::new("textutil")
        .args(["-convert", "txt", "-stdout"])
        .arg(path)
        .output()
        .map_err(|err| format!("textutil failed: {err}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }

    let text = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    Ok(text.trim_end().to_owned())
}

fn strip_basic_rtf(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    let mut skip_stack = vec![false];
    let mut skip_next_group = false;
    let destinations: HashSet<&'static str> = [
        "fonttbl",
        "colortbl",
        "stylesheet",
        "info",
        "pict",
        "expandedcolortbl",
        "generator",
    ]
    .into_iter()
    .collect();

    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                let parent_skip = *skip_stack.last().unwrap_or(&false);
                skip_stack.push(parent_skip || skip_next_group);
                skip_next_group = false;
            }
            '}' => {
                skip_stack.pop();
                if skip_stack.is_empty() {
                    skip_stack.push(false);
                }
            }
            '\\' => match chars.peek().copied() {
                Some('\\') | Some('{') | Some('}') => {
                    let escaped = chars.next().unwrap_or_default();
                    if !*skip_stack.last().unwrap_or(&false) {
                        out.push(escaped);
                    }
                }
                Some('\'') => {
                    chars.next();
                    let a = chars.next();
                    let b = chars.next();
                    if let (Some(a), Some(b)) = (a, b) {
                        if !*skip_stack.last().unwrap_or(&false) {
                            if let Ok(byte) = u8::from_str_radix(&format!("{a}{b}"), 16) {
                                out.push(byte as char);
                            }
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    skip_next_group = true;
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    let mut word = String::new();
                    while let Some(next) = chars.peek().copied() {
                        if next.is_ascii_alphabetic() {
                            word.push(next);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    while let Some(next) = chars.peek().copied() {
                        if next == '-' || next.is_ascii_digit() {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    if chars.peek() == Some(&' ') {
                        chars.next();
                    }

                    if destinations.contains(word.as_str()) {
                        if let Some(current) = skip_stack.last_mut() {
                            *current = true;
                        }
                        continue;
                    }

                    if *skip_stack.last().unwrap_or(&false) {
                        continue;
                    }

                    if word == "par" || word == "line" {
                        out.push('\n');
                    } else if word == "tab" {
                        out.push('\t');
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\r' => {}
            _ => {
                if !*skip_stack.last().unwrap_or(&false) {
                    out.push(ch);
                }
            }
        }
    }

    out.lines()
        .map(str::trim_end)
        .filter(|line| !looks_like_rtf_junk(line))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

fn looks_like_rtf_junk(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return true;
    }
    if lower.chars().all(|c| ";:".contains(c)) {
        return true;
    }
    lower == "helvetica" || lower == "times" || lower == "courier"
}
