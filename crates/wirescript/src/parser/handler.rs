//! The `on …` grammar: triggers, handler bodies, and the expression-trigger
//! desugar.

use super::*;

pub(super) fn trigger_to_expr(t: &Trigger) -> Expr {
    match t {
        Trigger::Ident { name, range } => Expr::Ident {
            name: name.clone(),
            range: range.clone(),
        },
        Trigger::Field { obj, field, range } => Expr::FieldAccess {
            obj: Box::new(Expr::Ident {
                name: obj.clone(),
                range: range.clone(),
            }),
            field: field.clone(),
            range: range.clone(),
        },
        Trigger::Not { inner, range } => Expr::UnOp {
            op: "!".into(),
            operand: Box::new(trigger_to_expr(inner)),
            range: range.clone(),
        },
        Trigger::Union { parts, range } => {
            if let Some(first) = parts.first() {
                trigger_to_expr(first)
            } else {
                Expr::Ident {
                    name: String::new(),
                    range: range.clone(),
                }
            }
        }
    }
}

/// Whether a general expression trigger's expression is a call carrying an
/// `exec = <x>` named arg (`on doWork(5, exec = s)`). A trailing postfix
/// (`on doWork(5, exec = s).field`) is unwrapped to the underlying call so the
/// `exec =` intent is still seen. Used to flag the `Handler` so lowering can
/// reject an `exec =` whose callee has no completion exec (WS043) instead of
/// silently orphaning it.
fn expr_call_has_exec_arg(e: &Expr) -> bool {
    match e {
        Expr::Call { args, .. } => args
            .iter()
            .any(|a| matches!(a, CallArg::Named { name, .. } if name == "exec")),
        Expr::FieldAccess { obj, .. }
        | Expr::TuplePick { obj, .. }
        | Expr::IndexAccess { obj, .. } => expr_call_has_exec_arg(obj),
        _ => false,
    }
}

impl<'a> Parser<'a> {
    /// Return `true` when the tokens after `on` look like an arbitrary
    /// expression rather than a simple trigger pattern.
    ///
    /// A *simple* trigger consists of:
    ///   `!* ident (. ident)?` repeated, separated by `|`
    /// If, after scanning that pattern, the next real token is `{` or `(`
    /// (body / params), the trigger is simple.  Any other token (e.g. `&&`,
    /// `||`, `+`, a literal, …) means the user wrote an expression trigger.
    fn looks_like_expr_trigger(&self) -> bool {
        let mut i = self.pos;
        let len = self.tokens.len();
        let get = |idx: usize| -> &Token {
            self.tokens
                .get(idx)
                .unwrap_or_else(|| self.tokens.last().unwrap())
        };

        // `on Foo(...)` where `Foo` is NOT a known event is a call-expression
        // trigger — `on ServerUptime()` fires whenever a builtin's pure value
        // changes, and `on myMod(x, exec = go)` (Task 6: general/mod-exec
        // triggers) fires on the call's own exec-typed output. Both desugar the
        // same way, like any other expr trigger: `let _on_expr_N = Foo(...)` +
        // `on _on_expr_N`. The parser can't tell a builtin call from a
        // user-declared chip/mod call from a genuinely unknown name here (chips
        // and mods aren't tracked by the parser — they may be declared later in
        // the file, or imported), so ANY non-event `Ident(...)` is routed
        // through the expr-trigger desugar; lowering resolves what `Foo`
        // actually is. This is distinct from the event-with-args form (`on
        // Clock(...)`, `on CustomEvent(...)`), whose name resolves as an event
        // and stays a plain trigger with config/data args.
        if get(i).kind == TokenKind::Ident
            && get(i + 1).kind == TokenKind::LParen
            && crate::catalog::events::find_event(&get(i).text).is_none()
        {
            return true;
        }

        // Skip past `idx` (which must be an open bracket) to just after its
        // matching close, so a `(...)`/`[...]` in the head is stepped over as a
        // unit rather than mistaken for an atom boundary.
        let skip_balanced = |mut idx: usize| -> usize {
            let (open, close) = match get(idx).kind {
                TokenKind::LParen => (TokenKind::LParen, TokenKind::RParen),
                TokenKind::LBracket => (TokenKind::LBracket, TokenKind::RBracket),
                _ => return idx,
            };
            let mut depth = 0usize;
            while idx < len {
                let k = get(idx).kind;
                if k == open {
                    depth += 1;
                } else if k == close {
                    depth -= 1;
                    if depth == 0 {
                        return idx + 1;
                    }
                }
                idx += 1;
            }
            idx
        };

        // Skip one or more `|`-separated trigger atoms.  Each atom is:
        //   `!*  ident  (.ident | .ident(...) | [...])*`
        // A `.ident(...)` method call or `[...]` index is a *value* postfix that
        // no plain trigger can take, so its presence forces an expression trigger
        // (e.g. `on a.Dot(b) > 0` or `on arr[i] > 0`). A bare `.ident` (a field
        // trigger like `on split.Jump`) and a bare `ident(...)` (an event's
        // config args, `on Clock(...)`) stay plain.
        let mut has_value_postfix = false;
        loop {
            // Skip leading `!` prefixes.
            while i < len && get(i).kind == TokenKind::Op && get(i).text == "!" {
                i += 1;
            }
            // Must see an ident for a simple trigger.
            if i >= len || get(i).kind != TokenKind::Ident {
                // Non-ident at atom start → expression trigger (e.g. a literal
                // or a `(` grouping used as expression, not trigger grouping).
                // Actually `(` is also valid for trigger grouping; treat as
                // expression only when it's not an ident or `!`.
                return get(i).kind != TokenKind::LParen && get(i).kind != TokenKind::Ident;
            }
            i += 1; // consume ident

            // Postfix chain: `.field`, `.field(...)` (method call), `[...]` (index).
            loop {
                match get(i).kind {
                    TokenKind::Dot => {
                        i += 1;
                        if i < len && get(i).kind == TokenKind::Ident {
                            i += 1;
                        }
                        if i < len && get(i).kind == TokenKind::LParen {
                            has_value_postfix = true; // method call → value expr
                            i = skip_balanced(i);
                        }
                    }
                    TokenKind::LBracket => {
                        has_value_postfix = true; // index → value expr
                        i = skip_balanced(i);
                    }
                    _ => break,
                }
            }

            // Is the next token a `|` (trigger union)?  If so, continue loop.
            if i < len && get(i).kind == TokenKind::Op && get(i).text == "|" {
                i += 1; // consume `|`
                continue;
            }
            break;
        }

        // A value-only postfix in the head is never a plain trigger.
        if has_value_postfix {
            return true;
        }

        // After the last atom the next meaningful token should be `{` or `(`.
        // Anything else (e.g. `&&`, `||`, `+`, …) means expression trigger.
        let t = get(i);
        !matches!(
            t.kind,
            TokenKind::LBrace
                | TokenKind::LParen
                | TokenKind::Newline
                | TokenKind::Semi
                | TokenKind::Eof
        )
    }

    // `event name = expr` or `event name = on Trigger { body }`
    pub(super) fn parse_handler(&mut self, no_fold: bool) -> Handler {
        let start = self.expect(TokenKind::Kw, Some("on")).start;

        // For expression triggers we build a synthetic let binding that is
        // queued in `pending_stmts` AFTER the body is parsed.  This avoids
        // the body's own `parse_block` call draining the pending queue early.
        let mut pending_let: Option<LetDecl> = None;
        // Whether a general expression trigger's call carried an `exec = <x>`
        // arg — set below and stored on the `Handler` so lowering can reject
        // (WS043) an `exec =` whose callee exposes no completion exec.
        let mut expr_trigger_has_exec_arg = false;

        let trigger = if self.looks_like_expr_trigger() {
            // `on <expr> { body }` — desugar into:
            //   let _on_expr_N = <expr>
            //   on _on_expr_N { body }
            let expr = self.parse_expr();
            expr_trigger_has_exec_arg = expr_call_has_exec_arg(&expr);
            let expr_range = expr.range().clone();
            let n = self.expr_trigger_counter;
            self.expr_trigger_counter += 1;
            let synth_name = format!("_on_expr_{}", n);

            pending_let = Some(LetDecl {
                binding: LetBinding::Ident {
                    name: synth_name.clone(),
                    range: expr_range.clone(),
                },
                typ: None,
                value: expr,
                no_fold: false,
                range: expr_range.clone(),
            });

            Trigger::Ident {
                name: synth_name,
                range: expr_range,
            }
        } else {
            self.parse_trigger()
        };

        // Task 10: an event trigger is a CALL, uniform with `on <call> ->
        // (…)` — `on RoundStart { }` must be written `on RoundStart() { }`.
        // Recover as if `()` were present so the rest of the handler parses.
        self.require_event_call_parens(&trigger);

        // Trigger args: the event call's parens hold config ONLY — string/
        // number literals and `name = value` pairs (e.g. `on ChatCommand("greet",
        // Description = "Greets you")`). Binding the event's data outputs is
        // done exclusively via a trailing `-> <pattern>` capture, parsed below.
        let mut params: Vec<HandlerParam> = Vec::new();
        let mut config: Vec<HandlerConfigArg> = Vec::new();
        if self.match_tok(TokenKind::LParen, None).is_some() {
            while !self.check(TokenKind::RParen, None) {
                if self.check(TokenKind::Ident, None) {
                    let tok = self.expect(TokenKind::Ident, None);
                    let name = tok.text;
                    if self.match_tok(TokenKind::Op, Some("=")).is_some() {
                        let value = self.parse_expr();
                        config.push(HandlerConfigArg::Named { name, value });
                    } else {
                        // A bare ident or `name: type` here used to bind the
                        // event's data outputs inline; that form is removed —
                        // steer to the `->` capture instead.
                        let mut end = tok.end;
                        if self.match_tok(TokenKind::Colon, None).is_some() {
                            end = self.parse_type().range().end;
                        }
                        self.error(
                            "bind event outputs with `-> (a, b)`, not inside the event call \
                             (`E(a) { }` is now `E() -> (a) { }`)",
                            tok.start,
                            end,
                        );
                    }
                } else {
                    let value = self.parse_expr();
                    config.push(HandlerConfigArg::Positional(value));
                }
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
            }
            self.expect(TokenKind::RParen, None);
        }

        // Optional `-> <pattern>` output capture (Task 5): binds the event's
        // data outputs from a trailing tuple/record pattern. `->` is already a
        // lexer token (mod/chip outputs use it) — this is grammar extension
        // only. It is now the ONLY way to bind handler params (Task 8).
        if self.match_tok(TokenKind::Arrow, None).is_some() {
            self.parse_handler_arrow_pattern(&trigger, &mut params);
        }

        let body = self.parse_block();
        let end = body.range.end;

        // Queue the synthetic let AFTER parsing the body so that parse_block
        // doesn't accidentally drain it into the handler body.
        if let Some(let_decl) = pending_let {
            self.pending_stmts.push(Stmt::Let(let_decl));
        }

        Handler {
            trigger,
            params,
            config,
            body,
            no_fold,
            expr_trigger_has_exec_arg,
            range: self.make_range(start, end),
        }
    }

    /// Parses `(a: int, b)` (tuple — positional, for ANY event; per-slot type
    /// optional, inferred for custom events, catalog-typed for named events) or
    /// `{ name, name: alias }` (record — by field name, named-data events only;
    /// subset + rename) after `on <EventCall> ->`, replacing `params` with the
    /// capture's bindings. `trigger` is the already-parsed handler trigger (used
    /// to reject records on custom events and, for records, to resolve field
    /// names against `catalog::events::find_event`'s data order).
    fn parse_handler_arrow_pattern(&mut self, trigger: &Trigger, params: &mut Vec<HandlerParam>) {
        // `params` is always empty on entry: the trigger-args loop above can
        // no longer produce params (Task 8 removed inline data-param
        // parsing), so there is nothing here to guard against.
        let trigger_name = match trigger {
            Trigger::Ident { name, .. } => Some(name.as_str()),
            _ => None,
        };
        let is_custom = matches!(trigger_name, Some("CustomEvent") | Some("GlobalCustomEvent"));
        let event_spec = trigger_name.and_then(crate::catalog::events::find_event);

        if self.check(TokenKind::LParen, None) {
            self.advance(); // consume `(` — tuple capture binds positionally for
            // any event (named events are catalog-typed, custom events typed or
            // inferred); no per-event restriction.
            let mut new_params = Vec::new();
            self.eat_newlines();
            while !self.check(TokenKind::RParen, None) && self.peek().kind != TokenKind::Eof {
                let name_tok = self.expect(TokenKind::Ident, None);
                let prange = self.make_range(name_tok.start, name_tok.end);
                let ty = if self.match_tok(TokenKind::Colon, None).is_some() {
                    Some(self.parse_type())
                } else {
                    None
                };
                new_params.push(HandlerParam {
                    name: name_tok.text,
                    ty,
                    range: prange,
                    source_field: None,
                });
                self.eat_newlines();
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
                self.eat_newlines();
            }
            self.expect(TokenKind::RParen, None);
            *params = new_params;
            return;
        }

        // Bare single capture: `-> p` is shorthand for `-> (p)` — one untyped
        // positional binding. A type needs the parenthesized form (`-> (p: T)`);
        // a `:` here is reported and swallowed so the handler body still parses.
        if self.check(TokenKind::Ident, None) {
            let name_tok = self.expect(TokenKind::Ident, None);
            let prange = self.make_range(name_tok.start, name_tok.end);
            if self.check(TokenKind::Colon, None) {
                let (cs, ce) = (self.peek().start, self.peek().end);
                self.error(
                    "a typed single capture needs parens: write `-> (name: T)`",
                    cs,
                    ce,
                );
                self.advance(); // consume `:`
                let _ = self.parse_type(); // swallow the type for clean recovery
            }
            *params = vec![HandlerParam {
                name: name_tok.text,
                ty: None,
                range: prange,
                source_field: None,
            }];
            return;
        }

        if self.check(TokenKind::LBrace, None) {
            let lbrace_start = self.advance().start; // consume `{`
            if is_custom {
                self.error(
                    "record `{ }` output capture is for named-data events; \
                     CustomEvent/GlobalCustomEvent use `( )`",
                    lbrace_start,
                    lbrace_start,
                );
            }
            let mut fields: Vec<RecordDestructField> = Vec::new();
            self.eat_newlines();
            while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
                if self.check(TokenKind::Op, Some("...")) {
                    // There is no record value to spread here — only a parse
                    // error, not a silently-accepted no-op.
                    let spread_start = self.advance().start;
                    let rest_tok = self.expect(TokenKind::Ident, None);
                    self.error(
                        "`...` rest capture is not valid in an event output pattern",
                        spread_start,
                        rest_tok.end,
                    );
                    self.eat_newlines();
                    break;
                }
                let name_tok = self.expect(TokenKind::Ident, None);
                let alias = if self.match_tok(TokenKind::Colon, None).is_some() {
                    Some(self.expect(TokenKind::Ident, None).text)
                } else {
                    None
                };
                let field_end = self.peek().start;
                fields.push(RecordDestructField::Named {
                    name: name_tok.text,
                    alias,
                    range: self.make_range(name_tok.start, field_end),
                });
                self.eat_newlines();
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
                self.eat_newlines();
            }
            let rbrace_end = self.expect(TokenKind::RBrace, None).end;
            let fill_range = self.make_range(lbrace_start, rbrace_end);

            *params = match (event_spec, trigger_name) {
                (Some(spec), Some(event_name)) => {
                    self.resolve_arrow_record_capture(event_name, spec, &fields, &fill_range)
                }
                // Non-catalog trigger (a general expression — mod/chip call):
                // the field ORDER/shape isn't known at parse time (mod/chip
                // signatures aren't tracked by the parser — forward decls,
                // imports), so resolution by name happens at LOWERING time
                // against the call's actual result record (see the general-expr
                // trigger case in `lower/handler.rs`). `name` carries the local
                // bound name (the alias when given); `source_field` preserves the
                // ORIGINAL field name to look up when it differs — set
                // unconditionally here so lowering always has it, even when no
                // alias was written (`name == source_field` then).
                _ => fields
                    .into_iter()
                    .filter_map(|f| match f {
                        RecordDestructField::Named { name, alias, range } => Some(HandlerParam {
                            name: alias.unwrap_or_else(|| name.clone()),
                            ty: None,
                            range,
                            source_field: Some(name),
                        }),
                        RecordDestructField::Rest { .. } => None,
                    })
                    .collect(),
            };
        }
    }

    /// Resolves a `-> { name, name: alias, … }` record capture's fields
    /// against `spec.data`'s binding names into a POSITIONAL `HandlerParam`
    /// list — built-in event params bind by slot index, not by name. Each
    /// matched field lands at its event data-slot index; any leading gap
    /// (an uncaptured slot before the highest captured index) is filled with
    /// a fresh unused name so the positional shape stays intact. An unknown
    /// field name is a parse error listing the event's valid data names.
    fn resolve_arrow_record_capture(
        &mut self,
        event_name: &str,
        spec: &crate::catalog::events::EventSpec,
        fields: &[RecordDestructField],
        fill_range: &SourceRange,
    ) -> Vec<HandlerParam> {
        let mut resolved: Vec<Option<HandlerParam>> = vec![None; spec.data.len()];
        let mut max_index: Option<usize> = None;
        for f in fields {
            let (name, alias, range) = match f {
                RecordDestructField::Named { name, alias, range } => (name, alias, range),
                RecordDestructField::Rest { .. } => continue,
            };
            match spec.data.iter().position(|d| d.name == name.as_str()) {
                Some(idx) => {
                    let local = alias.clone().unwrap_or_else(|| name.clone());
                    resolved[idx] = Some(HandlerParam {
                        name: local,
                        ty: None,
                        range: range.clone(),
                        source_field: None,
                    });
                    max_index = Some(max_index.map_or(idx, |m| m.max(idx)));
                }
                None => {
                    let valid: Vec<&str> = spec.data.iter().map(|d| d.name).collect();
                    self.error(
                        format!(
                            "event `{}` has no data output `{}` (valid: {})",
                            event_name,
                            name,
                            valid.join(", ")
                        ),
                        range.start,
                        range.end,
                    );
                }
            }
        }
        match max_index {
            Some(max_idx) => (0..=max_idx)
                .map(|i| {
                    resolved[i].take().unwrap_or_else(|| HandlerParam {
                        name: format!("_arrow_unused_{}", i),
                        ty: None,
                        range: fill_range.clone(),
                        source_field: None,
                    })
                })
                .collect(),
            None => Vec::new(),
        }
    }

    /// Task 10: an event trigger is a CALL — `on RoundStart()` / `let x = on
    /// RoundStart()`, never the bare `on RoundStart`. If `trigger` is a plain
    /// event-name `Ident` not immediately followed by `(`, emit the "must be
    /// called" error. Only built-in event idents are affected; non-event idents
    /// (`on someVar`), `Field`/`Not`/`Union`, and desugared expr triggers are
    /// left alone. The caller recovers as if `()` were present.
    pub(super) fn require_event_call_parens(&mut self, trigger: &Trigger) {
        if let Trigger::Ident { name, range } = trigger
            && crate::catalog::events::find_event(name).is_some()
            && !self.check(TokenKind::LParen, None)
        {
            self.error(
                format!("event `{name}` must be called: write `on {name}()`"),
                range.start,
                range.end,
            );
        }
    }

    pub(super) fn parse_trigger(&mut self) -> Trigger {
        let mut first = self.parse_trigger_atom();
        // Optional `|`-separated union.
        let mut parts: Vec<Trigger> = Vec::new();
        while self.check(TokenKind::Op, Some("|")) {
            // Only treat `|` as a trigger-union if followed by another atom.
            let save = self.pos;
            self.advance();
            let nxt = self.parse_trigger_atom();
            if parts.is_empty() {
                parts.push(first.clone());
            }
            parts.push(nxt);
            // keep going
            let _ = save;
        }
        if parts.is_empty() {
            first
        } else {
            let start = match &parts[0] {
                Trigger::Ident { range, .. }
                | Trigger::Field { range, .. }
                | Trigger::Not { range, .. }
                | Trigger::Union { range, .. } => range.start,
            };
            let end = match parts.last().unwrap() {
                Trigger::Ident { range, .. }
                | Trigger::Field { range, .. }
                | Trigger::Not { range, .. }
                | Trigger::Union { range, .. } => range.end,
            };
            // Drop `first` from capture when empty; use `parts[0]` as the new first.
            let _ = &mut first;
            Trigger::Union {
                parts,
                range: self.make_range(start, end),
            }
        }
    }

    fn parse_trigger_atom(&mut self) -> Trigger {
        let t = self.peek().clone();
        if t.kind == TokenKind::LParen {
            self.advance();
            let inner = self.parse_trigger();
            self.expect(TokenKind::RParen, None);
            return inner;
        }
        if t.kind == TokenKind::Op && t.text == "!" {
            let start = t.start;
            self.advance();
            let inner = self.parse_trigger_atom();
            let end = match &inner {
                Trigger::Ident { range, .. }
                | Trigger::Field { range, .. }
                | Trigger::Not { range, .. }
                | Trigger::Union { range, .. } => range.end,
            };
            return Trigger::Not {
                inner: Box::new(inner),
                range: self.make_range(start, end),
            };
        }
        let name_tok = self.expect(TokenKind::Ident, None);
        if self.match_tok(TokenKind::Dot, None).is_some() {
            let field_tok = self.expect(TokenKind::Ident, None);
            return Trigger::Field {
                obj: name_tok.text,
                field: field_tok.text,
                range: self.make_range(name_tok.start, field_tok.end),
            };
        }
        Trigger::Ident {
            name: name_tok.text,
            range: self.make_range(name_tok.start, name_tok.end),
        }
    }
}
