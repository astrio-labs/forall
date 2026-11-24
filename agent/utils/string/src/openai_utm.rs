//! Strip OpenAI web-search tracking params (`utm_source=openai`) from URLs.

/// Remove `utm_source=openai` from a single URL's query string.
///
/// Leaves the URL unchanged when the param is absent. Drops a trailing `?`
/// when it was the only query parameter.
pub fn strip_openai_utm_params(url: &str) -> String {
    let Some((before_query, after_q)) = url.split_once('?') else {
        return url.to_string();
    };
    let (query, fragment) = match after_q.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (after_q, None),
    };

    let mut kept: Vec<&str> = Vec::new();
    let mut removed = false;
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key.eq_ignore_ascii_case("utm_source") && value.eq_ignore_ascii_case("openai") {
            removed = true;
            continue;
        }
        kept.push(pair);
    }

    if !removed {
        return url.to_string();
    }

    let mut out = before_query.to_string();
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

/// Rewrite every `http(s)://…` URL in `text`, stripping OpenAI UTM params.
pub fn strip_openai_utm_params_in_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(rel) = find_url_start(rest) {
        out.push_str(&rest[..rel]);
        rest = &rest[rel..];
        let end = url_end_offset(rest);
        let (url, trailer) = split_trailing_url_punctuation(&rest[..end]);
        out.push_str(&strip_openai_utm_params(url));
        out.push_str(trailer);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn find_url_start(text: &str) -> Option<usize> {
    let http = text.find("http://");
    let https = text.find("https://");
    match (http, https) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn url_end_offset(text: &str) -> usize {
    text.char_indices()
        .find(|(_, ch)| ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'' | ')'))
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

fn split_trailing_url_punctuation(url: &str) -> (&str, &str) {
    let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    let trailer_start = trimmed.len();
    (&url[..trailer_start], &url[trailer_start..])
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::strip_openai_utm_params;
    use super::strip_openai_utm_params_in_text;

    #[test]
    fn strips_sole_utm_source_openai() {
        assert_eq!(
            strip_openai_utm_params("https://www.astrio.app/?utm_source=openai"),
            "https://www.astrio.app/"
        );
    }

    #[test]
    fn strips_utm_source_openai_keeping_other_params() {
        assert_eq!(
            strip_openai_utm_params(
                "https://docs.astrio.app/welcome?foo=1&utm_source=openai&bar=2"
            ),
            "https://docs.astrio.app/welcome?foo=1&bar=2"
        );
    }

    #[test]
    fn preserves_fragment_when_stripping() {
        assert_eq!(
            strip_openai_utm_params("https://example.com/path?utm_source=openai#section"),
            "https://example.com/path#section"
        );
    }

    #[test]
    fn leaves_unrelated_utm_source_alone() {
        assert_eq!(
            strip_openai_utm_params("https://example.com/?utm_source=newsletter"),
            "https://example.com/?utm_source=newsletter"
        );
    }

    #[test]
    fn rewrites_urls_embedded_in_markdownish_text() {
        let input = "See (astrio.app (https://www.astrio.app/?utm_source=openai)) and https://docs.astrio.app/introduction/welcome?utm_source=openai.";
        let expected = "See (astrio.app (https://www.astrio.app/)) and https://docs.astrio.app/introduction/welcome.";
        assert_eq!(strip_openai_utm_params_in_text(input), expected);
    }

    #[test]
    fn is_case_insensitive_for_openai_utm() {
        assert_eq!(
            strip_openai_utm_params("https://example.com/?UTM_SOURCE=OpenAI"),
            "https://example.com/"
        );
    }
}
