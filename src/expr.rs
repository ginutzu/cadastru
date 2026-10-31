//! Expresii pe atribute, în stilul clauzei SQL `WHERE`:
//!
//! ```text
//! admin_unit_id = 123
//! status = 1 AND is_active = 1
//! (judet = 'Argeș' OR judet = 'Olt') AND suprafata >= 5000
//! nume LIKE 'Pit%'   ·   cod IN (13, 14, 15)   ·   cod NOT IN ('A', 'B')
//! suprafata BETWEEN 100 AND 500   ·   observatii IS NULL   ·   NOT activ
//! ```
//!
//! Numele câmpurilor se scriu simplu sau între ghilimele duble / paranteze drepte
//! (`"nr cad"`, `[nr cad]`); textele între apostrofuri (`'Argeș'`). Un text între ghilimele duble
//! care nu este numele unui câmp este luat ca valoare (`judet = "Argeș"`).
//! Operatorii logici: `AND`/`OR`/`NOT` (și `ȘI`/`SAU`/`NU`, `&&`/`||`/`!`); `AND` are prioritate.
//!
//! Comparațiile: numere ca numere (și textele care conțin numere, dacă valoarea nu este scrisă
//! între apostrofuri), textele fără să țină cont de majuscule și de diacritice (în afară de modul
//! „exact”). O valoare lipsă (NULL) nu îndeplinește nicio comparație; `IS NULL` găsește și
//! textele goale.

use crate::i18n::tr;
use crate::model::{Value, VectorLayer};
use crate::query::fold;
use std::cmp::Ordering;
use std::fmt;

/// Eroare de sintaxă, cu poziția (în caractere, de la 0) în textul expresiei.
#[derive(Clone, Debug, PartialEq)]
pub struct ExprError {
    pub msg: String,
    pub pos: usize,
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", tr!("{} (la caracterul {})", self.msg, self.pos + 1))
    }
}

impl std::error::Error for ExprError {}

// ---------------------------------------------------------------------------------------------
// Analiza lexicală

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    /// nume simplu (câmp sau cuvânt cheie)
    Ident(String),
    /// "nume" – câmp sau, dacă nu există câmpul, text
    DQuoted(String),
    /// [nume] sau `nume` – întotdeauna câmp
    Field(String),
    /// 'text'
    Str(String),
    Num(f64),
    /// operator sau semn: = <> < <= > >= ( ) , && || !
    Sym(&'static str),
}

struct Token {
    tok: Tok,
    /// poziția în caractere
    pos: usize,
}

/// Simbolul așa cum a fost scris (pentru mesajele de eroare).
fn tok_text(t: &Tok) -> String {
    match t {
        Tok::Ident(s) => s.clone(),
        Tok::DQuoted(s) => format!("\"{s}\""),
        Tok::Field(s) => format!("[{s}]"),
        Tok::Str(s) => format!("'{s}'"),
        Tok::Num(v) => Value::Real(*v).to_text(),
        Tok::Sym(s) => s.to_string(),
    }
}

fn err(msg: impl Into<String>, pos: usize) -> ExprError {
    ExprError { msg: msg.into(), pos }
}

fn lex(src: &str) -> Result<Vec<Token>, ExprError> {
    let cs: Vec<char> = src.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        let start = i;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // text între delimitatori; delimitatorul dublat se citește ca atare ('' -> ')
        let quoted = |i: &mut usize, close: char| -> Result<String, ExprError> {
            let mut s = String::new();
            *i += 1;
            loop {
                if *i >= cs.len() {
                    return Err(err(tr!("Lipsește {} de închidere", close), start));
                }
                if cs[*i] == close {
                    if close != ']' && cs.get(*i + 1) == Some(&close) {
                        s.push(close);
                        *i += 2;
                        continue;
                    }
                    *i += 1;
                    return Ok(s);
                }
                s.push(cs[*i]);
                *i += 1;
            }
        };
        let tok = match c {
            '\'' | '‘' | '’' => Tok::Str(quoted(&mut i, if c == '\'' { '\'' } else { '’' })?),
            '"' | '“' | '”' | '„' => Tok::DQuoted(quoted(&mut i, if c == '"' { '"' } else { '”' })?),
            '[' => Tok::Field(quoted(&mut i, ']')?),
            '`' => Tok::Field(quoted(&mut i, '`')?),
            '(' | ')' | ',' => {
                i += 1;
                Tok::Sym(match c {
                    '(' => "(",
                    ')' => ")",
                    _ => ",",
                })
            }
            '=' => {
                i += if cs.get(i + 1) == Some(&'=') { 2 } else { 1 };
                Tok::Sym("=")
            }
            '!' => {
                if cs.get(i + 1) == Some(&'=') {
                    i += 2;
                    Tok::Sym("<>")
                } else {
                    i += 1;
                    Tok::Sym("!")
                }
            }
            '≠' => {
                i += 1;
                Tok::Sym("<>")
            }
            '≤' => {
                i += 1;
                Tok::Sym("<=")
            }
            '≥' => {
                i += 1;
                Tok::Sym(">=")
            }
            '<' => match cs.get(i + 1) {
                Some('>') => {
                    i += 2;
                    Tok::Sym("<>")
                }
                Some('=') => {
                    i += 2;
                    Tok::Sym("<=")
                }
                _ => {
                    i += 1;
                    Tok::Sym("<")
                }
            },
            '>' => {
                if cs.get(i + 1) == Some(&'=') {
                    i += 2;
                    Tok::Sym(">=")
                } else {
                    i += 1;
                    Tok::Sym(">")
                }
            }
            '&' if cs.get(i + 1) == Some(&'&') => {
                i += 2;
                Tok::Sym("&&")
            }
            '|' if cs.get(i + 1) == Some(&'|') => {
                i += 2;
                Tok::Sym("||")
            }
            '-' | '+' | '.' | '0'..='9'
                if c.is_ascii_digit()
                    || cs.get(i + 1).is_some_and(|d| d.is_ascii_digit())
                    || (c != '.' && cs.get(i + 1) == Some(&'.') && cs.get(i + 2).is_some_and(|d| d.is_ascii_digit())) =>
            {
                let mut j = i + 1;
                while j < cs.len() && (cs[j].is_ascii_digit() || cs[j] == '.') {
                    j += 1;
                }
                // exponent
                if j < cs.len()
                    && (cs[j] == 'e' || cs[j] == 'E')
                    && (cs.get(j + 1).is_some_and(|d| d.is_ascii_digit())
                        || (matches!(cs.get(j + 1), Some('+') | Some('-'))
                            && cs.get(j + 2).is_some_and(|d| d.is_ascii_digit())))
                {
                    j += 2;
                    while j < cs.len() && cs[j].is_ascii_digit() {
                        j += 1;
                    }
                }
                let s: String = cs[i..j].iter().collect();
                // un număr lipit de litere (ex. 12abc) este o greșeală
                if j < cs.len() && (cs[j].is_alphabetic() || cs[j] == '_') {
                    return Err(err(tr!("Număr greșit"), start));
                }
                let v: f64 = s.parse().map_err(|_| err(tr!("Număr greșit"), start))?;
                i = j;
                Tok::Num(v)
            }
            c if c.is_alphanumeric() || c == '_' || c == '$' => {
                let mut j = i + 1;
                while j < cs.len() && (cs[j].is_alphanumeric() || cs[j] == '_' || cs[j] == '$') {
                    j += 1;
                }
                let s: String = cs[i..j].iter().collect();
                i = j;
                Tok::Ident(s)
            }
            _ => return Err(err(tr!("Caracter neașteptat: {}", c), start)),
        };
        out.push(Token { tok, pos: start });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Arborele expresiei

/// O valoare scrisă în expresie.
#[derive(Clone, Debug)]
struct Lit {
    text: String,
    num: Option<f64>,
    /// scrisă între apostrofuri: se compară întotdeauna ca text
    quoted: bool,
    /// număr scris în expresie
    is_num: bool,
    null: bool,
}

impl Lit {
    fn text(s: &str) -> Lit {
        Lit {
            text: s.to_string(),
            num: None,
            quoted: true,
            is_num: false,
            null: false,
        }
    }
    fn num(v: f64) -> Lit {
        Lit {
            text: crate::model::Value::Real(v).to_text(),
            num: Some(v),
            quoted: false,
            is_num: true,
            null: false,
        }
    }
    fn bool(b: bool) -> Lit {
        Lit {
            text: if b { "true".into() } else { "false".into() },
            num: Some(if b { 1.0 } else { 0.0 }),
            quoted: false,
            is_num: false,
            null: false,
        }
    }
    fn null() -> Lit {
        Lit {
            text: String::new(),
            num: None,
            quoted: false,
            is_num: false,
            null: true,
        }
    }
}

#[derive(Clone, Debug)]
enum Operand {
    Field(usize),
    Lit(Lit),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug)]
enum Node {
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Not(Box<Node>),
    Cmp(Operand, Cmp, Operand),
    Like(Operand, Operand),
    In(Operand, Vec<Operand>),
    Between(Operand, Operand, Operand),
    IsNull(Operand),
    /// valoare folosită singură: adevărată dacă nu este goală, 0 sau fals
    Truthy(Operand),
}

/// Valoarea unui operand pentru o entitate, pregătită pentru comparații.
struct Val<'a> {
    null: bool,
    text: std::borrow::Cow<'a, str>,
    num: Option<f64>,
    /// text scris explicit (între apostrofuri)
    quoted: bool,
    /// număr scris în expresie
    is_num: bool,
}

fn num_of_text(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>()
        .ok()
        .or_else(|| t.replace(',', ".").parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

/// O expresie pregătită pentru un strat.
#[derive(Clone, Debug)]
pub struct Expr {
    root: Node,
    exact: bool,
}

struct Parser<'a> {
    toks: Vec<Token>,
    i: usize,
    layer: &'a VectorLayer,
    end: usize,
}

fn keyword(t: &Tok) -> Option<String> {
    match t {
        Tok::Ident(s) => Some(s.to_uppercase()),
        _ => None,
    }
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i).map(|t| &t.tok)
    }
    fn pos(&self) -> usize {
        self.toks.get(self.i).map(|t| t.pos).unwrap_or(self.end)
    }
    fn is_kw(&self, words: &[&str]) -> bool {
        self.peek()
            .and_then(keyword)
            .is_some_and(|k| words.contains(&k.as_str()))
    }
    fn is_sym(&self, s: &str) -> bool {
        self.peek() == Some(&Tok::Sym(match s {
            "(" => "(",
            ")" => ")",
            "," => ",",
            "!" => "!",
            "&&" => "&&",
            "||" => "||",
            _ => return false,
        }))
    }
    fn eat_kw(&mut self, words: &[&str]) -> bool {
        if self.is_kw(words) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expect_sym(&mut self, s: &'static str) -> Result<(), ExprError> {
        if self.peek() == Some(&Tok::Sym(s)) {
            self.i += 1;
            Ok(())
        } else {
            Err(err(tr!("Se aștepta „{}”", s), self.pos()))
        }
    }

    fn or(&mut self) -> Result<Node, ExprError> {
        let mut l = self.and()?;
        while self.eat_kw(&["OR", "SAU"]) || self.eat_sym("||") {
            let r = self.and()?;
            l = Node::Or(Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn and(&mut self) -> Result<Node, ExprError> {
        let mut l = self.not()?;
        while self.eat_kw(&["AND", "ȘI", "ŞI", "SI"]) || self.eat_sym("&&") {
            let r = self.not()?;
            l = Node::And(Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn eat_sym(&mut self, s: &str) -> bool {
        if self.is_sym(s) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn not(&mut self) -> Result<Node, ExprError> {
        if self.eat_kw(&["NOT", "NU"]) || self.eat_sym("!") {
            return Ok(Node::Not(Box::new(self.not()?)));
        }
        self.predicate()
    }
    fn predicate(&mut self) -> Result<Node, ExprError> {
        if self.eat_sym("(") {
            let n = self.or()?;
            self.expect_sym(")")?;
            return Ok(n);
        }
        let a = self.operand()?;
        // operatorii de comparație
        let cmp = match self.peek() {
            Some(Tok::Sym("=")) => Some(Cmp::Eq),
            Some(Tok::Sym("<>")) => Some(Cmp::Ne),
            Some(Tok::Sym("<")) => Some(Cmp::Lt),
            Some(Tok::Sym("<=")) => Some(Cmp::Le),
            Some(Tok::Sym(">")) => Some(Cmp::Gt),
            Some(Tok::Sym(">=")) => Some(Cmp::Ge),
            _ => None,
        };
        if let Some(c) = cmp {
            self.i += 1;
            let b = self.operand()?;
            // x = NULL / x <> NULL: ca IS NULL / IS NOT NULL
            let lit_null = |o: &Operand| matches!(o, Operand::Lit(l) if l.null);
            if lit_null(&b) || lit_null(&a) {
                let v = if lit_null(&b) { a } else { b };
                return match c {
                    Cmp::Eq => Ok(Node::IsNull(v)),
                    Cmp::Ne => Ok(Node::Not(Box::new(Node::IsNull(v)))),
                    _ => Err(err(tr!("NULL se poate compara doar cu = sau <>"), self.pos())),
                };
            }
            return Ok(Node::Cmp(a, c, b));
        }
        if self.eat_kw(&["IS"]) {
            let neg = self.eat_kw(&["NOT"]);
            if !self.eat_kw(&["NULL"]) {
                return Err(err(tr!("Se aștepta NULL după IS"), self.pos()));
            }
            let n = Node::IsNull(a);
            return Ok(if neg { Node::Not(Box::new(n)) } else { n });
        }
        let neg = self.eat_kw(&["NOT"]);
        let n = if self.eat_kw(&["LIKE", "ILIKE"]) {
            Node::Like(a, self.operand()?)
        } else if self.eat_kw(&["IN"]) {
            self.expect_sym("(")?;
            let mut list = vec![self.operand()?];
            while self.eat_sym(",") {
                list.push(self.operand()?);
            }
            self.expect_sym(")")?;
            Node::In(a, list)
        } else if self.eat_kw(&["BETWEEN"]) {
            let lo = self.operand()?;
            if !self.eat_kw(&["AND", "ȘI", "ŞI", "SI"]) {
                return Err(err(tr!("Se aștepta AND în BETWEEN … AND …"), self.pos()));
            }
            let hi = self.operand()?;
            Node::Between(a, lo, hi)
        } else if neg {
            return Err(err(tr!("Se aștepta LIKE, IN sau BETWEEN după NOT"), self.pos()));
        } else {
            // valoare folosită singură (ex. `is_active`, `TRUE`)
            return Ok(Node::Truthy(a));
        };
        Ok(if neg { Node::Not(Box::new(n)) } else { n })
    }

    fn field(&self, name: &str) -> Option<usize> {
        self.layer.field_index(name).or_else(|| {
            let f = fold(name);
            self.layer.fields.iter().position(|x| fold(&x.name) == f)
        })
    }

    fn operand(&mut self) -> Result<Operand, ExprError> {
        let pos = self.pos();
        let Some(t) = self.toks.get(self.i).map(|t| t.tok.clone()) else {
            return Err(err(tr!("Expresia s-a terminat neașteptat"), pos));
        };
        self.i += 1;
        Ok(match t {
            Tok::Num(v) => Operand::Lit(Lit::num(v)),
            Tok::Str(s) => Operand::Lit(Lit::text(&s)),
            Tok::DQuoted(s) => match self.field(&s) {
                Some(i) => Operand::Field(i),
                None => Operand::Lit(Lit::text(&s)),
            },
            Tok::Field(s) => match self.field(&s) {
                Some(i) => Operand::Field(i),
                None => return Err(err(tr!("Câmpul „{}” nu există în strat", s), pos)),
            },
            Tok::Ident(s) => match s.to_uppercase().as_str() {
                "NULL" => Operand::Lit(Lit::null()),
                "TRUE" | "ADEVARAT" | "ADEVĂRAT" => Operand::Lit(Lit::bool(true)),
                "FALSE" | "FALS" => Operand::Lit(Lit::bool(false)),
                "AND" | "OR" | "NOT" | "LIKE" | "ILIKE" | "IN" | "BETWEEN" | "IS" | "ȘI" | "ŞI" | "SAU"
                    if self.field(&s).is_none() =>
                {
                    return Err(err(tr!("Se aștepta un câmp sau o valoare în locul lui {}", s), pos));
                }
                _ => match self.field(&s) {
                    Some(i) => Operand::Field(i),
                    None => {
                        return Err(err(
                            tr!(
                                "Câmpul „{}” nu există în strat (un text se scrie între apostrofuri: '{}')",
                                s,
                                s
                            ),
                            pos,
                        ));
                    }
                },
            },
            Tok::Sym(s) => {
                return Err(err(tr!("Se aștepta un câmp sau o valoare în locul lui {}", s), pos));
            }
        })
    }
}

/// Potrivire LIKE: `%` = orice șir, `_` = un caracter.
fn like(s: &[char], p: &[char]) -> bool {
    // algoritm cu revenire la ultimul %
    let (mut i, mut j) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while i < s.len() {
        if j < p.len() && (p[j] == '_' || p[j] == s[i]) {
            i += 1;
            j += 1;
        } else if j < p.len() && p[j] == '%' {
            star = Some((j, i));
            j += 1;
        } else if let Some((sj, si)) = star {
            j = sj + 1;
            i = si + 1;
            star = Some((sj, si + 1));
        } else {
            return false;
        }
    }
    while j < p.len() && p[j] == '%' {
        j += 1;
    }
    j == p.len()
}

impl Expr {
    /// Pregătește expresia pentru stratul dat. `exact` = textele se compară exact
    /// (cu majuscule și diacritice).
    pub fn compile(src: &str, layer: &VectorLayer, exact: bool) -> Result<Expr, ExprError> {
        let toks = lex(src)?;
        if toks.is_empty() {
            return Err(err(tr!("Expresia este goală"), 0));
        }
        let mut p = Parser {
            toks,
            i: 0,
            layer,
            end: src.chars().count(),
        };
        let root = p.or()?;
        if let Some(t) = p.toks.get(p.i) {
            let msg = match &t.tok {
                Tok::Sym(")") => tr!("Paranteză „)” în plus").to_string(),
                tok => tr!(
                    "Se aștepta un operator (=, <>, LIKE, IN …) sau AND / OR înainte de {}",
                    tok_text(tok)
                ),
            };
            return Err(err(msg, t.pos));
        }
        Ok(Expr { root, exact })
    }

    fn val<'a>(&self, o: &'a Operand, attrs: &'a [Value]) -> Val<'a> {
        use std::borrow::Cow;
        match o {
            Operand::Lit(l) => Val {
                null: l.null,
                text: Cow::Borrowed(&l.text),
                num: l.num,
                quoted: l.quoted,
                is_num: l.is_num,
            },
            Operand::Field(i) => match attrs.get(*i).unwrap_or(&Value::Null) {
                Value::Null => Val {
                    null: true,
                    text: Cow::Borrowed(""),
                    num: None,
                    quoted: false,
                    is_num: false,
                },
                Value::Text(s) => Val {
                    null: false,
                    text: Cow::Borrowed(s.as_str()),
                    num: num_of_text(s),
                    quoted: false,
                    is_num: false,
                },
                v => Val {
                    null: false,
                    text: Cow::Owned(v.to_text()),
                    num: v.as_f64(),
                    quoted: false,
                    is_num: false,
                },
            },
        }
    }

    fn norm(&self, s: &str) -> String {
        if self.exact { s.trim().to_string() } else { fold(s) }
    }

    /// Ordinea a două valori; `None` dacă una lipsește sau dacă un număr scris în expresie se
    /// compară cu un text care nu este număr.
    fn order(&self, a: &Val, b: &Val) -> Option<Ordering> {
        if a.null || b.null {
            return None;
        }
        if (a.is_num && b.num.is_none()) || (b.is_num && a.num.is_none()) {
            return None;
        }
        if !a.quoted
            && !b.quoted
            && let (Some(x), Some(y)) = (a.num, b.num)
        {
            if (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0) {
                return Some(Ordering::Equal);
            }
            return x.partial_cmp(&y);
        }
        Some(self.norm(&a.text).cmp(&self.norm(&b.text)))
    }

    /// Valoarea de adevăr, cu logica SQL în trei valori: `None` = necunoscut (valoare lipsă).
    fn eval(&self, n: &Node, attrs: &[Value]) -> Option<bool> {
        match n {
            Node::And(a, b) => match (self.eval(a, attrs), self.eval(b, attrs)) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            },
            Node::Or(a, b) => match (self.eval(a, attrs), self.eval(b, attrs)) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            },
            Node::Not(a) => self.eval(a, attrs).map(|v| !v),
            Node::Cmp(a, c, b) => {
                let (va, vb) = (self.val(a, attrs), self.val(b, attrs));
                if va.null || vb.null {
                    return None;
                }
                let Some(o) = self.order(&va, &vb) else {
                    // un număr comparat cu un text care nu e număr: doar „diferit” e adevărat
                    return Some(*c == Cmp::Ne);
                };
                Some(match c {
                    Cmp::Eq => o == Ordering::Equal,
                    Cmp::Ne => o != Ordering::Equal,
                    Cmp::Lt => o == Ordering::Less,
                    Cmp::Le => o != Ordering::Greater,
                    Cmp::Gt => o == Ordering::Greater,
                    Cmp::Ge => o != Ordering::Less,
                })
            }
            Node::Like(a, p) => {
                let v = self.val(a, attrs);
                let p = self.val(p, attrs);
                if v.null || p.null {
                    return None;
                }
                let s: Vec<char> = self.norm(&v.text).chars().collect();
                let pat: Vec<char> = self.norm(&p.text).chars().collect();
                Some(like(&s, &pat))
            }
            Node::In(a, list) => {
                let v = self.val(a, attrs);
                if v.null {
                    return None;
                }
                let mut unknown = false;
                for o in list {
                    let w = self.val(o, attrs);
                    if w.null {
                        unknown = true;
                    } else if self.order(&v, &w) == Some(Ordering::Equal) {
                        return Some(true);
                    }
                }
                if unknown { None } else { Some(false) }
            }
            Node::Between(a, lo, hi) => {
                let v = self.val(a, attrs);
                let (l, h) = (self.val(lo, attrs), self.val(hi, attrs));
                if v.null || l.null || h.null {
                    return None;
                }
                let lo = self.order(&v, &l);
                let hi = self.order(&v, &h);
                Some(
                    matches!(lo, Some(Ordering::Greater | Ordering::Equal))
                        && matches!(hi, Some(Ordering::Less | Ordering::Equal)),
                )
            }
            Node::IsNull(a) => {
                let v = self.val(a, attrs);
                Some(v.null || v.text.trim().is_empty())
            }
            Node::Truthy(a) => {
                let v = self.val(a, attrs);
                if v.null {
                    return None;
                }
                if let Some(x) = v.num {
                    return Some(x != 0.0);
                }
                let t = fold(&v.text);
                Some(!matches!(t.as_str(), "" | "false" | "fals" | "nu" | "no" | "f" | "n"))
            }
        }
    }
    /// Entitatea cu aceste atribute îndeplinește expresia.
    pub fn matches(&self, attrs: &[Value]) -> bool {
        self.eval(&self.root, attrs) == Some(true)
    }

    /// Indicii entităților stratului care îndeplinesc expresia.
    pub fn run(&self, layer: &VectorLayer) -> Vec<usize> {
        layer
            .features
            .iter()
            .enumerate()
            .filter(|(_, f)| self.matches(&f.attrs))
            .map(|(i, _)| i)
            .collect()
    }
}

/// Pregătește și aplică expresia: indicii entităților potrivite.
pub fn select(layer: &VectorLayer, src: &str, exact: bool) -> Result<Vec<usize>, ExprError> {
    Ok(Expr::compile(src, layer, exact)?.run(layer))
}

/// Păstrează în strat doar entitățile care îndeplinesc expresia; întoarce câte au fost scoase.
pub fn filter_layer(layer: &mut VectorLayer, src: &str) -> Result<usize, ExprError> {
    let e = Expr::compile(src, layer, false)?;
    let before = layer.features.len();
    layer.features.retain(|f| e.matches(&f.attrs));
    Ok(before - layer.features.len())
}

/// Numele unui câmp așa cum trebuie scris într-o expresie (între ghilimele dacă e nevoie).
pub fn quote_field(name: &str) -> String {
    let simple = name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !matches!(
            name.to_uppercase().as_str(),
            "AND" | "OR" | "NOT" | "LIKE" | "ILIKE" | "IN" | "BETWEEN" | "IS" | "NULL" | "TRUE" | "FALSE"
                | "ȘI" | "ŞI" | "SI" | "SAU" | "NU" | "FALS" | "ADEVARAT" | "ADEVĂRAT"
        );
    if simple {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

/// O valoare așa cum trebuie scrisă într-o expresie: numerele simplu, textele între apostrofuri.
pub fn quote_value(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Int(_) | Value::Real(_) => v.to_text(),
        Value::Bool(b) => if *b { "TRUE".into() } else { "FALSE".into() },
        Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Feature, Field, FieldType};

    fn layer() -> VectorLayer {
        let mut l = VectorLayer::new("t");
        l.fields = vec![
            Field::new("admin_unit_id", FieldType::Integer),
            Field::new("status", FieldType::Integer),
            Field::new("is_active", FieldType::Text),
            Field::new("judet", FieldType::Text),
            Field::new("nr cad", FieldType::Text),
            Field::new("supr", FieldType::Real),
        ];
        let rows: [(i64, i64, &str, &str, &str, Option<f64>); 5] = [
            (13, 1, "1", "Argeș", "1001", Some(120.5)),
            (13, 1, "0", "Arges", "1002", Some(5000.0)),
            (14, 2, "1", "Olt", "A-7", None),
            (40, 1, "1", "București", "", Some(300.0)),
            (40, 0, "true", "ILFOV", "1'2", Some(-4.0)),
        ];
        for (a, s, act, j, nc, sp) in rows {
            l.features.push(Feature {
                geom: None,
                attrs: vec![
                    Value::Int(a),
                    Value::Int(s),
                    Value::Text(act.into()),
                    Value::Text(j.into()),
                    if nc.is_empty() { Value::Null } else { Value::Text(nc.into()) },
                    sp.map(Value::Real).unwrap_or(Value::Null),
                ],
            });
        }
        l
    }

    fn q(s: &str) -> Vec<usize> {
        select(&layer(), s, false).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn simple_and_combined() {
        assert_eq!(q("admin_unit_id = 13"), vec![0, 1]);
        assert_eq!(q("status=1 and is_active=1"), vec![0, 3]);
        assert_eq!(q("STATUS = 1 AND IS_ACTIVE = 1"), vec![0, 3]);
        assert_eq!(q("status = 1 && is_active == '1'"), vec![0, 3]);
        assert_eq!(q("admin_unit_id <> 13"), vec![2, 3, 4]);
        assert_eq!(q("admin_unit_id != 13 AND status >= 1"), vec![2, 3]);
        // AND înaintea lui OR; parantezele schimbă ordinea
        assert_eq!(q("admin_unit_id = 14 OR status = 1 AND is_active = 0"), vec![1, 2]);
        assert_eq!(q("(admin_unit_id = 14 OR status = 1) AND is_active = 0"), vec![1]);
        assert_eq!(q("NOT status = 1"), vec![2, 4]);
        assert_eq!(q("status = 1 ȘI judet = 'arges'"), vec![0, 1]);
        assert_eq!(q("status = 1 SAU status = 2"), vec![0, 1, 2, 3]);
    }

    #[test]
    fn text_and_numbers() {
        // fără majuscule și diacritice; "..." care nu e câmp = text
        assert_eq!(q("judet = 'arges'"), vec![0, 1]);
        assert_eq!(q("judet = \"Bucuresti\""), vec![3]);
        assert_eq!(select(&layer(), "judet = 'Arges'", true).unwrap(), vec![1]);
        // câmp cu spațiu
        assert_eq!(q("\"nr cad\" = 1002"), vec![1]);
        assert_eq!(q("[nr cad] = '1''2'"), vec![4]);
        assert_eq!(q("supr > 200"), vec![1, 3]);
        assert_eq!(q("supr < 0"), vec![4]);
        assert_eq!(q("supr >= 120.5 and supr <= 300"), vec![0, 3]);
        assert_eq!(q("supr = 1.205e2"), vec![0]);
        // text care conține un număr, comparat cu un număr
        assert_eq!(q("\"nr cad\" > 1001"), vec![1]);
        assert_eq!(q("\"nr cad\" <> 1001"), vec![1, 2, 4]);
        assert_eq!(q("\"nr cad\" > '1001'"), vec![1, 2]);
    }

    #[test]
    fn like_in_between_null() {
        assert_eq!(q("judet LIKE 'ar%'"), vec![0, 1]);
        assert_eq!(q("judet LIKE '_lt'"), vec![2]);
        assert_eq!(q("judet NOT LIKE '%e%'"), vec![2, 4]);
        assert_eq!(q("admin_unit_id IN (13, 40)"), vec![0, 1, 3, 4]);
        assert_eq!(q("judet NOT IN ('olt', 'ilfov')"), vec![0, 1, 3]);
        assert_eq!(q("supr BETWEEN 100 AND 400"), vec![0, 3]);
        assert_eq!(q("supr NOT BETWEEN 100 AND 400"), vec![1, 4]);
        assert_eq!(q("supr IS NULL"), vec![2]);
        assert_eq!(q("\"nr cad\" IS NOT NULL"), vec![0, 1, 2, 4]);
        assert_eq!(q("supr = NULL"), vec![2]);
        // valori lipsă: nicio comparație nu e adevărată
        assert_eq!(q("supr <> 120.5"), vec![1, 3, 4]);
        // valoare singură
        assert_eq!(q("is_active"), vec![0, 2, 3, 4]);
        assert_eq!(q("NOT is_active"), vec![1]);
        assert_eq!(q("TRUE"), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn errors() {
        let l = layer();
        let e = Expr::compile("statu = 1", &l, false).unwrap_err();
        assert_eq!(e.pos, 0);
        assert!(e.msg.contains("statu"));
        let e = Expr::compile("status = 1 and", &l, false).unwrap_err();
        assert_eq!(e.pos, 14);
        assert!(Expr::compile("(status = 1", &l, false).is_err());
        assert!(Expr::compile("status = 1)", &l, false).is_err());
        assert!(Expr::compile("judet = 'abc", &l, false).is_err());
        assert!(Expr::compile("", &l, false).is_err());
        assert!(Expr::compile("status IN 1", &l, false).is_err());
        assert!(Expr::compile("status = 12abc", &l, false).is_err());
        assert!(Expr::compile("status > NULL", &l, false).is_err());
        let e = Expr::compile("status  2", &l, false).unwrap_err();
        assert_eq!(e.pos, 8);
        assert!(e.msg.contains('2'), "{}", e.msg);
    }

    #[test]
    fn quoting() {
        assert_eq!(quote_field("status"), "status");
        assert_eq!(quote_field("nr cad"), "\"nr cad\"");
        assert_eq!(quote_field("IN"), "\"IN\"");
        assert_eq!(quote_value(&Value::Text("1'2".into())), "'1''2'");
        assert_eq!(quote_value(&Value::Int(5)), "5");
        let l = layer();
        for v in ["1'2", "Argeș"] {
            let s = format!("{} = {}", quote_field("nr cad"), quote_value(&Value::Text(v.into())));
            assert!(Expr::compile(&s, &l, false).is_ok());
        }
        assert!(like(&['a', 'b', 'c'], &['%', 'c']));
        assert!(like(&['a', 'b', 'c'], &['a', '%', '%']));
        assert!(!like(&['a', 'b'], &['a', '_', '_']));
    }
}
