//! Field / variant rename rules.

/// Apply a rename strategy to an identifier.
pub fn apply(rule: &str, name: &str) -> String {
    let words = split_words(name);
    match rule {
        "lowercase" => words.join("").to_lowercase(),
        "UPPERCASE" => words.join("").to_uppercase(),
        "PascalCase" => words.iter().map(|w| capitalize(w)).collect::<String>(),
        "camelCase" => {
            let mut out = String::new();
            for (i, w) in words.iter().enumerate() {
                if i == 0 {
                    out.push_str(&w.to_lowercase());
                } else {
                    out.push_str(&capitalize(w));
                }
            }
            out
        }
        "snake_case" => words.join("_").to_lowercase(),
        "SCREAMING_SNAKE_CASE" => words.join("_").to_uppercase(),
        "kebab-case" => words.join("-").to_lowercase(),
        "SCREAMING-KEBAB-CASE" => words.join("-").to_uppercase(),
        _ => name.to_string(),
    }
}

fn capitalize(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Split a mixed-style identifier into words.
fn split_words(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '_' || c == '-' || c == ' ' {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            i += 1;
            continue;
        }
        if c.is_uppercase() {
            let prev = if i > 0 { Some(chars[i - 1]) } else { None };
            let next = chars.get(i + 1).copied();
            let boundary = match (prev, next) {
                (Some(p), _) if p.is_lowercase() => true,
                (_, Some(n)) if n.is_lowercase() && !cur.is_empty() => true,
                _ => false,
            };
            if boundary && !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            cur.push(c);
        } else {
            cur.push(c);
        }
        i += 1;
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        assert_eq!(apply("snake_case", "helloWorld"), "hello_world");
        assert_eq!(apply("camelCase", "hello_world"), "helloWorld");
        assert_eq!(apply("PascalCase", "hello_world"), "HelloWorld");
        assert_eq!(apply("SCREAMING_SNAKE_CASE", "helloWorld"), "HELLO_WORLD");
        assert_eq!(apply("kebab-case", "helloWorld"), "hello-world");
        assert_eq!(apply("SCREAMING-KEBAB-CASE", "hello_world"), "HELLO-WORLD");
        assert_eq!(apply("lowercase", "HelloWorld"), "helloworld");
        assert_eq!(apply("UPPERCASE", "hello_world"), "HELLOWORLD");
        assert_eq!(apply("snake_case", "XMLHttpRequest"), "xml_http_request");
        assert_eq!(apply("camelCase", "XMLHttpRequest"), "xmlHttpRequest");
    }
}
