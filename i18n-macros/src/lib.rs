//! Macrourile de traducere ale aplicației Cadastru.
//!
//! Textele sursă sunt în limba română. Pachetul englez este fișierul `lang/en.tsv` al
//! crate-ului care folosește macroul: câte un rând `text românesc<TAB>text englez`, scrise
//! exact ca în sursa Rust (cu aceleași secvențe `\n`, `\"` …). Rândurile care încep cu `#`
//! sunt comentarii.
//!
//! * `tr!("Deschide")` → `&'static str` în limba curentă;
//! * `tr!("Încărcat: {} ({})", a, b)` sau `tr!("Export eșuat: {e:#}")` → `String`, formatat
//!   cu șablonul limbii curente (aceleași locuri de completat; verificate de `format!`).
//! * `tr_table!()` → tabelul `&[(&str, &str)]` cu toate perechile, pentru textele care nu
//!   sunt literale în cod (nume din constante), traduse la rulare cu `i18n::trs`.
//!
//! Un text care lipsește din pachet rămâne în română.

use proc_macro::{Delimiter, Group, Literal, Punct, Spacing, TokenStream, TokenTree};
use std::collections::HashMap;

fn load_pack() -> HashMap<String, String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let path = std::path::Path::new(&dir).join("lang").join("en.tsv");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut m = HashMap::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if let Some((ro, en)) = line.split_once('\t') {
            m.insert(ro.to_string(), en.to_string());
        }
    }
    m
}

fn pack() -> &'static HashMap<String, String> {
    use std::sync::OnceLock;
    static P: OnceLock<HashMap<String, String>> = OnceLock::new();
    P.get_or_init(load_pack)
}

/// Conținutul brut (sursă) al unui literal "…", fără ghilimele.
fn raw_content(lit: &Literal) -> Option<String> {
    let s = lit.to_string();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        Some(s[1..s.len() - 1].to_string())
    } else {
        None
    }
}

fn has_placeholders(s: &str) -> bool {
    let t = s.replace("{{", "").replace("}}", "");
    t.contains('{')
}

fn path(segs: &[&str]) -> TokenStream {
    // ::cadastru::i18n::is_en
    let mut ts = TokenStream::new();
    for s in segs {
        ts.extend([
            TokenTree::Punct(Punct::new(':', Spacing::Joint)),
            TokenTree::Punct(Punct::new(':', Spacing::Alone)),
        ]);
        ts.extend(s.parse::<TokenStream>().unwrap());
    }
    ts
}

#[proc_macro]
pub fn tr(input: TokenStream) -> TokenStream {
    let mut it = input.into_iter();
    let first = it.next();
    let Some(TokenTree::Literal(lit)) = first else {
        return "compile_error!(\"tr!: primul argument trebuie să fie un text literal\")"
            .parse()
            .unwrap();
    };
    let Some(ro) = raw_content(&lit) else {
        return "compile_error!(\"tr!: se așteaptă un șir \\\"…\\\"\")".parse().unwrap();
    };
    let rest: TokenStream = it.collect();
    let en = pack().get(&ro).cloned();
    let formatted = has_placeholders(&ro) || !rest.is_empty();
    let lit_of = |s: &str| -> TokenStream { format!("\"{s}\"").parse().unwrap() };
    let call = |l: TokenStream| -> TokenStream {
        // format!(l <rest>)
        let mut args = l;
        args.extend(rest.clone());
        let mut ts: TokenStream = "::std::format!".parse().unwrap();
        ts.extend([TokenTree::Group(Group::new(Delimiter::Parenthesis, args))]);
        ts
    };
    let ro_expr = if formatted { call(lit_of(&ro)) } else { lit_of(&ro) };
    let Some(en) = en else {
        return ro_expr;
    };
    let en_expr = if formatted { call(lit_of(&en)) } else { lit_of(&en) };
    // if ::cadastru::i18n::is_en() { en } else { ro }
    let mut out: TokenStream = "if".parse().unwrap();
    out.extend(path(&["cadastru", "i18n", "is_en"]));
    out.extend([TokenTree::Group(Group::new(Delimiter::Parenthesis, TokenStream::new()))]);
    out.extend([TokenTree::Group(Group::new(Delimiter::Brace, en_expr))]);
    out.extend("else".parse::<TokenStream>().unwrap());
    out.extend([TokenTree::Group(Group::new(Delimiter::Brace, ro_expr))]);
    let mut wrapped = TokenStream::new();
    wrapped.extend([TokenTree::Group(Group::new(Delimiter::Parenthesis, out))]);
    wrapped
}

/// Tabelul complet al pachetului englez (pentru traducerea la rulare a textelor din constante).
#[proc_macro]
pub fn tr_table(_input: TokenStream) -> TokenStream {
    let mut pairs: Vec<(&String, &String)> = pack().iter().collect();
    pairs.sort();
    let mut s = String::from("&[");
    for (ro, en) in pairs {
        s.push_str(&format!("(\"{ro}\", \"{en}\"),"));
    }
    s.push(']');
    s.parse().unwrap()
}
