//! A tokenizer for policy inspection, never for executing a command.

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Token {
    Word(String),
    Pipe,
    Break,
    Redirect(bool),
}

pub(crate) struct Lexed {
    pub tokens: Vec<Token>,
    pub simple: bool,
}

pub(crate) fn lex(input: &str) -> Lexed {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    let mut simple = true;
    let mut chars = input.chars().peekable();
    let flush = |tokens: &mut Vec<_>, word: &mut String, started: &mut bool| {
        if std::mem::take(started) {
            tokens.push(Token::Word(std::mem::take(word)));
        }
    };

    while let Some(ch) = chars.next() {
        if quote == Some('\'') {
            if ch == '\'' {
                quote = None;
            } else {
                word.push(ch);
            }
            continue;
        }
        if ch == '\\' {
            started = true;
            match chars.next() {
                Some('\n') => simple = false,
                // In double quotes only these characters are escaped by a backslash.
                Some(next) if quote.is_none() || "$`\\\"".contains(next) => word.push(next),
                Some(next) => {
                    word.push(ch);
                    word.push(next);
                }
                None => simple = false,
            }
            continue;
        }
        if quote == Some('"') {
            if ch == '"' {
                quote = None;
            } else {
                simple &= !matches!(ch, '$' | '`' | '!');
                word.push(ch);
            }
            continue;
        }

        match ch {
            '\'' | '"' => {
                started = true;
                quote = Some(ch);
            }
            ' ' | '\t' => flush(&mut tokens, &mut word, &mut started),
            '#' if !started => {
                simple = false;
                while chars.peek().is_some_and(|ch| *ch != '\n') {
                    chars.next();
                }
            }
            '|' | '&' | ';' | '\n' | '(' | ')' | '<' | '>' => {
                flush(&mut tokens, &mut word, &mut started);
                simple = false;
                let token = match ch {
                    '|' if chars.peek() != Some(&'|') => {
                        if chars.peek() == Some(&'&') {
                            chars.next();
                        }
                        Token::Pipe
                    }
                    '<' | '>' => Token::Redirect(ch == '>'),
                    _ => Token::Break,
                };
                if matches!(ch, '|' | '&' | '<' | '>' | ';') && chars.peek() == Some(&ch) {
                    chars.next();
                }
                tokens.push(token);
            }
            _ => {
                started = true;
                simple &= !matches!(
                    ch,
                    '$' | '`' | '~' | '*' | '?' | '[' | ']' | '{' | '}' | '!'
                );
                simple &= !ch.is_control();
                word.push(ch);
            }
        }
    }

    flush(&mut tokens, &mut word, &mut started);
    // Deliberately treat even quoted or escaped metacharacters as ineligible for auto-approval.
    simple &= quote.is_none()
        && !input.contains([';', '&', '|', '`', '>', '<', '\n', '\r'])
        && !input.contains("$(");
    Lexed { tokens, simple }
}

pub(crate) fn assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.as_bytes()[0].is_ascii_digit()
            && name
                .bytes()
                .all(|ch| ch == b'_' || ch.is_ascii_alphanumeric())
    })
}
