/*
   line_comments //
   block_comments /* */

   keywords if else while loop fn match let use mod
   modifiers pub

   symbols ( ) { } [ ] < > = ! + - * / | : ;

   strings " "
   chars ' '
   literals true false
*/

use std::ops::Range;

use crate::pattern::{MatchResult, Pattern, PatternState};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TokenKind {
    Text,
    LineComment,
    BlockComment,
    Keyword,
    Modifier,
    Symbol,
    String,
    Char,
    Literal,
    Number,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineKind {
    Finished,
    Unfinished(usize, PatternState),
}

pub struct Syntax {
    rules: Vec<(TokenKind, Pattern)>,
}

impl Syntax {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn add_rule(&mut self, kind: TokenKind, pattern: Pattern) {
        self.rules.push((kind, pattern));
    }

    pub fn parse_line(
        &self,
        line: &str,
        previous_line_kind: LineKind,
        tokens: &mut Vec<Token>,
    ) -> LineKind {
        if self.rules.len() == 0 {
            tokens.push(Token {
                kind: TokenKind::Text,
                range: 0..line.len(),
            });
            return LineKind::Finished;
        }

        let line_len = line.len();
        let mut line_index = 0;

        match previous_line_kind {
            LineKind::Finished => (),
            LineKind::Unfinished(pattern_index, state) => match self.rules[pattern_index]
                .1
                .matches_from_state(line.as_bytes(), &state)
            {
                MatchResult::Ok(len) => {
                    tokens.push(Token {
                        kind: self.rules[pattern_index].0,
                        range: 0..len,
                    });
                    line_index += len;
                }
                MatchResult::Err => (),
                MatchResult::Pending(_, state) => {
                    tokens.push(Token {
                        kind: self.rules[pattern_index].0,
                        range: 0..line_len,
                    });
                    return LineKind::Unfinished(pattern_index, state);
                }
            },
        }

        while line_index < line_len {
            let line_slice = &line[line_index..].as_bytes();
            let whitespace_len = line_slice
                .iter()
                .take_while(|b| b.is_ascii_whitespace())
                .count();
            let line_slice = &line_slice[whitespace_len..];

            let mut best_pattern_index = 0;
            let mut max_len = 0;
            for (i, (kind, pattern)) in self.rules.iter().enumerate() {
                match pattern.matches(line_slice) {
                    MatchResult::Ok(len) => {
                        if len > max_len {
                            max_len = len;
                            best_pattern_index = i;
                        }
                    }
                    MatchResult::Err => (),
                    MatchResult::Pending(_, state) => {
                        tokens.push(Token {
                            kind: *kind,
                            range: line_index..line_len,
                        });
                        return LineKind::Unfinished(i, state);
                    }
                }
            }

            let mut kind = self.rules[best_pattern_index].0;

            if max_len == 0 {
                kind = TokenKind::Text;
                max_len = line_slice
                    .iter()
                    .take_while(|b| b.is_ascii_alphanumeric())
                    .count()
                    .max(1);
            }

            max_len += whitespace_len;

            let from = line_index;
            line_index = line_len.min(line_index + max_len);
            tokens.push(Token {
                kind,
                range: from..line_index,
            });
        }

        LineKind::Finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_token(slice: &str, kind: TokenKind, line: &str, token: &Token) {
        assert_eq!(kind, token.kind);
        assert_eq!(slice, &line[token.range.clone()]);
    }

    #[test]
    fn test_no_syntax() {
        let syntax = Syntax::new();
        let mut tokens = Vec::new();
        let line = " fn main() ;  ";
        let line_kind = syntax.parse_line(line, LineKind::Finished, &mut tokens);

        assert_eq!(LineKind::Finished, line_kind);
        assert_eq!(1, tokens.len());
        assert_token(line, TokenKind::Text, line, &tokens[0]);
    }

    #[test]
    fn test_one_rule_syntax() {
        let mut syntax = Syntax::new();
        syntax.add_rule(TokenKind::Symbol, Pattern::new(";").unwrap());

        let mut tokens = Vec::new();
        let line = " fn main() ;  ";
        let line_kind = syntax.parse_line(line, LineKind::Finished, &mut tokens);

        assert_eq!(LineKind::Finished, line_kind);
        assert_eq!(6, tokens.len());
        assert_token(" fn", TokenKind::Text, line, &tokens[0]);
        assert_token(" main", TokenKind::Text, line, &tokens[1]);
        assert_token("(", TokenKind::Text, line, &tokens[2]);
        assert_token(")", TokenKind::Text, line, &tokens[3]);
        assert_token(" ;", TokenKind::Symbol, line, &tokens[4]);
        assert_token("  ", TokenKind::Text, line, &tokens[5]);
    }

    #[test]
    fn test_simple_syntax() {
        let mut syntax = Syntax::new();
        syntax.add_rule(TokenKind::Keyword, Pattern::new("fn").unwrap());
        syntax.add_rule(TokenKind::Symbol, Pattern::new("(").unwrap());
        syntax.add_rule(TokenKind::Symbol, Pattern::new(")").unwrap());

        let mut tokens = Vec::new();
        let line = " fn main() ;  ";
        let line_kind = syntax.parse_line(line, LineKind::Finished, &mut tokens);

        assert_eq!(LineKind::Finished, line_kind);
        assert_eq!(6, tokens.len());
        assert_token(" fn", TokenKind::Keyword, line, &tokens[0]);
        assert_token(" main", TokenKind::Text, line, &tokens[1]);
        assert_token("(", TokenKind::Symbol, line, &tokens[2]);
        assert_token(")", TokenKind::Symbol, line, &tokens[3]);
        assert_token(" ;", TokenKind::Text, line, &tokens[4]);
        assert_token("  ", TokenKind::Text, line, &tokens[5]);
    }
}
