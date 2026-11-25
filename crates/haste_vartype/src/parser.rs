use crate::{Error, Token, TokenKind, Tokenizer};

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Lit<'a> {
    Str(&'a str),
    Num(i64),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Expr<'a> {
    Ident(&'a str),
    Lit(Lit<'a>),
    Pointer(Box<Expr<'a>>),
    // Foo< A, B, -1 >: one or more arguments, each a type or a number literal.
    Template {
        expr: Box<Expr<'a>>,
        args: Box<[Expr<'a>]>,
    },
    Array {
        expr: Box<Expr<'a>>,
        len: Box<Expr<'a>>,
    },
    Path(Box<[Expr<'a>]>),
}

#[derive(Debug, Clone)]
struct Parser<'a> {
    tokenizer: Tokenizer<'a>,
    prev_token: Option<Token<'a>>,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            tokenizer: Tokenizer::new(input),
            prev_token: None,
        }
    }

    #[inline(always)]
    fn next_token(&mut self) -> Result<Option<Token<'a>>, Error> {
        if let Some(token) = self.prev_token.take() {
            Ok(Some(token))
        } else {
            self.tokenizer.next().transpose()
        }
    }

    #[inline(always)]
    fn expect_token<F>(&mut self, f: F) -> Result<Token<'a>, Error>
    where
        F: FnOnce(&TokenKind<'a>) -> bool,
    {
        match self.next_token()? {
            Some(token) if f(&token.kind) => Ok(token),
            Some(token) => Err(Error::UnexpectedToken(token.span.start)),
            None => Err(Error::UnexpectedEof),
        }
    }

    fn parse_path(&mut self) -> Result<Expr<'a>, Error> {
        let segment = {
            let token = self.expect_token(|k| matches!(k, TokenKind::Ident(_)))?;
            let TokenKind::Ident(ident) = token.kind else {
                unreachable!();
            };
            Expr::Ident(ident)
        };
        match self.next_token()? {
            Some(Token {
                kind: TokenKind::ColonColon,
                ..
            }) => {}
            Some(token) => {
                self.prev_token = Some(token);
                return Ok(segment);
            }
            None => return Ok(segment),
        }

        let mut segments = vec![segment];
        loop {
            let segment = match self.next_token()? {
                Some(Token {
                    kind: TokenKind::Ident(ident),
                    ..
                }) => Expr::Ident(ident),
                Some(token) => {
                    self.prev_token = Some(token);
                    break;
                }
                None => break,
            };
            segments.push(segment);
            match self.next_token()? {
                Some(Token {
                    kind: TokenKind::ColonColon,
                    ..
                }) => {}
                Some(next) => {
                    self.prev_token = Some(next);
                    break;
                }
                None => break,
            }
        }
        Ok(Expr::Path(segments.into_boxed_slice()))
    }

    // a number literal token, as an Expr. "-1" -> Lit(Num(-1)).
    fn parse_lit(token: &Token<'a>) -> Result<Expr<'a>, Error> {
        let TokenKind::Lit(lit) = token.kind else {
            unreachable!();
        };
        let num = lit
            .parse::<i64>()
            .map_err(|_| Error::UnexpectedToken(token.span.start))?;
        Ok(Expr::Lit(Lit::Num(num)))
    }

    // one template argument: a number literal, or a whole type (which may itself be a
    // template, array, ...).
    fn parse_template_arg(&mut self) -> Result<Expr<'a>, Error> {
        match self.next_token()? {
            Some(token) if matches!(token.kind, TokenKind::Lit(_)) => Self::parse_lit(&token),
            Some(token) => {
                self.prev_token = Some(token);
                self.parse()
            }
            None => Err(Error::UnexpectedEof),
        }
    }

    fn parse(&mut self) -> Result<Expr<'a>, Error> {
        let mut expr = self.parse_path()?;

        while let Some(next) = self.next_token()? {
            match next.kind {
                TokenKind::LAngle => {
                    // arguments separated by commas, up to the closing '>'.
                    let mut args = Vec::new();
                    loop {
                        args.push(self.parse_template_arg()?);
                        let token = self.expect_token(|k| {
                            matches!(k, TokenKind::Comma | TokenKind::RAngle)
                        })?;
                        if token.kind == TokenKind::RAngle {
                            break;
                        }
                    }
                    expr = Expr::Template {
                        expr: Box::new(expr),
                        args: args.into_boxed_slice(),
                    };
                }
                TokenKind::LSquare => {
                    let token = self.expect_token(|_| true)?;
                    let len = match token.kind {
                        TokenKind::Ident(ident) => Expr::Ident(ident),
                        TokenKind::Lit(_) => Self::parse_lit(&token)?,
                        _ => return Err(Error::UnexpectedToken(token.span.start)),
                    };

                    let _rsquare = self.expect_token(|k| matches!(k, TokenKind::RSquare))?;
                    expr = Expr::Array {
                        expr: Box::new(expr),
                        len: Box::new(len),
                    };
                }
                TokenKind::Asterisk => {
                    expr = Expr::Pointer(Box::new(expr));
                }
                // these end this expression; the caller (a template or array) takes them.
                TokenKind::RAngle | TokenKind::RSquare | TokenKind::Comma => {
                    self.prev_token = Some(next);
                    break;
                }
                _ => return Err(Error::UnexpectedToken(next.span.start)),
            }
        }

        Ok(expr)
    }
}

pub fn parse(input: &str) -> Result<Expr<'_>, Error> {
    let mut parser = Parser::new(input);
    let expr = parser.parse()?;
    // the whole input must be used: "Foo >" or "Foo, Bar" is an error, not Foo.
    match parser.next_token()? {
        Some(token) => Err(Error::UnexpectedToken(token.span.start)),
        None => Ok(expr),
    }
}

#[cfg(test)]
mod tests {
    use expect_test::expect;

    use super::*;

    #[test]
    fn unexpected_eof() {
        let result = parse("");
        let expected = expect![[r#"
            Err(
                UnexpectedEof,
            )
        "#]];
        expected.assert_debug_eq(&result);
    }

    #[test]
    fn unexpected_token() {
        let result = parse("42");
        let expected = expect![[r#"
            Err(
                UnexpectedToken(
                    0,
                ),
            )
        "#]];
        expected.assert_debug_eq(&result);
    }

    #[test]
    fn malformed_templates() {
        // each must fail; the number is where.
        let cases = [
            ("A<>", 2),    // no argument
            ("A<B,>", 4),  // nothing after the comma
            ("A<B C>", 4), // missing comma
            ("A<B", 0),    // never closed (eof)
            ("A>", 1),     // stray '>'
            ("A, B", 1),   // stray ','
        ];
        for (input, at) in cases {
            match parse(input) {
                Err(Error::UnexpectedToken(pos)) => assert_eq!(pos, at, "{input}"),
                Err(Error::UnexpectedEof) => assert_eq!(at, 0, "{input}"),
                other => panic!("{input}: expected an error, got {other:?}"),
            }
        }
    }

    #[test]
    fn it_works() -> Result<(), Error> {
        const INPUTS: &[&'static str] = &[
            "uint64[256]",
            "CDOTAGameManager*",
            "CNetworkUtlVectorBase< CHandle< CBasePlayerController > >",
            "CHandle< CDOTASpecGraphPlayerData >[24]",
            "CDOTA_AbilityDraftAbilityState[MAX_ABILITY_DRAFT_ABILITIES]",
            "CCitadel_Ability_Baba_BubblingBrew::EState",
            "CNetworkUtlVectorBase< CNetworkUtlVectorBase< EEvent, NetworkVar_m_vecActiveEvents , -1, int > >",
        ];

        let mut outputs: Vec<Result<Expr<'static>, Error>> = Vec::with_capacity(INPUTS.len());

        INPUTS.iter().for_each(|input| outputs.push(parse(input)));

        let expected = expect![[r#"
            [
                Ok(
                    Array {
                        expr: Ident(
                            "uint64",
                        ),
                        len: Lit(
                            Num(
                                256,
                            ),
                        ),
                    },
                ),
                Ok(
                    Pointer(
                        Ident(
                            "CDOTAGameManager",
                        ),
                    ),
                ),
                Ok(
                    Template {
                        expr: Ident(
                            "CNetworkUtlVectorBase",
                        ),
                        args: [
                            Template {
                                expr: Ident(
                                    "CHandle",
                                ),
                                args: [
                                    Ident(
                                        "CBasePlayerController",
                                    ),
                                ],
                            },
                        ],
                    },
                ),
                Ok(
                    Array {
                        expr: Template {
                            expr: Ident(
                                "CHandle",
                            ),
                            args: [
                                Ident(
                                    "CDOTASpecGraphPlayerData",
                                ),
                            ],
                        },
                        len: Lit(
                            Num(
                                24,
                            ),
                        ),
                    },
                ),
                Ok(
                    Array {
                        expr: Ident(
                            "CDOTA_AbilityDraftAbilityState",
                        ),
                        len: Ident(
                            "MAX_ABILITY_DRAFT_ABILITIES",
                        ),
                    },
                ),
                Ok(
                    Path(
                        [
                            Ident(
                                "CCitadel_Ability_Baba_BubblingBrew",
                            ),
                            Ident(
                                "EState",
                            ),
                        ],
                    ),
                ),
                Ok(
                    Template {
                        expr: Ident(
                            "CNetworkUtlVectorBase",
                        ),
                        args: [
                            Template {
                                expr: Ident(
                                    "CNetworkUtlVectorBase",
                                ),
                                args: [
                                    Ident(
                                        "EEvent",
                                    ),
                                    Ident(
                                        "NetworkVar_m_vecActiveEvents",
                                    ),
                                    Lit(
                                        Num(
                                            -1,
                                        ),
                                    ),
                                    Ident(
                                        "int",
                                    ),
                                ],
                            },
                        ],
                    },
                ),
            ]
        "#]];
        expected.assert_debug_eq(&outputs);

        Ok(())
    }
}
