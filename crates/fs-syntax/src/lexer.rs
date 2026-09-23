//! Hand-written lexer for FeatureScript.
//!
//! The lexer works on bytes (FeatureScript source is UTF-8; the only
//! non-ASCII token that matters is the `✨` version placeholder, and
//! arbitrary UTF-8 is allowed inside strings and comments).

use crate::error::{ParseError, ParseResult};
use crate::span::Span;
use crate::token::{Keyword, Token, TokenKind};

pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    keep_doc_comments: bool,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Lexer {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            keep_doc_comments: true,
        }
    }

    /// Whether `/** ... */` comments are emitted as [`TokenKind::DocComment`]
    /// tokens (default) or dropped like ordinary comments.
    pub fn keep_doc_comments(mut self, keep: bool) -> Self {
        self.keep_doc_comments = keep;
        self
    }

    /// Tokenize the whole input. The last token is always `Eof`.
    pub fn tokenize(mut self) -> ParseResult<Vec<Token>> {
        let mut out = Vec::with_capacity(self.src.len() / 4);
        loop {
            let tok = self.next_token()?;
            let eof = tok.kind == TokenKind::Eof;
            out.push(tok);
            if eof {
                break;
            }
        }
        Ok(out)
    }

    fn peek(&self) -> u8 {
        self.bytes.get(self.pos).copied().unwrap_or(0)
    }

    fn peek_at(&self, n: usize) -> u8 {
        self.bytes.get(self.pos + n).copied().unwrap_or(0)
    }

    fn span_from(&self, start: usize) -> Span {
        Span::new(start as u32, self.pos as u32)
    }

    fn err<T>(&self, start: usize, msg: impl Into<String>) -> ParseResult<T> {
        Err(ParseError::new(self.span_from(start), msg))
    }

    /// Skip whitespace and non-doc comments. Returns a doc comment token if
    /// one was encountered and doc comments are being kept.
    fn skip_trivia(&mut self) -> ParseResult<Option<Token>> {
        loop {
            match self.peek() {
                b' ' | b'\t' | b'\r' | b'\n' | 0x0c => self.pos += 1,
                b'/' if self.peek_at(1) == b'/' => {
                    while self.pos < self.bytes.len() && self.peek() != b'\n' {
                        self.pos += 1;
                    }
                }
                b'/' if self.peek_at(1) == b'*' => {
                    let start = self.pos;
                    // `/**/` is an empty block comment, not a doc comment.
                    let is_doc = self.peek_at(2) == b'*' && self.peek_at(3) != b'/';
                    self.pos += 2;
                    loop {
                        if self.pos >= self.bytes.len() {
                            return self.err(start, "unterminated block comment");
                        }
                        if self.peek() == b'*' && self.peek_at(1) == b'/' {
                            self.pos += 2;
                            break;
                        }
                        self.pos += 1;
                    }
                    if is_doc && self.keep_doc_comments {
                        let text = &self.src[start + 3..self.pos - 2];
                        return Ok(Some(Token {
                            kind: TokenKind::DocComment(text.to_string()),
                            span: self.span_from(start),
                        }));
                    }
                }
                _ => return Ok(None),
            }
        }
    }

    fn next_token(&mut self) -> ParseResult<Token> {
        if let Some(doc) = self.skip_trivia()? {
            return Ok(doc);
        }
        let start = self.pos;
        if self.pos >= self.bytes.len() {
            return Ok(Token {
                kind: TokenKind::Eof,
                span: self.span_from(start),
            });
        }
        let c = self.peek();

        // Identifiers, keywords, @builtins.
        if c.is_ascii_alphabetic() || c == b'_' || c == b'@' {
            if c == b'@' {
                self.pos += 1;
                if !(self.peek().is_ascii_alphabetic() || self.peek() == b'_') {
                    return self.err(start, "expected identifier after `@`");
                }
            }
            while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
                self.pos += 1;
            }
            let text = &self.src[start..self.pos];
            let kind = if c == b'@' {
                TokenKind::Ident(text.to_string())
            } else {
                match Keyword::parse(text) {
                    Some(k) => TokenKind::Kw(k),
                    None => TokenKind::Ident(text.to_string()),
                }
            };
            return Ok(Token {
                kind,
                span: self.span_from(start),
            });
        }

        if c.is_ascii_digit() || (c == b'.' && self.peek_at(1).is_ascii_digit()) {
            return self.number(start);
        }

        if c == b'"' || c == b'\'' {
            return self.string(start, c);
        }

        // ✨ (U+2728) = E2 9C A8
        if c == 0xE2 && self.peek_at(1) == 0x9C && self.peek_at(2) == 0xA8 {
            self.pos += 3;
            return Ok(Token {
                kind: TokenKind::Sparkle,
                span: self.span_from(start),
            });
        }

        use TokenKind::*;
        let two = |a: u8, b: u8| -> bool { c == a && self.peek_at(1) == b };
        let three = |a: u8, b: u8, d: u8| -> bool {
            c == a && self.peek_at(1) == b && self.peek_at(2) == d
        };
        let (kind, len) = if three(b'|', b'|', b'=') {
            (PipePipeEq, 3)
        } else if three(b'&', b'&', b'=') {
            (AmpAmpEq, 3)
        } else if three(b'?', b'?', b'=') {
            (QuestionQuestionEq, 3)
        } else if two(b'?', b'.') {
            (QuestionDot, 2)
        } else if two(b'?', b'?') {
            (QuestionQuestion, 2)
        } else if two(b'-', b'>') {
            (Arrow, 2)
        } else if two(b'=', b'>') {
            (FatArrow, 2)
        } else if two(b'=', b'=') {
            (EqEq, 2)
        } else if two(b'!', b'=') {
            (BangEq, 2)
        } else if two(b'<', b'=') {
            (LtEq, 2)
        } else if two(b'>', b'=') {
            (GtEq, 2)
        } else if two(b'&', b'&') {
            (AmpAmp, 2)
        } else if two(b'|', b'|') {
            (PipePipe, 2)
        } else if two(b'+', b'=') {
            (PlusEq, 2)
        } else if two(b'-', b'=') {
            (MinusEq, 2)
        } else if two(b'*', b'=') {
            (StarEq, 2)
        } else if two(b'/', b'=') {
            (SlashEq, 2)
        } else if two(b'%', b'=') {
            (PercentEq, 2)
        } else if two(b'~', b'=') {
            (TildeEq, 2)
        } else if two(b'^', b'=') {
            (CaretEq, 2)
        } else {
            let k = match c {
                b'(' => LParen,
                b')' => RParen,
                b'{' => LBrace,
                b'}' => RBrace,
                b'[' => LBracket,
                b']' => RBracket,
                b',' => Comma,
                b';' => Semi,
                b':' => Colon,
                b'.' => Dot,
                b'?' => Question,
                b'+' => Plus,
                b'-' => Minus,
                b'*' => Star,
                b'/' => Slash,
                b'%' => Percent,
                b'^' => Caret,
                b'~' => Tilde,
                b'!' => Bang,
                b'=' => Eq,
                b'<' => Lt,
                b'>' => Gt,
                _ => {
                    let ch = self.src[start..].chars().next().unwrap();
                    self.pos += ch.len_utf8();
                    return self.err(start, format!("unexpected character `{ch}`"));
                }
            };
            (k, 1)
        };
        self.pos += len;
        Ok(Token {
            kind,
            span: self.span_from(start),
        })
    }

    fn number(&mut self, start: usize) -> ParseResult<Token> {
        // Integer part.
        while self.peek().is_ascii_digit() {
            self.pos += 1;
        }
        // Fraction: `1.5`, `1.`, `.5`. A `.` followed by an identifier char
        // is member access on a number (never seen in practice, but be safe).
        let next = self.peek_at(1);
        let exponent_follows = matches!(next, b'e' | b'E')
            && (self.peek_at(2).is_ascii_digit()
                || (matches!(self.peek_at(2), b'+' | b'-') && self.peek_at(3).is_ascii_digit()));
        if self.peek() == b'.'
            && (exponent_follows || !(next.is_ascii_alphabetic() || next == b'_'))
        {
            self.pos += 1;
            while self.peek().is_ascii_digit() {
                self.pos += 1;
            }
        }
        // Exponent.
        if matches!(self.peek(), b'e' | b'E') {
            let save = self.pos;
            self.pos += 1;
            if matches!(self.peek(), b'+' | b'-') {
                self.pos += 1;
            }
            if self.peek().is_ascii_digit() {
                while self.peek().is_ascii_digit() {
                    self.pos += 1;
                }
            } else {
                // Not an exponent after all (e.g. `2e` followed by nothing useful).
                self.pos = save;
            }
        }
        if self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
            return self.err(start, "invalid numeric literal");
        }
        let text = self.src[start..self.pos].to_string();
        Ok(Token {
            kind: TokenKind::Number(text),
            span: self.span_from(start),
        })
    }

    fn string(&mut self, start: usize, quote: u8) -> ParseResult<Token> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            if self.pos >= self.bytes.len() {
                return self.err(start, "unterminated string literal");
            }
            let c = self.peek();
            if c == quote {
                self.pos += 1;
                break;
            }
            if c == b'\\' {
                self.pos += 1;
                let e = self.peek();
                self.pos += 1;
                match e {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    b'0' => out.push('\0'),
                    b'\\' => out.push('\\'),
                    b'"' => out.push('"'),
                    b'\'' => out.push('\''),
                    b'/' => out.push('/'),
                    b'u' => {
                        let hex_start = self.pos;
                        while self.pos < hex_start + 4 && self.peek().is_ascii_hexdigit() {
                            self.pos += 1;
                        }
                        if self.pos != hex_start + 4 {
                            return self.err(hex_start - 2, "invalid \\u escape");
                        }
                        let code = u32::from_str_radix(&self.src[hex_start..self.pos], 16).unwrap();
                        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    }
                    0 => return self.err(start, "unterminated string literal"),
                    _ => {
                        // Unknown escape: keep it verbatim (FeatureScript regexes
                        // are written with `\\` so this rarely matters).
                        out.push('\\');
                        let ch = self.src[self.pos - 1..].chars().next().unwrap();
                        out.push(ch);
                        self.pos += ch.len_utf8() - 1;
                    }
                }
                continue;
            }
            // Copy one UTF-8 char.
            let ch = self.src[self.pos..].chars().next().unwrap();
            out.push(ch);
            self.pos += ch.len_utf8();
        }
        Ok(Token {
            kind: TokenKind::Str(out),
            span: self.span_from(start),
        })
    }
}

/// Convenience: tokenize a source string, dropping doc comments.
pub fn tokenize(src: &str) -> ParseResult<Vec<Token>> {
    Lexer::new(src).keep_doc_comments(false).tokenize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use TokenKind::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokenize(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn header_and_import() {
        let k = kinds(
            "FeatureScript ✨; import(path : \"onshape/std/geometry.fs\", version : \"✨\");",
        );
        assert_eq!(k[0], Kw(Keyword::FeatureScript));
        assert_eq!(k[1], Sparkle);
        assert_eq!(k[2], Semi);
        assert_eq!(k[3], Kw(Keyword::Import));
        assert_eq!(k[6], Colon);
        assert_eq!(k[7], Str("onshape/std/geometry.fs".into()));
        assert_eq!(k[11], Str("✨".into()));
    }

    #[test]
    fn numbers() {
        let k = kinds("1 2.5 1e-8 0. 1.e5 .5 3E+2");
        let nums: Vec<_> = k
            .iter()
            .filter_map(|t| {
                if let Number(s) = t {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(nums, ["1", "2.5", "1e-8", "0.", "1.e5", ".5", "3E+2"]);
    }

    #[test]
    fn operators() {
        let k =
            kinds("a -> b => c ?. d ?? e ~= f ^ g != h <= i >= j && k || l += m ||= n &&= o ??= p");
        let ops: Vec<_> = k
            .iter()
            .filter(|t| !matches!(t, Ident(_) | Eof))
            .cloned()
            .collect();
        assert_eq!(
            ops,
            vec![
                Arrow,
                FatArrow,
                QuestionDot,
                QuestionQuestion,
                TildeEq,
                Caret,
                BangEq,
                LtEq,
                GtEq,
                AmpAmp,
                PipePipe,
                PlusEq,
                PipePipeEq,
                AmpAmpEq,
                QuestionQuestionEq
            ]
        );
    }

    #[test]
    fn strings_and_escapes() {
        let k = kinds(r#"'a' "b\"c" "re\\." "tab\t" "#);
        assert_eq!(k[0], Str("a".into()));
        assert_eq!(k[1], Str("b\"c".into()));
        assert_eq!(k[2], Str("re\\.".into()));
        assert_eq!(k[3], Str("tab\t".into()));
    }

    #[test]
    fn comments() {
        let toks = Lexer::new("/** doc */ x // line\n /* block */ y /**/ z")
            .tokenize()
            .unwrap();
        let k: Vec<_> = toks.into_iter().map(|t| t.kind).collect();
        assert_eq!(
            k,
            vec![
                DocComment(" doc ".into()),
                Ident("x".into()),
                Ident("y".into()),
                Ident("z".into()),
                Eof
            ]
        );
    }

    #[test]
    fn builtins() {
        assert_eq!(kinds("@size(x)")[0], Ident("@size".into()));
    }

    #[test]
    fn errors_have_spans() {
        let e = tokenize("x = \"abc").unwrap_err();
        assert_eq!(e.span.start, 4);
        assert!(e.message.contains("unterminated"));
    }
}
