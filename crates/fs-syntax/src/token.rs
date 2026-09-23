//! Token definitions for the FeatureScript lexer.

use crate::span::Span;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    // ---- literals / names ----
    /// An identifier, including `@builtin` names (the `@` is kept in the text).
    Ident(String),
    /// Numeric literal, stored as the source text (parsed to f64 later).
    Number(String),
    /// String literal with escapes already processed.
    Str(String),
    /// A `/** ... */` documentation comment (text without delimiters). Only
    /// emitted when the lexer is configured to keep doc comments.
    DocComment(String),
    /// The `✨` placeholder used for version numbers in the std library mirror.
    Sparkle,

    // ---- keywords ----
    Kw(Keyword),

    // ---- punctuation ----
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    Dot,
    /// `?.`
    QuestionDot,
    Question,
    /// `??`
    QuestionQuestion,
    /// `->`
    Arrow,
    /// `=>`
    FatArrow,

    // ---- operators ----
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    Tilde,
    Bang,
    Eq,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    AmpAmp,
    PipePipe,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    TildeEq,
    CaretEq,
    /// `||=`
    PipePipeEq,
    /// `&&=`
    AmpAmpEq,
    /// `??=`
    QuestionQuestionEq,

    Eof,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Keyword {
    FeatureScript,
    Import,
    Export,
    Function,
    Predicate,
    Precondition,
    Returns,
    Type,
    Typecheck,
    Enum,
    Const,
    Var,
    If,
    Else,
    For,
    While,
    In,
    Is,
    As,
    Return,
    Break,
    Continue,
    Throw,
    Try,
    Silent,
    Catch,
    Switch,
    New,
    Annotation,
    Operator,
    True,
    False,
    Undefined,
}

impl Keyword {
    pub fn parse(s: &str) -> Option<Keyword> {
        use Keyword::*;
        Some(match s {
            "FeatureScript" => FeatureScript,
            "import" => Import,
            "export" => Export,
            "function" => Function,
            "predicate" => Predicate,
            "precondition" => Precondition,
            "returns" => Returns,
            "type" => Type,
            "typecheck" => Typecheck,
            "enum" => Enum,
            "const" => Const,
            "var" => Var,
            "if" => If,
            "else" => Else,
            "for" => For,
            "while" => While,
            "in" => In,
            "is" => Is,
            "as" => As,
            "return" => Return,
            "break" => Break,
            "continue" => Continue,
            "throw" => Throw,
            "try" => Try,
            "silent" => Silent,
            "catch" => Catch,
            "switch" => Switch,
            "new" => New,
            "annotation" => Annotation,
            "operator" => Operator,
            "true" => True,
            "false" => False,
            "undefined" => Undefined,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        use Keyword::*;
        match self {
            FeatureScript => "FeatureScript",
            Import => "import",
            Export => "export",
            Function => "function",
            Predicate => "predicate",
            Precondition => "precondition",
            Returns => "returns",
            Type => "type",
            Typecheck => "typecheck",
            Enum => "enum",
            Const => "const",
            Var => "var",
            If => "if",
            Else => "else",
            For => "for",
            While => "while",
            In => "in",
            Is => "is",
            As => "as",
            Return => "return",
            Break => "break",
            Continue => "continue",
            Throw => "throw",
            Try => "try",
            Silent => "silent",
            Catch => "catch",
            Switch => "switch",
            New => "new",
            Annotation => "annotation",
            Operator => "operator",
            True => "true",
            False => "false",
            Undefined => "undefined",
        }
    }

    /// Keywords that are "soft": they may also appear as ordinary
    /// identifiers (e.g. as a map key `{ "silent" : ... }` is a string so
    /// that's fine, but `silent`, `type`, `operator`, `in` and friends do show
    /// up as parameter or variable names in real code).
    pub fn is_soft(self) -> bool {
        use Keyword::*;
        matches!(
            self,
            Silent
                | Type
                | Typecheck
                | Operator
                | Precondition
                | Returns
                | Annotation
                | FeatureScript
                | New
                | Switch
        )
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use TokenKind::*;
        let s = match self {
            Ident(s) => return write!(f, "identifier `{s}`"),
            Number(s) => return write!(f, "number `{s}`"),
            Str(_) => "string literal",
            DocComment(_) => "doc comment",
            Sparkle => "`✨`",
            Kw(k) => return write!(f, "`{}`", k.as_str()),
            LParen => "`(`",
            RParen => "`)`",
            LBrace => "`{`",
            RBrace => "`}`",
            LBracket => "`[`",
            RBracket => "`]`",
            Comma => "`,`",
            Semi => "`;`",
            Colon => "`:`",
            Dot => "`.`",
            QuestionDot => "`?.`",
            Question => "`?`",
            QuestionQuestion => "`??`",
            Arrow => "`->`",
            FatArrow => "`=>`",
            Plus => "`+`",
            Minus => "`-`",
            Star => "`*`",
            Slash => "`/`",
            Percent => "`%`",
            Caret => "`^`",
            Tilde => "`~`",
            Bang => "`!`",
            Eq => "`=`",
            EqEq => "`==`",
            BangEq => "`!=`",
            Lt => "`<`",
            LtEq => "`<=`",
            Gt => "`>`",
            GtEq => "`>=`",
            AmpAmp => "`&&`",
            PipePipe => "`||`",
            PlusEq => "`+=`",
            MinusEq => "`-=`",
            StarEq => "`*=`",
            SlashEq => "`/=`",
            PercentEq => "`%=`",
            TildeEq => "`~=`",
            CaretEq => "`^=`",
            PipePipeEq => "`||=`",
            AmpAmpEq => "`&&=`",
            QuestionQuestionEq => "`??=`",
            Eof => "end of file",
        };
        f.write_str(s)
    }
}
