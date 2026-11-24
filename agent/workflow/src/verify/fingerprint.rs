//! Annotation-aware per-symbol fingerprints — the staleness engine's unit of
//! truth.
//!
//! A proof holds for a symbol's SEMANTICS, so its fingerprint must survive
//! reformatting and prose-comment edits but change the moment the code or
//! its formal annotations change. The fingerprint is a SHA-256 over the
//! symbol's tree-sitter leaf tokens with whitespace gone, plain comments
//! dropped, and annotation comments (`//@` lines, `/*@ ... */` blocks — the
//! ACSL/JML/Forall-TS carriers) kept as semantic content. Annotations
//! ABOVE a declaration (the Java/C convention) are attached to the symbol
//! they precede; annotations inside the body (the Forall TS convention) are
//! leaf nodes of the subtree and are kept by the same rule.
//!
//! Symbol resolution is name-based, matching the mapping's `CodeRef.symbols`
//! today: the first definition with a matching name wins. Overload-precise
//! selectors (Java signatures) arrive with symbol-level ledger anchors.

use sha2::{Digest, Sha256};
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FingerprintLanguage {
    TypeScript,
    Rust,
    Java,
    C,
}

impl FingerprintLanguage {
    pub fn for_path(path: &str) -> Option<Self> {
        let extension = std::path::Path::new(path).extension()?.to_str()?;
        match extension {
            "ts" | "tsx" => Some(Self::TypeScript),
            "rs" => Some(Self::Rust),
            "java" => Some(Self::Java),
            "c" | "h" => Some(Self::C),
            _ => None,
        }
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::TypeScript => "ts",
            Self::Rust => "rs",
            Self::Java => "java",
            Self::C => "c",
        }
    }

    /// Node kinds that define a nameable symbol.
    fn definition_kinds(self) -> &'static [&'static str] {
        match self {
            Self::TypeScript => &["function_declaration", "method_definition"],
            Self::Rust => &["function_item"],
            Self::Java => &["method_declaration", "constructor_declaration"],
            Self::C => &["function_definition"],
        }
    }

    /// The defined name of a definition node, when it has one.
    fn name_of<'s>(self, node: Node<'_>, source: &'s str) -> Option<&'s str> {
        match self {
            Self::TypeScript | Self::Rust | Self::Java => node
                .child_by_field_name("name")
                .and_then(|name| name.utf8_text(source.as_bytes()).ok()),
            // C buries the identifier under pointer/function declarators:
            // `uint8_t *f(void)` is declarator(pointer_declarator(
            // function_declarator(identifier))).
            Self::C => {
                let mut declarator = node.child_by_field_name("declarator")?;
                loop {
                    if declarator.kind() == "identifier" {
                        return declarator.utf8_text(source.as_bytes()).ok();
                    }
                    declarator = declarator.child_by_field_name("declarator")?;
                }
            }
        }
    }
}

fn is_comment(node: Node<'_>) -> bool {
    node.kind().contains("comment")
}

/// Annotation comments are semantic content; everything else in a comment is
/// prose. `//@` carries JML and the Forall TS dialect, `/*@` carries ACSL.
fn is_annotation_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("//@") || trimmed.starts_with("/*@")
}

/// Canonical form of an annotation comment. The syntax carriers (`//@`,
/// `/*@ ... */`, the `@` continuation gutter on ACSL block lines) and
/// whitespace runs are formatting; the specification words are the
/// semantics. Re-indenting or re-wrapping an annotation must not move a
/// fingerprint, or formatting edits masquerade as proof staleness.
fn normalize_annotation(text: &str) -> String {
    let trimmed = text.trim();
    let body = if let Some(block) = trimmed.strip_prefix("/*@") {
        block.strip_suffix("*/").unwrap_or(block)
    } else if let Some(line) = trimmed.strip_prefix("//@") {
        line
    } else {
        trimmed
    };
    // Whitespace INSIDE string and character literals is specification
    // content, not formatting: `"a b"` and `"a  b"` are different claims.
    // Each line is scanned once, literals kept verbatim (escapes honored),
    // and only the text between literals goes through word-splitting.
    fn flush_words(tokens: &mut Vec<String>, chunk: &str) {
        tokens.extend(
            chunk
                .split_whitespace()
                // A standalone `@` is always gutter decoration (`@*/`
                // closers, mid-line gutters), never specification content.
                .filter(|word| *word != "@")
                .map(str::to_string),
        );
    }
    let mut tokens: Vec<String> = Vec::new();
    for line in body.lines() {
        let line = line.trim_start();
        let line = line.strip_prefix('@').unwrap_or(line);
        let mut plain = String::new();
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            if c == '"' || c == '\'' {
                flush_words(&mut tokens, &plain);
                plain.clear();
                let mut literal = String::new();
                literal.push(c);
                let mut escaped = false;
                for lc in chars.by_ref() {
                    literal.push(lc);
                    if escaped {
                        escaped = false;
                    } else if lc == '\\' {
                        escaped = true;
                    } else if lc == c {
                        break;
                    }
                }
                tokens.push(literal);
                continue;
            }
            plain.push(c);
        }
        flush_words(&mut tokens, &plain);
    }
    format!("@{}", tokens.join(" "))
}

/// One unit of fingerprint input. The distinction is structural, not
/// textual: Java's `@Override` produces a bare `@` code leaf, so a prefix
/// convention could not tell code from annotations.
enum Token {
    Code(String),
    Annotation(String),
}

impl Token {
    fn text(&self) -> &str {
        match self {
            Token::Code(text) | Token::Annotation(text) => text,
        }
    }
}

/// Depth-first leaf tokens of `node`, with plain comments dropped and
/// annotation comments kept in canonical form.
fn collect_tokens(node: Node<'_>, source: &str, out: &mut Vec<Token>) {
    if node.child_count() == 0 {
        let Ok(text) = node.utf8_text(source.as_bytes()) else {
            return;
        };
        if is_comment(node) {
            if is_annotation_text(text) {
                out.push(Token::Annotation(normalize_annotation(text)));
            }
            return;
        }
        out.push(Token::Code(text.to_string()));
        return;
    }
    // A comment with children does not occur in these grammars, but guard
    // the same way: annotation text is one normalized token.
    if is_comment(node) {
        if let Ok(text) = node.utf8_text(source.as_bytes())
            && is_annotation_text(text)
        {
            out.push(Token::Annotation(normalize_annotation(text)));
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_tokens(child, source, out);
    }
}

/// Annotation comments sitting immediately above the definition (the
/// above-declaration ACSL/JML convention): walk preceding siblings while
/// they are comments, keeping the annotation-shaped ones, in source order.
fn leading_annotations(node: Node<'_>, source: &str) -> Vec<Token> {
    let mut annotations = Vec::new();
    let mut current = node.prev_sibling();
    while let Some(sibling) = current {
        if !is_comment(sibling) {
            break;
        }
        if let Ok(text) = sibling.utf8_text(source.as_bytes())
            && is_annotation_text(text)
        {
            annotations.push(Token::Annotation(normalize_annotation(text)));
        }
        current = sibling.prev_sibling();
    }
    annotations.reverse();
    annotations
}

fn find_definition<'t>(
    node: Node<'t>,
    language: FingerprintLanguage,
    source: &str,
    symbol: &str,
) -> Option<Node<'t>> {
    if language.definition_kinds().contains(&node.kind())
        && language.name_of(node, source) == Some(symbol)
    {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_definition(child, language, source, symbol) {
            return Some(found);
        }
    }
    None
}

/// Both fingerprints of a symbol: `full` covers code plus formal
/// annotations; `code` excludes the annotations. The pair is what lets
/// staleness grading tell a body edit (code drift — the proof may not hold)
/// from an annotation-only edit (spec drift — the proof holds, but of the
/// old claim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolFingerprints {
    pub full: String,
    pub code: String,
}

fn hash_tokens<'t>(
    language: FingerprintLanguage,
    tokens: impl Iterator<Item = &'t Token>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"forall-symbol-fp-v1\0");
    hasher.update(language.tag().as_bytes());
    hasher.update(b"\0");
    for token in tokens {
        hasher.update(token.text().as_bytes());
        hasher.update(b"\0");
    }
    format!("{:x}", hasher.finalize())
}

/// The annotation-aware fingerprints of `symbol` in `source`, or `None` when
/// the symbol cannot be resolved — a missing fingerprint must read as
/// "unknown", never as "unchanged".
pub fn symbol_fingerprints(
    source: &str,
    language: FingerprintLanguage,
    symbol: &str,
) -> Option<SymbolFingerprints> {
    let mut parser = Parser::new();
    parser.set_language(&language.grammar()).ok()?;
    let tree = parser.parse(source, None)?;
    let definition = find_definition(tree.root_node(), language, source, symbol)?;

    let mut tokens = leading_annotations(definition, source);
    collect_tokens(definition, source, &mut tokens);

    // One walk, two hashes: the code hash simply skips annotation tokens.
    let full = hash_tokens(language, tokens.iter());
    let code = hash_tokens(
        language,
        tokens
            .iter()
            .filter(|token| matches!(token, Token::Code(_))),
    );
    Some(SymbolFingerprints { full, code })
}

/// The combined (code + annotations) fingerprint alone.
pub fn symbol_fingerprint(
    source: &str,
    language: FingerprintLanguage,
    symbol: &str,
) -> Option<String> {
    symbol_fingerprints(source, language, symbol).map(|fingerprints| fingerprints.full)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(source: &str, language: FingerprintLanguage, symbol: &str) -> String {
        symbol_fingerprint(source, language, symbol)
            .unwrap_or_else(|| panic!("symbol '{symbol}' not found"))
    }

    #[test]
    fn c_fingerprint_survives_formatting_and_prose_but_not_semantics() {
        let base = "//@ requires len > 0;\nstatic uint16_t crc16(const uint8_t *data, size_t len)\n{\n    return 0;\n}\n";
        let reformatted = "//@ requires len > 0;\nstatic uint16_t crc16(const uint8_t *data,\n                      size_t len) {\n        return 0;\n}\n";
        let prose_comment = "//@ requires len > 0;\n// totally new prose comment\nstatic uint16_t crc16(const uint8_t *data, size_t len)\n{\n    /* inner prose */\n    return 0;\n}\n";
        let annotation_edit = "//@ requires len >= 0;\nstatic uint16_t crc16(const uint8_t *data, size_t len)\n{\n    return 0;\n}\n";
        let body_edit = "//@ requires len > 0;\nstatic uint16_t crc16(const uint8_t *data, size_t len)\n{\n    return 1;\n}\n";

        let lang = FingerprintLanguage::C;
        let baseline = fingerprint(base, lang, "crc16");
        assert_eq!(baseline, fingerprint(reformatted, lang, "crc16"));
        assert_eq!(baseline, fingerprint(prose_comment, lang, "crc16"));
        assert_ne!(baseline, fingerprint(annotation_edit, lang, "crc16"));
        assert_ne!(baseline, fingerprint(body_edit, lang, "crc16"));
    }

    #[test]
    fn c_acsl_block_annotation_above_declaration_is_semantic() {
        let base = "/*@ requires \\valid(p);\n  @ ensures \\result == 0;\n  @*/\nint f(int *p) { return 0; }\n";
        let annotation_edit = "/*@ requires \\valid(p);\n  @ ensures \\result == 1;\n  @*/\nint f(int *p) { return 0; }\n";
        let lang = FingerprintLanguage::C;
        assert_ne!(
            fingerprint(base, lang, "f"),
            fingerprint(annotation_edit, lang, "f")
        );
    }

    #[test]
    fn annotation_reformatting_is_not_staleness() {
        // Re-indented, re-wrapped, gutter-shuffled — same specification.
        let base = "/*@ requires \\valid(p);\n  @ ensures \\result == 0;\n  @*/\nint f(int *p) { return 0; }\n";
        let reindented = "/*@   requires \\valid(p);\n      @   ensures \\result == 0; @*/\nint f(int *p) { return 0; }\n";
        let rewrapped = "/*@ requires\n  @   \\valid(p);\n  @ ensures\n  @   \\result == 0;\n  @*/\nint f(int *p) { return 0; }\n";
        let lang = FingerprintLanguage::C;
        let baseline = fingerprint(base, lang, "f");
        assert_eq!(baseline, fingerprint(reindented, lang, "f"));
        assert_eq!(baseline, fingerprint(rewrapped, lang, "f"));

        // Line-annotation spacing is formatting too.
        let jml = "class M {\n  //@ requires x >= 0;\n  public int clamp(int x) { return x; }\n}\n";
        let jml_spaced =
            "class M {\n  //@     requires   x >= 0;\n  public int clamp(int x) { return x; }\n}\n";
        assert_eq!(
            fingerprint(jml, FingerprintLanguage::Java, "clamp"),
            fingerprint(jml_spaced, FingerprintLanguage::Java, "clamp")
        );
    }

    #[test]
    fn typescript_inside_body_annotations_are_semantic() {
        let base = "export function clamp(x: number): number {\n  //@ requires Number.isFinite(x)\n  return x;\n}\n";
        let annotation_edit =
            "export function clamp(x: number): number {\n  //@ requires x >= 0\n  return x;\n}\n";
        let prose_edit = "export function clamp(x: number): number {\n  //@ requires Number.isFinite(x)\n  // prose only\n  return x;\n}\n";

        let lang = FingerprintLanguage::TypeScript;
        let baseline = fingerprint(base, lang, "clamp");
        assert_ne!(baseline, fingerprint(annotation_edit, lang, "clamp"));
        assert_eq!(baseline, fingerprint(prose_edit, lang, "clamp"));
    }

    #[test]
    fn rust_and_java_resolve_and_discriminate() {
        let rust =
            "pub fn clamp(x: u64, lo: u64, hi: u64) -> u64 {\n    if x < lo { lo } else { x }\n}\n";
        let rust_edit = "pub fn clamp(x: u64, lo: u64, hi: u64) -> u64 {\n    if x <= lo { lo } else { x }\n}\n";
        assert_ne!(
            fingerprint(rust, FingerprintLanguage::Rust, "clamp"),
            fingerprint(rust_edit, FingerprintLanguage::Rust, "clamp")
        );

        let java =
            "class M {\n  //@ requires x >= 0;\n  public int clamp(int x) { return x; }\n}\n";
        let java_reformat = "class M {\n  //@ requires x >= 0;\n  public int clamp(int x) {\n      return x;\n  }\n}\n";
        let java_annotation =
            "class M {\n  //@ requires x > 0;\n  public int clamp(int x) { return x; }\n}\n";
        let lang = FingerprintLanguage::Java;
        let baseline = fingerprint(java, lang, "clamp");
        assert_eq!(baseline, fingerprint(java_reformat, lang, "clamp"));
        assert_ne!(baseline, fingerprint(java_annotation, lang, "clamp"));
    }

    #[test]
    fn literal_whitespace_inside_annotations_is_specification() {
        // Different whitespace INSIDE a string literal is a different claim.
        let base = "//@ ensures message == \"bad frame\";\nint f(void) { return 0; }\n";
        let literal_edit = "//@ ensures message == \"bad  frame\";\nint f(void) { return 0; }\n";
        // Formatting AROUND the literal is still just formatting.
        let spaced = "//@   ensures   message   ==   \"bad frame\";\nint f(void) { return 0; }\n";

        let lang = FingerprintLanguage::C;
        let baseline = fingerprint(base, lang, "f");
        assert_ne!(baseline, fingerprint(literal_edit, lang, "f"));
        assert_eq!(baseline, fingerprint(spaced, lang, "f"));
    }

    #[test]
    fn unresolvable_symbols_read_as_unknown_not_unchanged() {
        assert!(
            symbol_fingerprint("int f(void) { return 0; }", FingerprintLanguage::C, "g").is_none()
        );
        assert!(symbol_fingerprint("not even c source", FingerprintLanguage::C, "f").is_none());
        assert!(FingerprintLanguage::for_path("src/x.py").is_none());
        assert_eq!(
            FingerprintLanguage::for_path("src/frame.h"),
            Some(FingerprintLanguage::C)
        );
    }
}
