//! `.wsir` text -> Program. Two passes: a recursive-descent pass over the
//! token list that records nodes with unresolved names, then one resolution
//! pass that assigns ids, resolves every `%name` through the name table,
//! builds ports, properties and wires, and assembles the module tree.

use std::borrow::Cow;
use std::sync::Arc;

use super::defaults::{self, AnnValue, word};
use super::lex::{Tok, Token, lex};
use crate::collections::{HashMap, HashSet};
use crate::diagnostic::{Pos, SourceRange};
use crate::intern::{Sym, intern, resolve, sym};
use crate::ir::module_tree::{ModuleTree, nest_bodies};
use crate::ir::port_registry::WirePort;
use crate::ir::{
    GateIO, LayoutMode, Literal, Module, Node, NodeId, NodeKind, PortRef, PortSpec, Program, ProgramSettings,
    ROOT_SCOPE_ID, ScopeInfo, SourceComment, SourceMap, StyleValue, Type, Wire,
};

/// A parse or resolution error: where, what was found, what was expected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// The offending token, as it reads in the message.
    pub found: String,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}: {} (found {})", self.file, self.line, self.col, self.message, self.found)
    }
}

impl std::error::Error for ParseError {}

/// Parse `.wsir` text. `file` is the name errors give for it.
pub fn parse_program(src: &str, file: &str) -> Result<Program, ParseError> {
    let toks = lex(src).map_err(|e| ParseError {
        file: file.to_string(),
        line: e.line,
        col: e.col,
        found: format!("`{}`", e.found),
        message: e.message,
    })?;
    let mut p = Parser {
        toks,
        pos: 0,
        file,
        nodes: Vec::new(),
        mods: Vec::new(),
        tree: ModuleTree::new(),
        dirs: Directives::default(),
        files: HashMap::default(),
    };
    p.file()?;
    p.resolve()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    Gate,
    In,
    Out,
    Chip,
}

#[derive(Clone, Copy)]
struct At {
    line: u32,
    col: u32,
}

struct PRef<'a> {
    name: &'a str,
    port: Option<&'a str>,
    at: At,
}

enum IdRef<'a> {
    Name(&'a str, At),
    Id(u32),
}

enum POperand<'a> {
    Ref(PRef<'a>),
    Lit(Literal),
}

enum PAttrVal<'a> {
    Flag,
    Lit(Literal),
    Word(&'a str),
    Ref(PRef<'a>),
    /// `[%a, %b]`: a list of node references.
    Refs(Vec<PRef<'a>>),
    /// `map[k => %a]`: constant keys to node references.
    RefMap(Vec<(Literal, PRef<'a>)>),
}

struct PAttr<'a> {
    key: Cow<'a, str>,
    quoted: bool,
    val: PAttrVal<'a>,
    at: At,
}

struct PNode<'a> {
    name: &'a str,
    at: At,
    module: usize,
    form: Form,
    class: &'static str,
    pin_type: Option<Type>,
    operands: Vec<(&'a str, POperand<'a>, At)>,
    attrs: Vec<PAttr<'a>>,
    sig: Option<GateIO>,
    id_meta: Option<u32>,
    kind_meta: Option<NodeKind>,
    scope: u32,
    chain: Option<u32>,
    chip: Option<IdRef<'a>>,
    label: Option<PRef<'a>>,
    color: Option<PRef<'a>>,
    material: Option<PRef<'a>>,
    layout_after: Vec<IdRef<'a>>,
    note: Option<&'static str>,
    loc: SourceRange,
    child: Option<usize>,
}

struct PModule<'a> {
    /// `None` for a chip body written without `@name`.
    name: Option<String>,
    /// Index of the chip node whose body this is.
    chip_node: Option<usize>,
    nodes: Vec<usize>,
    scopes: Vec<(u32, ScopeInfo)>,
    inputs: Option<Vec<IdRef<'a>>>,
    outputs: Option<Vec<IdRef<'a>>>,
    wire_order: Option<(Vec<u32>, At)>,
}

/// A root `color`/`material` directive: a literal, or the `_from` port.
enum PStyle<'a> {
    Baked(Literal),
    Wired(PRef<'a>),
}

#[derive(Default)]
struct Directives<'a> {
    source_file: Option<String>,
    layout: LayoutMode,
    invisible: bool,
    no_gate_labels: bool,
    no_fold: bool,
    doc: Option<String>,
    label: Option<String>,
    label_from: Option<PRef<'a>>,
    color: Option<PStyle<'a>>,
    material: Option<PStyle<'a>>,
    source: Option<(String, Vec<u32>)>,
    comments: Vec<SourceComment>,
}

struct Parser<'a> {
    toks: Vec<Token<'a>>,
    pos: usize,
    file: &'a str,
    nodes: Vec<PNode<'a>>,
    mods: Vec<PModule<'a>>,
    tree: ModuleTree,
    dirs: Directives<'a>,
    files: HashMap<String, Arc<str>>,
}

type R<T> = Result<T, ParseError>;

const NODE_META: &[&str] = &["id", "scope", "chain", "chip", "label", "color", "material", "layout_after", "kind", "note"];

impl<'a> Parser<'a> {
    // ---------- token helpers ----------

    fn peek(&self) -> &Tok<'a> {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok<'a> {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    fn at(&self) -> At {
        let t = &self.toks[self.pos];
        At { line: t.line, col: t.col }
    }

    fn bump(&mut self) -> Tok<'a> {
        let t = self.toks[self.pos].tok.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn err_here(&self, expected: impl Into<String>) -> ParseError {
        let t = &self.toks[self.pos];
        ParseError {
            file: self.file.to_string(),
            line: t.line,
            col: t.col,
            found: t.tok.describe(),
            message: format!("expected {}", expected.into()),
        }
    }

    fn err_at(&self, at: At, found: impl Into<String>, message: impl Into<String>) -> ParseError {
        ParseError { file: self.file.to_string(), line: at.line, col: at.col, found: found.into(), message: message.into() }
    }

    fn is_punct(&self, c: u8) -> bool {
        *self.peek() == Tok::Punct(c)
    }

    fn eat_punct(&mut self, c: u8) -> bool {
        if self.is_punct(c) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Comma-separated items through the closing `close`, which may come
    /// right away.
    fn sep_list<T>(&mut self, close: u8, mut item: impl FnMut(&mut Self) -> R<T>) -> R<Vec<T>> {
        let mut out = Vec::new();
        if !self.is_punct(close) {
            loop {
                out.push(item(self)?);
                if !self.eat_punct(b',') {
                    break;
                }
            }
        }
        self.expect_punct(close)?;
        Ok(out)
    }

    fn expect_punct(&mut self, c: u8) -> R<()> {
        if self.eat_punct(c) { Ok(()) } else { Err(self.err_here(format!("`{}`", c as char))) }
    }

    fn is_ident(&self, word: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if *s == word)
    }

    fn eat_ident(&mut self, word: &str) -> bool {
        if self.is_ident(word) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_ident(&mut self, what: &str) -> R<&'a str> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            _ => Err(self.err_here(what.to_string())),
        }
    }

    fn expect_str(&mut self, what: &str) -> R<String> {
        match self.peek().clone() {
            Tok::Str(s) => {
                self.bump();
                Ok(s.into_owned())
            }
            _ => Err(self.err_here(what.to_string())),
        }
    }

    fn expect_uint(&mut self, what: &str) -> R<u32> {
        match self.peek().clone() {
            Tok::Int(n) if n >= 0 && n <= i64::from(u32::MAX) => {
                self.bump();
                Ok(n as u32)
            }
            _ => Err(self.err_here(what.to_string())),
        }
    }

    fn expect_usize(&mut self, what: &str) -> R<usize> {
        match self.peek().clone() {
            Tok::Int(n) if n >= 0 => {
                self.bump();
                Ok(n as usize)
            }
            _ => Err(self.err_here(what.to_string())),
        }
    }

    /// A port name: an identifier, or a string for one that is not.
    fn port_name(&mut self) -> R<&'a str> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            Tok::Str(Cow::Borrowed(s)) => {
                self.bump();
                Ok(s)
            }
            Tok::Str(Cow::Owned(s)) => {
                self.bump();
                Ok(resolve(intern(&s)))
            }
            _ => Err(self.err_here("a port name")),
        }
    }

    // ---------- file ----------

    fn file(&mut self) -> R<()> {
        loop {
            match self.peek().clone() {
                Tok::Ident("module") => break,
                Tok::Ident(key) => {
                    let at = self.at();
                    self.bump();
                    self.expect_punct(b'=')?;
                    self.directive(key, at)?;
                }
                Tok::MetaKw("source") => self.source_meta()?,
                Tok::MetaKw("comment") => self.comment_meta()?,
                _ => return Err(self.err_here("a directive (`name = value`) or `module`")),
            }
        }
        self.bump();
        let name = match self.bump() {
            Tok::ModName(n) => n.into_owned(),
            _ => {
                self.pos -= 1;
                return Err(self.err_here("a module name `@name`"));
            }
        };
        self.mods.push(PModule::new(Some(name), None));
        self.body(0)?;
        if *self.peek() != Tok::Eof {
            return Err(self.err_here("end of file after the root module"));
        }
        Ok(())
    }

    fn bool_value(&mut self) -> R<bool> {
        if self.eat_ident("true") {
            Ok(true)
        } else if self.eat_ident("false") {
            Ok(false)
        } else {
            Err(self.err_here("`true` or `false`"))
        }
    }

    fn directive(&mut self, key: &'a str, at: At) -> R<()> {
        match key {
            "source_filename" => self.dirs.source_file = Some(self.expect_str("a string")?),
            "layout" => {
                let found = match self.peek() {
                    Tok::Ident(w) => defaults::LAYOUT_MODES.iter().find(|(s, _)| s == w),
                    _ => None,
                };
                let Some((_, mode)) = found else { return Err(self.err_here("`dag`, `code` or `cube`")) };
                self.dirs.layout = *mode;
                self.bump();
            }
            "invisible" => self.dirs.invisible = self.bool_value()?,
            "no_gate_labels" => self.dirs.no_gate_labels = self.bool_value()?,
            "no_fold" => self.dirs.no_fold = self.bool_value()?,
            "doc" => self.dirs.doc = Some(self.expect_str("a string")?),
            "label" => self.dirs.label = Some(self.expect_str("a string")?),
            "label_from" => self.dirs.label_from = Some(self.port_ref()?),
            "color" => self.dirs.color = Some(PStyle::Baked(self.literal()?)),
            "color_from" => self.dirs.color = Some(PStyle::Wired(self.port_ref()?)),
            "material" => self.dirs.material = Some(PStyle::Baked(self.literal()?)),
            "material_from" => self.dirs.material = Some(PStyle::Wired(self.port_ref()?)),
            _ => {
                return Err(self.err_at(
                    at,
                    format!("`{key}`"),
                    "unknown directive; expected source_filename, layout, invisible, no_gate_labels, no_fold, doc, label, label_from, color, color_from, material or material_from",
                ));
            }
        }
        Ok(())
    }

    fn source_meta(&mut self) -> R<()> {
        self.bump();
        self.expect_punct(b'(')?;
        let file = self.expect_str("the source file name")?;
        self.expect_punct(b',')?;
        if !self.eat_ident("indent") {
            return Err(self.err_here("`indent:`"));
        }
        self.expect_punct(b':')?;
        self.expect_punct(b'[')?;
        let indent = self.sep_list(b']', |p| p.expect_uint("an indent column"))?;
        self.expect_punct(b')')?;
        self.dirs.source = Some((file, indent));
        Ok(())
    }

    fn comment_meta(&mut self) -> R<()> {
        self.bump();
        self.expect_punct(b'(')?;
        let file = self.expect_str("the source file name")?;
        if let Some((f, _)) = &self.dirs.source
            && *f != file
        {
            return Err(self.err_here(format!("the file named by `!source`, {f:?}")));
        }
        self.expect_punct(b':')?;
        let line = self.expect_uint("a line number")?;
        self.expect_punct(b':')?;
        let col = self.expect_uint("a column number")?;
        self.expect_punct(b',')?;
        let text = self.expect_str("the comment text")?;
        let mut c = SourceComment { line, col, text, own_line: false, in_array: false };
        while self.eat_punct(b',') {
            match self.expect_ident("`own_line` or `in_array`")? {
                "own_line" => c.own_line = true,
                "in_array" => c.in_array = true,
                _ => {
                    self.pos -= 1;
                    return Err(self.err_here("`own_line` or `in_array`"));
                }
            }
        }
        self.expect_punct(b')')?;
        self.dirs.comments.push(c);
        Ok(())
    }

    // ---------- modules ----------

    fn body(&mut self, mi: usize) -> R<()> {
        self.expect_punct(b'{')?;
        loop {
            match self.peek().clone() {
                Tok::Punct(b'}') => {
                    self.bump();
                    return Ok(());
                }
                Tok::Value(name) => {
                    let at = self.at();
                    self.bump();
                    self.expect_punct(b'=')?;
                    self.node(mi, name, at)?;
                }
                Tok::Ident("out") => {
                    self.bump();
                    let at = self.at();
                    let name = match self.bump() {
                        Tok::Value(n) => n,
                        _ => {
                            self.pos -= 1;
                            return Err(self.err_here("an output name `%name` after `out`"));
                        }
                    };
                    self.out_node(mi, name, at)?;
                }
                Tok::MetaNum(n) => {
                    self.bump();
                    self.expect_punct(b'=')?;
                    self.scope_def(mi, n)?;
                }
                Tok::MetaKw(kw @ ("inputs" | "outputs")) => {
                    self.bump();
                    let list = self.id_list()?;
                    let slot = if kw == "inputs" { &mut self.mods[mi].inputs } else { &mut self.mods[mi].outputs };
                    *slot = Some(list);
                }
                Tok::MetaKw("wire_order") => {
                    let at = self.at();
                    self.bump();
                    self.expect_punct(b'(')?;
                    let list = self.sep_list(b')', |p| p.expect_uint("a wire number"))?;
                    self.mods[mi].wire_order = Some((list, at));
                }
                Tok::Eof => return Err(self.err_here("`}` to close the module")),
                _ => return Err(self.err_here("a node (`%name = ...` or `out %name`), module metadata, or `}`")),
            }
        }
    }

    fn id_list(&mut self) -> R<Vec<IdRef<'a>>> {
        self.expect_punct(b'(')?;
        let list = self.sep_list(b')', |p| p.id_ref())?;
        Ok(list)
    }

    fn id_ref(&mut self) -> R<IdRef<'a>> {
        let at = self.at();
        match self.peek().clone() {
            Tok::Value(n) => {
                self.bump();
                Ok(IdRef::Name(n, at))
            }
            Tok::Int(n) if n >= 0 && n <= i64::from(u32::MAX) => {
                self.bump();
                Ok(IdRef::Id(n as u32))
            }
            _ => Err(self.err_here("a node `%name` or a node id")),
        }
    }

    fn scope_def(&mut self, mi: usize, n: u32) -> R<()> {
        if !matches!(self.peek(), Tok::MetaKw("scope")) {
            return Err(self.err_here("`!scope(...)`"));
        }
        self.bump();
        self.expect_punct(b'(')?;
        let word = self.expect_ident("a scope kind")?;
        let Some(&(_, named, make)) = defaults::SCOPE_KINDS.iter().find(|(w, _, _)| *w == word) else {
            self.pos -= 1;
            return Err(self.err_here(
                "a scope kind: ModuleRoot, ChipBody, HandlerBody, IfGroup, IfCond, IfThen, IfElse or Block",
            ));
        };
        let kind = make(if named { self.expect_str("the scope's name")? } else { String::new() });
        let mut info = ScopeInfo { kind, source_range: SourceRange::default(), parent: None };
        while self.eat_punct(b',') {
            if self.eat_ident("parent") {
                self.expect_punct(b':')?;
                match self.bump() {
                    Tok::MetaNum(p) => info.parent = Some(p),
                    _ => {
                        self.pos -= 1;
                        return Err(self.err_here("a scope number `!N`"));
                    }
                }
            } else if self.is_ident("loc") {
                info.source_range = self.loc()?;
            } else {
                return Err(self.err_here("`parent: !N` or `loc(...)`"));
            }
        }
        self.expect_punct(b')')?;
        self.mods[mi].scopes.push((n, info));
        Ok(())
    }

    // ---------- nodes ----------

    fn new_node(&mut self, mi: usize, name: &'a str, at: At, form: Form, class: &'static str) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(PNode {
            name,
            at,
            module: mi,
            form,
            class,
            pin_type: None,
            operands: Vec::new(),
            attrs: Vec::new(),
            sig: None,
            id_meta: None,
            kind_meta: None,
            scope: ROOT_SCOPE_ID,
            chain: None,
            chip: None,
            label: None,
            color: None,
            material: None,
            layout_after: Vec::new(),
            note: None,
            loc: SourceRange::default(),
            child: None,
        });
        self.mods[mi].nodes.push(idx);
        idx
    }

    fn node(&mut self, mi: usize, name: &'a str, at: At) -> R<()> {
        match self.peek().clone() {
            Tok::Ident("in") => {
                self.bump();
                let class = self.optional_class().unwrap_or(defaults::INPUT_CLASS);
                let idx = self.new_node(mi, name, at, Form::In, class);
                self.nodes[idx].pin_type = self.optional_pin_type()?;
                if self.is_punct(b'(') {
                    self.bump();
                    let at = self.at();
                    let port = if matches!(self.peek_at(1), Tok::Punct(b':')) {
                        let p = self.port_name()?;
                        self.expect_punct(b':')?;
                        p
                    } else {
                        WirePort::RerInput.as_str()
                    };
                    let op = self.operand()?;
                    self.nodes[idx].operands.push((port, op, at));
                    self.expect_punct(b')')?;
                }
                self.tail(idx)
            }
            Tok::Ident("chip") => {
                self.bump();
                let class = self.optional_class().unwrap_or(defaults::CHIP_CLASS);
                let idx = self.new_node(mi, name, at, Form::Chip, class);
                let child_name = match self.peek().clone() {
                    Tok::ModName(n) => {
                        self.bump();
                        Some(n.into_owned())
                    }
                    _ => None,
                };
                if self.is_punct(b'(') {
                    self.operands(idx)?;
                }
                if self.is_punct(b'{') && self.brace_is_attrs() {
                    self.attrs(idx)?;
                }
                if self.is_punct(b':') {
                    self.nodes[idx].sig = Some(self.sig()?);
                }
                self.metas(idx)?;
                let cm = self.tree.push(mi);
                self.mods.push(PModule::new(child_name, Some(idx)));
                self.nodes[idx].child = Some(cm);
                self.body(cm)?;
                if self.is_ident("loc") {
                    self.nodes[idx].loc = self.loc()?;
                }
                Ok(())
            }
            Tok::Ident(c) => {
                self.bump();
                let class = defaults::resolve_class(c);
                let idx = self.new_node(mi, name, at, Form::Gate, class);
                if self.is_punct(b'(') {
                    self.operands(idx)?;
                }
                self.tail(idx)
            }
            _ => Err(self.err_here("`in`, `chip`, or a gate class")),
        }
    }

    fn out_node(&mut self, mi: usize, name: &'a str, at: At) -> R<()> {
        let class = self.optional_class().unwrap_or(defaults::OUTPUT_CLASS);
        let idx = self.new_node(mi, name, at, Form::Out, class);
        self.nodes[idx].pin_type = self.optional_pin_type()?;
        if self.eat_punct(b'=') {
            let at = self.at();
            let op = self.operand()?;
            self.nodes[idx].operands.push((WirePort::RerInput.as_str(), op, at));
        }
        self.tail(idx)
    }

    /// A class name where one may stand before a pin's type: an identifier
    /// that is neither a type word nor a word of the grammar (`out` starting
    /// the next line, `loc`).
    fn optional_class(&mut self) -> Option<&'static str> {
        match self.peek() {
            Tok::Ident(s) if !defaults::is_type_word(s) && !defaults::RESERVED.contains(s) => {
                let c = defaults::resolve_class(s);
                self.bump();
                Some(c)
            }
            _ => None,
        }
    }

    /// The type of an `in`/`out`, if one is written. A `{` starts a record
    /// type when its first entry is `name:`, or when it is the empty `{ }`
    /// (empty attributes are written as no braces at all); otherwise it is
    /// attributes.
    fn optional_pin_type(&mut self) -> R<Option<Type>> {
        let starts_type = match self.peek() {
            Tok::Ident(s) => defaults::is_type_word(s),
            Tok::Punct(b'*') => true,
            Tok::Punct(b'{') => match self.peek_at(1) {
                Tok::Punct(b'}') => true,
                Tok::Ident(_) | Tok::Str(_) => matches!(self.peek_at(2), Tok::Punct(b':')),
                _ => false,
            },
            _ => false,
        };
        if starts_type { Ok(Some(self.ty()?)) } else { Ok(None) }
    }

    /// Whether the `{` under the cursor opens attributes rather than a chip
    /// body: a body starts with a node, `out`, or module metadata, and a lone
    /// `{ }` is a body.
    fn brace_is_attrs(&self) -> bool {
        match self.peek_at(1) {
            Tok::Value(_) | Tok::MetaNum(_) | Tok::Punct(b'}') => false,
            Tok::Ident("out") => !matches!(self.peek_at(2), Tok::Punct(b'=' | b',' | b'}')),
            Tok::MetaKw(_) => false,
            _ => true,
        }
    }

    fn operands(&mut self, idx: usize) -> R<()> {
        self.expect_punct(b'(')?;
        let ops = self.sep_list(b')', |p| {
            let at = p.at();
            let port = p.port_name()?;
            p.expect_punct(b':')?;
            Ok((port, p.operand()?, at))
        })?;
        self.nodes[idx].operands.extend(ops);
        Ok(())
    }

    fn operand(&mut self) -> R<POperand<'a>> {
        if matches!(self.peek(), Tok::Value(_)) {
            Ok(POperand::Ref(self.port_ref()?))
        } else {
            Ok(POperand::Lit(self.literal()?))
        }
    }

    fn port_ref(&mut self) -> R<PRef<'a>> {
        let at = self.at();
        let name = match self.bump() {
            Tok::Value(n) => n,
            _ => {
                self.pos -= 1;
                return Err(self.err_here("a value `%name`"));
            }
        };
        let port = if self.eat_punct(b'.') { Some(self.port_name()?) } else { None };
        Ok(PRef { name, port, at })
    }

    fn tail(&mut self, idx: usize) -> R<()> {
        if self.is_punct(b'{') {
            self.attrs(idx)?;
        }
        if self.is_punct(b':') {
            self.nodes[idx].sig = Some(self.sig()?);
        }
        self.metas(idx)?;
        if self.is_ident("loc") {
            self.nodes[idx].loc = self.loc()?;
        }
        Ok(())
    }

    fn attrs(&mut self, idx: usize) -> R<()> {
        self.expect_punct(b'{')?;
        let attrs = self.sep_list(b'}', |p| {
            let at = p.at();
            let (key, quoted) = match p.bump() {
                Tok::Ident(k) => (Cow::Borrowed(k), false),
                Tok::Str(k) => (k, true),
                _ => {
                    p.pos -= 1;
                    return Err(p.err_here("an attribute key"));
                }
            };
            let val = if p.eat_punct(b'=') {
                let nodes = !quoted && defaults::annotation_by_key(&key).is_some_and(|(_, kind)| kind == AnnValue::Nodes);
                match p.peek().clone() {
                    _ if nodes => p.node_refs()?,
                    Tok::Value(_) => PAttrVal::Ref(p.port_ref()?),
                    Tok::Ident(w) if !defaults::is_literal_word(w) => {
                        p.bump();
                        PAttrVal::Word(w)
                    }
                    _ => PAttrVal::Lit(p.literal()?),
                }
            } else {
                PAttrVal::Flag
            };
            Ok(PAttr { key, quoted, val, at })
        })?;
        self.nodes[idx].attrs.extend(attrs);
        Ok(())
    }

    /// The value of an [`AnnValue::Nodes`] attribute: `[%a, %b]` or
    /// `map[k => %a]`.
    fn node_refs(&mut self) -> R<PAttrVal<'a>> {
        if self.is_ident(word::MAP) {
            self.bump();
            self.expect_punct(b'[')?;
            let entries = self.sep_list(b']', |p| {
                let k = p.literal()?;
                if *p.peek() != Tok::FatArrow {
                    return Err(p.err_here("`=>` between a map key and its value"));
                }
                p.bump();
                Ok((k, p.port_ref()?))
            })?;
            return Ok(PAttrVal::RefMap(entries));
        }
        self.expect_punct(b'[')?;
        Ok(PAttrVal::Refs(self.sep_list(b']', |p| p.port_ref())?))
    }

    fn sig(&mut self) -> R<GateIO> {
        self.expect_punct(b':')?;
        let inputs = self.port_specs()?;
        if *self.peek() != Tok::Arrow {
            return Err(self.err_here("`->` between input and output ports"));
        }
        self.bump();
        let outputs = self.port_specs()?;
        Ok(GateIO { inputs, outputs })
    }

    fn port_specs(&mut self) -> R<Vec<PortSpec>> {
        self.expect_punct(b'(')?;
        let out = self.sep_list(b')', |p| {
            let name = p.port_name()?;
            p.expect_punct(b':')?;
            let t = p.ty()?;
            Ok(PortSpec::new(intern(name), t))
        })?;
        Ok(out)
    }

    fn metas(&mut self, idx: usize) -> R<()> {
        while let Tok::MetaKw(kw) = self.peek().clone() {
            if !NODE_META.contains(&kw) {
                break;
            }
            self.bump();
            match kw {
                "id" => self.nodes[idx].id_meta = Some(self.expect_uint("a node id")?),
                "scope" => match self.bump() {
                    Tok::MetaNum(n) => self.nodes[idx].scope = n,
                    _ => {
                        self.pos -= 1;
                        return Err(self.err_here("a scope number `!N`"));
                    }
                },
                "chain" => self.nodes[idx].chain = Some(self.expect_uint("a chain id")?),
                "chip" => self.nodes[idx].chip = Some(self.id_ref()?),
                "label" => {
                    self.expect_punct(b'(')?;
                    self.nodes[idx].label = Some(self.port_ref()?);
                    self.expect_punct(b')')?;
                }
                "color" => {
                    self.expect_punct(b'(')?;
                    self.nodes[idx].color = Some(self.port_ref()?);
                    self.expect_punct(b')')?;
                }
                "material" => {
                    self.expect_punct(b'(')?;
                    self.nodes[idx].material = Some(self.port_ref()?);
                    self.expect_punct(b')')?;
                }
                "layout_after" => {
                    let list = self.id_list()?;
                    self.nodes[idx].layout_after.extend(list);
                }
                "kind" => {
                    let word = self.expect_ident("a node kind")?;
                    let Some(&(_, kind)) = defaults::NODE_KINDS.iter().find(|(w, _)| *w == word) else {
                        self.pos -= 1;
                        return Err(self.err_here("a node kind: gate, event, chip, input, output or coerce"));
                    };
                    self.nodes[idx].kind_meta = Some(kind);
                }
                "note" => {
                    let s = self.expect_str("the note text")?;
                    self.nodes[idx].note = Some(resolve(intern(&s)));
                }
                _ => unreachable!("NODE_META lists every key matched here"),
            }
        }
        Ok(())
    }

    fn loc(&mut self) -> R<SourceRange> {
        self.bump();
        self.expect_punct(b'(')?;
        // `loc(line:col ...)` is in the entry file, `source_filename`.
        let file = match self.peek().clone() {
            Tok::Str(s) => {
                self.bump();
                self.expect_punct(b':')?;
                self.file_arc(&s)
            }
            Tok::Int(_) => match self.dirs.source_file.clone() {
                Some(f) => self.file_arc(&f),
                None => return Err(self.err_here("a quoted file name; `loc(line:col)` needs a `source_filename` directive")),
            },
            _ => return Err(self.err_here("a quoted file name or `line:col`")),
        };
        let start = self.pos_value()?;
        let end = if self.eat_ident("to") { self.pos_value()? } else { start };
        self.expect_punct(b')')?;
        Ok(SourceRange { file, start, end })
    }

    /// One shared `Arc<str>` per distinct file name.
    fn file_arc(&mut self, name: &str) -> Arc<str> {
        if let Some(a) = self.files.get(name) {
            return a.clone();
        }
        let a: Arc<str> = Arc::from(name);
        self.files.insert(name.to_string(), a.clone());
        a
    }

    fn pos_value(&mut self) -> R<Pos> {
        let line = self.expect_uint("a line number")?;
        self.expect_punct(b':')?;
        let col = self.expect_uint("a column number")?;
        let offset = if self.eat_punct(b'+') { self.expect_usize("a byte offset")? } else { 0 };
        Ok(Pos { offset, line, col })
    }

    // ---------- literals and types ----------

    fn number(&mut self) -> R<f64> {
        match self.bump() {
            Tok::Float(f) => Ok(f),
            Tok::Int(n) => Ok(n as f64),
            Tok::Ident(word::INF) => Ok(f64::INFINITY),
            Tok::Ident(word::NAN) => Ok(f64::NAN),
            _ => {
                self.pos -= 1;
                Err(self.err_here("a number"))
            }
        }
    }

    fn numbers<const N: usize>(&mut self) -> R<[f64; N]> {
        self.expect_punct(b'(')?;
        let mut out = [0.0; N];
        for (i, slot) in out.iter_mut().enumerate() {
            if i > 0 {
                self.expect_punct(b',')?;
            }
            *slot = self.number()?;
        }
        self.expect_punct(b')')?;
        Ok(out)
    }

    fn literal(&mut self) -> R<Literal> {
        let t = self.peek().clone();
        match t {
            Tok::Int(n) => {
                self.bump();
                Ok(Literal::Int(n))
            }
            Tok::Float(f) => {
                self.bump();
                Ok(Literal::Float(f))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Literal::String(s.into_owned()))
            }
            Tok::Color([r, g, b, a]) => {
                self.bump();
                Ok(Literal::Color { r, g, b, a })
            }
            Tok::Punct(b'[') => {
                self.bump();
                let items = self.sep_list(b']', |p| p.literal())?;
                Ok(Literal::Array(items))
            }
            Tok::Ident(w) => {
                self.bump();
                match w {
                    word::TRUE => Ok(Literal::Bool(true)),
                    word::FALSE => Ok(Literal::Bool(false)),
                    word::NULL => Ok(Literal::Object),
                    word::INF => Ok(Literal::Float(f64::INFINITY)),
                    word::NAN => Ok(Literal::Float(f64::NAN)),
                    word::VEC => {
                        let [x, y, z] = self.numbers()?;
                        Ok(Literal::Vector { x, y, z })
                    }
                    word::ROT => {
                        let [pitch, yaw, roll] = self.numbers()?;
                        Ok(Literal::Rotator { pitch, yaw, roll })
                    }
                    word::QUAT => {
                        let [x, y, z, w] = self.numbers()?;
                        Ok(Literal::Quat { x, y, z, w })
                    }
                    word::LRGBA => {
                        let [r, g, b, a] = self.numbers()?;
                        Ok(Literal::LinearColor { r, g, b, a })
                    }
                    word::MAP => {
                        self.expect_punct(b'[')?;
                        let entries = self.sep_list(b']', |p| {
                            let k = p.literal()?;
                            if *p.peek() != Tok::FatArrow {
                                return Err(p.err_here("`=>` between a map key and its value"));
                            }
                            p.bump();
                            let v = p.literal()?;
                            Ok((k, v))
                        })?;
                        Ok(Literal::Map(entries))
                    }
                    word::REC => {
                        self.expect_punct(b'{')?;
                        let fields = self.sep_list(b'}', |p| {
                            let k = match p.bump() {
                                Tok::Ident(k) => k.to_string(),
                                Tok::Str(k) => k.into_owned(),
                                _ => {
                                    p.pos -= 1;
                                    return Err(p.err_here("a record field name"));
                                }
                            };
                            p.expect_punct(b':')?;
                            Ok((k, p.literal()?))
                        })?;
                        Ok(Literal::Record(fields))
                    }
                    word::ASSET => {
                        self.expect_punct(b'(')?;
                        let asset_type = self.expect_str("the asset type")?;
                        self.expect_punct(b',')?;
                        let asset_name = self.expect_str("the asset name")?;
                        self.expect_punct(b')')?;
                        Ok(Literal::Asset { asset_type, asset_name })
                    }
                    word::PREFAB | word::NESTED_PREFAB | word::PREFAB_BYTES => {
                        self.expect_punct(b'(')?;
                        let at = self.at();
                        let s = self.expect_str("a string")?;
                        self.expect_punct(b')')?;
                        Ok(match w {
                            word::PREFAB => Literal::PrefabRef { path: s },
                            word::NESTED_PREFAB => Literal::NestedPrefab { source: s },
                            _ => Literal::PrefabBytes(
                                super::base64::decode(&s).map_err(|e| self.err_at(at, "the prefab bytes", e))?.into(),
                            ),
                        })
                    }
                    _ => {
                        self.pos -= 1;
                        Err(self.err_here("a literal"))
                    }
                }
            }
            _ => Err(self.err_here("a literal or a value `%name`")),
        }
    }

    fn ty(&mut self) -> R<Type> {
        let base = self.ty_base()?;
        let mut t = base;
        while self.is_punct(b'[') && matches!(self.peek_at(1), Tok::Punct(b']')) {
            self.bump();
            self.bump();
            t = Type::Array(Box::new(t));
        }
        Ok(t)
    }

    fn ty_args(&mut self) -> R<Vec<Type>> {
        self.expect_punct(b'<')?;
        let out = self.sep_list(b'>', |p| p.ty())?;
        Ok(out)
    }

    fn fields(&mut self) -> R<Vec<(String, Type)>> {
        self.expect_punct(b'{')?;
        let out = self.sep_list(b'}', |p| {
            let k = match p.bump() {
                Tok::Ident(k) => k.to_string(),
                Tok::Str(k) => k.into_owned(),
                _ => {
                    p.pos -= 1;
                    return Err(p.err_here("a field name"));
                }
            };
            p.expect_punct(b':')?;
            Ok((k, p.ty()?))
        })?;
        Ok(out)
    }

    fn str_arg(&mut self) -> R<String> {
        self.expect_punct(b'<')?;
        let s = self.expect_str("a quoted name")?;
        Ok(s)
    }

    fn ty_base(&mut self) -> R<Type> {
        if self.eat_punct(b'*') {
            return Ok(Type::Ref(Box::new(self.ty()?)));
        }
        if self.is_punct(b'{') {
            return Ok(Type::Record(self.fields()?));
        }
        let word = match self.peek().clone() {
            Tok::Ident(w) => w,
            _ => return Err(self.err_here("a type")),
        };
        self.bump();
        if let Some((_, t)) = defaults::SCALAR_TYPES.iter().find(|(w, _)| *w == word) {
            return Ok(t.clone());
        }
        Ok(match word {
            "map" => {
                let a = self.ty_args()?;
                let [k, v]: [Type; 2] = a.try_into().map_err(|_| self.err_here("`map<K, V>` with two types"))?;
                Type::Map(Box::new(k), Box::new(v))
            }
            "array" => {
                let a = self.ty_args()?;
                let [t]: [Type; 1] = a.try_into().map_err(|_| self.err_here("`array<T>` with one type"))?;
                Type::Array(Box::new(t))
            }
            "union" => Type::Union(self.ty_args()?),
            "tuple" => Type::Tuple(self.ty_args()?),
            "outputs" => Type::Outputs(self.fields()?),
            "param" => {
                let s = self.str_arg()?;
                self.expect_punct(b'>')?;
                Type::Param(s)
            }
            "component" => {
                let s = self.str_arg()?;
                self.expect_punct(b'>')?;
                Type::Component(s)
            }
            "enum" => {
                let name = self.str_arg()?;
                let mut args = Vec::new();
                while self.eat_punct(b',') {
                    args.push(self.ty()?);
                }
                self.expect_punct(b'>')?;
                Type::Enum { name, args }
            }
            _ => {
                self.pos -= 1;
                return Err(self.err_here("a type"));
            }
        })
    }

    // ---------- resolution ----------

    fn resolve(mut self) -> R<Program> {
        let dirs = std::mem::take(&mut self.dirs);
        let file = self.file;
        // Name table.
        let mut by_name: HashMap<&'a str, usize> = HashMap::default();
        for (i, n) in self.nodes.iter().enumerate() {
            if by_name.insert(n.name, i).is_some() {
                return Err(self.err_at(n.at, format!("`%{}`", n.name), "a value of this name is already defined"));
            }
        }

        // Ids.
        let mut ids: Vec<NodeId> = Vec::with_capacity(self.nodes.len());
        let mut prev: u32 = 0;
        let mut seen: HashMap<NodeId, usize> = HashMap::default();
        for (i, n) in self.nodes.iter().enumerate() {
            let id = if n.name.bytes().all(|b| b.is_ascii_digit()) {
                n.name
                    .parse::<u32>()
                    .map_err(|_| self.err_at(n.at, format!("`%{}`", n.name), "node id out of range"))?
            } else if let Some(id) = n.id_meta {
                id
            } else {
                prev.checked_add(1).ok_or_else(|| self.err_at(n.at, format!("`%{}`", n.name), "node id out of range"))?
            };
            if let Some(&other) = seen.get(&NodeId(id)) {
                return Err(self.err_at(
                    n.at,
                    format!("`%{}`", n.name),
                    format!("node id {id} is already taken by `%{}`", self.nodes[other].name),
                ));
            }
            seen.insert(NodeId(id), i);
            ids.push(NodeId(id));
            prev = id;
        }

        let lookup = |name: &str, at: At| -> R<usize> {
            by_name.get(name).copied().ok_or_else(|| ParseError {
                file: file.to_string(),
                line: at.line,
                col: at.col,
                found: format!("`%{name}`"),
                message: "this value is never defined".into(),
            })
        };
        let id_of = |r: &IdRef<'a>| -> R<NodeId> {
            match r {
                IdRef::Name(n, at) => Ok(ids[lookup(n, *at)?]),
                IdRef::Id(n) => Ok(NodeId(*n)),
            }
        };

        // Nodes, without pin types still to infer.
        let mut built: Vec<Node> = Vec::with_capacity(self.nodes.len());
        for (i, n) in self.nodes.iter().enumerate() {
            let mut props: HashMap<Sym, Literal> = HashMap::default();
            let mut port_label_given = false;
            let mut nolabel = false;
            for a in &n.attrs {
                let ann = if a.quoted { None } else { defaults::annotation_by_key(&a.key) };
                let bad = |what: &str| self.err_at(a.at, format!("`{}`", a.key), what.to_string());
                match (ann, &a.val) {
                    (None, PAttrVal::Flag) if !a.quoted && a.key == defaults::NO_LABEL => nolabel = true,
                    (None, PAttrVal::Flag) => return Err(bad("expected `=` and a value after this attribute key")),
                    (None, PAttrVal::Lit(l)) => {
                        let key = intern(&a.key);
                        if key == *sym::PORT_LABEL {
                            port_label_given = true;
                        }
                        props.insert(key, l.clone());
                    }
                    (None, PAttrVal::Word(w)) => {
                        return Err(self.err_at(a.at, format!("`{w}`"), "expected a literal value; quote a string"));
                    }
                    (None, PAttrVal::Ref(_) | PAttrVal::Refs(_) | PAttrVal::RefMap(_)) => {
                        return Err(bad("only `handle_target` and `handle_targets` take values `%name`; others take a literal"));
                    }
                    (Some((prop, kind)), val) => {
                        let lit = match (kind, val) {
                            (AnnValue::Flag, PAttrVal::Flag) => Literal::Bool(true),
                            (AnnValue::Str, PAttrVal::Lit(Literal::String(s))) => Literal::String(s.clone()),
                            (AnnValue::Word, PAttrVal::Word(w)) => Literal::String(w.to_string()),
                            (AnnValue::Word, PAttrVal::Lit(Literal::String(s))) => Literal::String(s.clone()),
                            (AnnValue::Node, PAttrVal::Ref(r)) => Literal::Int(i64::from(ids[lookup(r.name, r.at)?].0)),
                            (AnnValue::Nodes, PAttrVal::Refs(rs)) => Literal::Array(
                                rs.iter().map(|r| Ok(Literal::Int(i64::from(ids[lookup(r.name, r.at)?].0)))).collect::<R<_>>()?,
                            ),
                            (AnnValue::Nodes, PAttrVal::RefMap(es)) => Literal::Map(
                                es.iter()
                                    .map(|(k, r)| Ok((k.clone(), Literal::Int(i64::from(ids[lookup(r.name, r.at)?].0)))))
                                    .collect::<R<_>>()?,
                            ),
                            (_, PAttrVal::Lit(l)) => l.clone(),
                            (AnnValue::Flag, _) => return Err(bad("a flag; write it with no value")),
                            (AnnValue::Str, _) => return Err(bad("expected a string value")),
                            (AnnValue::Word, _) => return Err(bad("expected `left`, `right`, `top` or `bottom`")),
                            (AnnValue::Node, _) => return Err(bad("expected a value `%name`")),
                            (AnnValue::Nodes, _) => return Err(bad("expected `[%a, ...]` or `map[k => %a, ...]`")),
                        };
                        props.insert(intern(prop), lit);
                    }
                }
            }
            for (port, op, _) in &n.operands {
                if let POperand::Lit(l) = op {
                    props.insert(intern(port), l.clone());
                }
            }
            if matches!(n.form, Form::In | Form::Out) && !port_label_given && !nolabel {
                props.insert(*sym::PORT_LABEL, Literal::String(n.name.to_string()));
            }
            let kind = n.kind_meta.unwrap_or(match n.form {
                Form::In => NodeKind::Input,
                Form::Out => NodeKind::Output,
                Form::Chip => NodeKind::Chip,
                Form::Gate => defaults::default_kind(n.class),
            });
            let ports = match (&n.sig, n.form) {
                (Some(s), _) => s.clone(),
                (None, Form::In | Form::Out) => GateIO::pin(n.pin_type.clone().unwrap_or(Type::Any)),
                (None, _) => defaults::default_ports(n.class, props.get(&*sym::VALUE)),
            };
            let chip_id = match &n.chip {
                Some(r) => Some(id_of(r)?),
                None => None,
            };
            built.push(Node {
                id: ids[i],
                kind,
                gate_class: n.class,
                properties: Arc::new(props),
                ports: Arc::new(ports),
                source_range: n.loc.clone(),
                chip_id,
                chain_id: n.chain,
                scope_id: n.scope,
                note: n.note,
            });
        }

        // Pins written without a type take the type of what feeds them,
        // pass after pass until none changes, so a chain resolves whatever
        // order it is written in. A pin fed by another one still unresolved
        // waits for it.
        let mut pending: HashSet<usize> = (0..self.nodes.len())
            .filter(|&i| {
                let n = &self.nodes[i];
                matches!(n.form, Form::In | Form::Out) && n.pin_type.is_none() && n.sig.is_none()
            })
            .collect();
        loop {
            let before = pending.len();
            let mut round: Vec<usize> = pending.iter().copied().collect();
            round.sort_unstable();
            for i in round {
                let src = self.nodes[i].operands.iter().find_map(|(_, op, _)| match op {
                    POperand::Ref(r) => Some(r),
                    POperand::Lit(_) => None,
                });
                let Some(r) = src else {
                    pending.remove(&i);
                    continue;
                };
                let si = lookup(r.name, r.at)?;
                match defaults::output_type(&built[si].ports, r.port) {
                    Some(t) if t != Type::Any || !pending.contains(&si) => {
                        built[i].ports = Arc::new(GateIO::pin(t));
                        pending.remove(&i);
                    }
                    _ => {}
                }
            }
            if pending.len() == before {
                break;
            }
        }

        // A gate written without a signature gives an `any` input port the
        // type of the wire feeding it. After the pins, so a pin written
        // without a type has its own by now.
        for (i, n) in self.nodes.iter().enumerate() {
            if n.sig.is_some() || matches!(n.form, Form::In | Form::Out) {
                continue;
            }
            if !built[i].ports.inputs.iter().any(|p| matches!(p.ty, Type::Any)) {
                continue;
            }
            let mut ports: Option<GateIO> = None;
            for (port, op, _) in &n.operands {
                let POperand::Ref(r) = op else { continue };
                let Some(&si) = by_name.get(r.name) else { continue };
                let Some(t) = defaults::output_type(&built[si].ports, r.port) else { continue };
                let current = ports.get_or_insert_with(|| (*built[i].ports).clone());
                defaults::infer_any_input(current, intern(port), t);
            }
            if let Some(p) = ports.filter(|p| *p != *built[i].ports) {
                built[i].ports = Arc::new(p);
            }
        }

        // Wires, numbered in file order.
        let mut wires: Vec<Wire> = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            for (port, op, at) in &n.operands {
                let POperand::Ref(r) = op else { continue };
                let target_port = self.input_port(&built[i], n, port, *at)?;
                let si = lookup(r.name, r.at)?;
                let source_port = self.output_port(&built[si], &self.nodes[si], r)?;
                wires.push(Wire {
                    source: PortRef { node_id: ids[si], port: source_port },
                    target: PortRef { node_id: ids[i], port: target_port },
                });
            }
            for r in &n.layout_after {
                let src = id_of(r)?;
                wires.push(Wire {
                    source: PortRef { node_id: src, port: WirePort::Layout },
                    target: PortRef { node_id: ids[i], port: WirePort::Layout },
                });
            }
        }

        // Which module stores each wire.
        let module_of_id: HashMap<NodeId, usize> = self.nodes.iter().enumerate().map(|(i, n)| (ids[i], n.module)).collect();
        let mut lists: Vec<Vec<usize>> = vec![Vec::new(); self.mods.len()];
        let mut claimed = vec![false; wires.len()];
        for (mi, m) in self.mods.iter().enumerate() {
            if let Some((list, at)) = &m.wire_order {
                for &w in list {
                    let w = w as usize;
                    if w >= wires.len() {
                        return Err(self.err_at(
                            *at,
                            format!("`{w}`"),
                            format!("`!wire_order` names wire {w}, but the file has {} wires", wires.len()),
                        ));
                    }
                    if std::mem::replace(&mut claimed[w], true) {
                        return Err(self.err_at(*at, format!("`{w}`"), "`!wire_order` lists this wire twice"));
                    }
                    lists[mi].push(w);
                }
            }
        }
        for (w, wire) in wires.iter().enumerate() {
            if claimed[w] {
                continue;
            }
            let a = module_of_id.get(&wire.source.node_id).copied();
            let b = module_of_id.get(&wire.target.node_id).copied();
            let home = match (a, b) {
                (Some(a), Some(b)) => self.tree.common_module(a, b),
                (None, Some(b)) => b,
                _ => 0,
            };
            lists[home].push(w);
        }

        let mut modules: Vec<Module> = Vec::with_capacity(self.mods.len());
        for (mi, pm) in self.mods.iter().enumerate() {
            let name = match (&pm.name, pm.chip_node) {
                (Some(n), _) => n.clone(),
                (None, Some(ci)) => defaults::default_chip_module_name(ids[ci].0),
                (None, None) => String::new(),
            };
            let mut m = Module::new(&name);
            for (sid, info) in &pm.scopes {
                m.scopes.insert(*sid, info.clone());
            }
            for &ni in &pm.nodes {
                m.nodes.insert(ids[ni], built[ni].clone());
                let name = self.nodes[ni].name;
                if !name.bytes().all(|b| b.is_ascii_digit()) {
                    m.value_names.insert(ids[ni], intern(name));
                }
                if let Some(r) = &self.nodes[ni].label {
                    let si = lookup(r.name, r.at)?;
                    let port = self.output_port(&built[si], &self.nodes[si], r)?;
                    m.dynamic_labels.insert(ids[ni], PortRef { node_id: ids[si], port });
                }
                if let Some(r) = &self.nodes[ni].color {
                    let si = lookup(r.name, r.at)?;
                    let port = self.output_port(&built[si], &self.nodes[si], r)?;
                    m.dynamic_colors.insert(ids[ni], PortRef { node_id: ids[si], port });
                }
                if let Some(r) = &self.nodes[ni].material {
                    let si = lookup(r.name, r.at)?;
                    let port = self.output_port(&built[si], &self.nodes[si], r)?;
                    m.dynamic_materials.insert(ids[ni], PortRef { node_id: ids[si], port });
                }
            }
            m.wires = lists[mi].iter().map(|&w| wires[w]).collect();
            let ordered = |kind: NodeKind| -> Vec<NodeId> {
                let mut v: Vec<NodeId> = pm.nodes.iter().filter(|&&ni| built[ni].kind == kind).map(|&ni| ids[ni]).collect();
                v.sort();
                v
            };
            m.inputs = match &pm.inputs {
                Some(list) => list.iter().map(id_of).collect::<R<_>>()?,
                None => ordered(NodeKind::Input),
            };
            m.outputs = match &pm.outputs {
                Some(list) => list.iter().map(id_of).collect::<R<_>>()?,
                None => ordered(NodeKind::Output),
            };
            if mi == 0 {
                m.root_label_override = dirs.label.clone();
                if let Some(r) = &dirs.label_from {
                    let si = lookup(r.name, r.at)?;
                    let port = self.output_port(&built[si], &self.nodes[si], r)?;
                    m.root_dynamic_label = Some(PortRef { node_id: ids[si], port });
                }
                let style = |s: &Option<PStyle<'a>>| -> R<Option<StyleValue>> {
                    Ok(match s {
                        Some(PStyle::Baked(lit)) => Some(StyleValue::Baked(lit.clone())),
                        Some(PStyle::Wired(r)) => {
                            let si = lookup(r.name, r.at)?;
                            let port = self.output_port(&built[si], &self.nodes[si], r)?;
                            Some(StyleValue::Wired(PortRef { node_id: ids[si], port }))
                        }
                        None => None,
                    })
                };
                m.root_style.color = style(&dirs.color)?;
                m.root_style.material = style(&dirs.material)?;
            }
            modules.push(m);
        }
        let bodies = (1..self.mods.len())
            .map(|mi| {
                let chip = self.mods[mi].chip_node.expect("only the root has no chip");
                (ids[chip], mi, self.tree.parent(mi).expect("only the root has no parent"))
            })
            .collect();
        let module = nest_bodies(modules, bodies);

        let settings = ProgramSettings {
            source_file: dirs.source_file,
            layout: dirs.layout,
            invisible: dirs.invisible,
            no_gate_labels: dirs.no_gate_labels,
            no_fold: dirs.no_fold,
            module_doc: dirs.doc,
            source_map: dirs.source.map(|(file, line_indent)| {
                Arc::new(SourceMap { file: Arc::from(file), line_indent, comments: dirs.comments })
            }),
        };
        Ok(Program { module, settings })
    }

    /// The input port `port` of `node`, which must be one of its ports.
    fn input_port(&self, node: &Node, pn: &PNode<'a>, port: &str, at: At) -> R<WirePort> {
        let listed = node.ports.inputs.iter().any(|p| resolve(p.name) == port);
        match WirePort::try_from_name(port) {
            Some(wp) if listed => Ok(wp),
            None if listed => Err(self.err_at(
                at,
                format!("`{port}`"),
                format!("`{port}` is declared on `%{}`, but no inventory gate has a wire port of that name", pn.name),
            )),
            _ => {
                let valid: Vec<&str> = node.ports.inputs.iter().map(|p| resolve(p.name)).collect();
                Err(self.err_at(
                    at,
                    format!("`{port}`"),
                    format!(
                        "`%{}` ({}) has no input port `{port}`; its input ports are: {}",
                        pn.name,
                        defaults::class_spelling(pn.class),
                        if valid.is_empty() { "none".to_string() } else { valid.join(", ") }
                    ),
                ))
            }
        }
    }

    /// The output port a reference names: its `.Port`, or the node's only
    /// output when it has exactly one.
    fn output_port(&self, node: &Node, pn: &PNode<'a>, r: &PRef<'a>) -> R<WirePort> {
        let outs: Vec<&str> = node.ports.outputs.iter().map(|p| resolve(p.name)).collect();
        let list = || if outs.is_empty() { "none".to_string() } else { outs.join(", ") };
        match r.port {
            Some(p) => match WirePort::try_from_name(p) {
                Some(wp) if outs.contains(&p) => Ok(wp),
                _ => Err(self.err_at(
                    r.at,
                    format!("`%{}.{p}`", r.name),
                    format!(
                        "`%{}` ({}) has no output port `{p}`; its output ports are: {}",
                        r.name,
                        defaults::class_spelling(pn.class),
                        list()
                    ),
                )),
            },
            None if outs.len() == 1 => WirePort::try_from_name(outs[0]).ok_or_else(|| {
                self.err_at(r.at, format!("`%{}`", r.name), format!("output port `{}` is not a known wire port", outs[0]))
            }),
            None => Err(self.err_at(
                r.at,
                format!("`%{}`", r.name),
                format!(
                    "`%{}` ({}) has {} output ports, so name one: `%{}.Port`; its outputs are: {}",
                    r.name,
                    defaults::class_spelling(pn.class),
                    outs.len(),
                    r.name,
                    list()
                ),
            )),
        }
    }
}

impl<'a> PModule<'a> {
    fn new(name: Option<String>, chip_node: Option<usize>) -> Self {
        PModule {
            name,
            chip_node,
            nodes: Vec::new(),
            scopes: Vec::new(),
            inputs: None,
            outputs: None,
            wire_order: None,
        }
    }
}
