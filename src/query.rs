//! Interogări pe atribute: condiții pe câmpuri combinate cu ȘI / SAU.
//!
//! O interogare este o listă de condiții. Fiecare condiție (în afară de prima) se leagă de
//! cea dinainte cu ȘI sau SAU; ȘI are prioritate, ca în SQL: `A ȘI B SAU C` înseamnă
//! `(A ȘI B) SAU C`. Textele se compară fără să țină cont de majuscule și de diacritice
//! („Ştefăneşti” = „stefanesti”); valorile numerice se compară ca numere.

use crate::i18n::tr;
use crate::model::{Value, VectorLayer};

/// Operatorul unei condiții.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    Gt,
    Ge,
    Lt,
    Le,
    Empty,
    NotEmpty,
}

impl Op {
    pub const ALL: [Op; 12] = [
        Op::Eq,
        Op::Ne,
        Op::Contains,
        Op::NotContains,
        Op::StartsWith,
        Op::EndsWith,
        Op::Gt,
        Op::Ge,
        Op::Lt,
        Op::Le,
        Op::Empty,
        Op::NotEmpty,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "≠",
            Op::Contains => tr!("conține"),
            Op::NotContains => tr!("nu conține"),
            Op::StartsWith => tr!("începe cu"),
            Op::EndsWith => tr!("se termină cu"),
            Op::Gt => ">",
            Op::Ge => "≥",
            Op::Lt => "<",
            Op::Le => "≤",
            Op::Empty => tr!("este gol"),
            Op::NotEmpty => tr!("nu este gol"),
        }
    }
    /// Operatorul are nevoie de o valoare de comparat.
    pub fn needs_value(self) -> bool {
        !matches!(self, Op::Empty | Op::NotEmpty)
    }
    /// Operatorii negativi: pentru „orice câmp” înseamnă „niciun câmp nu …”.
    fn negated(self) -> Option<Op> {
        match self {
            Op::Ne => Some(Op::Eq),
            Op::NotContains => Some(Op::Contains),
            _ => None,
        }
    }
}

/// Legătura unei condiții cu cea dinainte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Join {
    #[default]
    And,
    Or,
}

impl Join {
    pub fn label(self) -> &'static str {
        match self {
            Join::And => tr!("ȘI"),
            Join::Or => tr!("SAU"),
        }
    }
}

/// O condiție: câmpul (sau orice câmp, dacă `field` este `None`), operatorul și valoarea.
#[derive(Clone, Debug, PartialEq)]
pub struct Cond {
    pub join: Join,
    pub field: Option<String>,
    pub op: Op,
    pub value: String,
}

impl Cond {
    pub fn new(field: Option<&str>, op: Op, value: &str) -> Cond {
        Cond {
            join: Join::And,
            field: field.map(|s| s.to_string()),
            op,
            value: value.to_string(),
        }
    }
    pub fn or(mut self) -> Cond {
        self.join = Join::Or;
        self
    }
}

/// Text normalizat pentru comparații: litere mici, fără diacritice, fără spații la capete.
pub fn fold(s: &str) -> String {
    s.trim()
        .chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'ă' | 'â' | 'á' | 'à' | 'ä' => 'a',
            'î' | 'í' | 'ì' | 'ï' => 'i',
            'ș' | 'ş' => 's',
            'ț' | 'ţ' => 't',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'ó' | 'ö' | 'ô' | 'ő' => 'o',
            'ú' | 'ü' | 'û' | 'ű' => 'u',
            c => c,
        })
        .collect()
}

/// Număr scris cu punct sau virgulă zecimală.
fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim().replace(' ', "");
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>()
        .ok()
        .or_else(|| t.replace(',', ".").parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

/// O condiție pregătită pentru un strat anume (indicele câmpului, valoarea normalizată).
struct Prepared {
    join: Join,
    /// `None` = orice câmp; `Some(None)` = câmpul nu există în strat
    field: Option<Option<usize>>,
    op: Op,
    text: String,
    num: Option<f64>,
}

fn test_value(v: &Value, op: Op, text: &str, num: Option<f64>) -> bool {
    let s = match v {
        Value::Null => String::new(),
        _ => fold(&v.to_text()),
    };
    let vnum = match v {
        Value::Text(t) => parse_num(t),
        Value::Bool(_) | Value::Null => None,
        _ => v.as_f64(),
    };
    let cmp = |o: std::cmp::Ordering| -> bool {
        let ord = match (vnum, num) {
            (Some(a), Some(b)) => a.partial_cmp(&b),
            _ if s.is_empty() => None,
            _ => Some(s.as_str().cmp(text)),
        };
        ord == Some(o)
    };
    use std::cmp::Ordering::*;
    match op {
        Op::Eq => match (vnum, num) {
            (Some(a), Some(b)) => (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0),
            _ => s == text,
        },
        Op::Ne => !test_value(v, Op::Eq, text, num),
        Op::Contains => s.contains(text),
        Op::NotContains => !s.contains(text),
        Op::StartsWith => s.starts_with(text),
        Op::EndsWith => s.ends_with(text),
        Op::Gt => cmp(Greater),
        Op::Ge => cmp(Greater) || cmp(Equal),
        Op::Lt => cmp(Less),
        Op::Le => cmp(Less) || cmp(Equal),
        Op::Empty => s.is_empty(),
        Op::NotEmpty => !s.is_empty(),
    }
}

impl Prepared {
    fn test(&self, attrs: &[Value]) -> bool {
        match self.field {
            Some(Some(i)) => attrs
                .get(i)
                .is_some_and(|v| test_value(v, self.op, &self.text, self.num)),
            // câmp inexistent în acest strat: condiția nu este îndeplinită
            Some(None) => false,
            None => match self.op.negated() {
                Some(pos) => !attrs
                    .iter()
                    .any(|v| test_value(v, pos, &self.text, self.num)),
                None => attrs
                    .iter()
                    .any(|v| test_value(v, self.op, &self.text, self.num)),
            },
        }
    }
}

/// O interogare pregătită pentru un strat.
pub struct Query {
    conds: Vec<Prepared>,
}

impl Query {
    pub fn new(layer: &VectorLayer, conds: &[Cond]) -> Query {
        let conds = conds
            .iter()
            .map(|c| Prepared {
                join: c.join,
                field: c.field.as_ref().map(|n| layer.field_index(n)),
                op: c.op,
                text: fold(&c.value),
                num: parse_num(&c.value),
            })
            .collect();
        Query { conds }
    }

    /// Valoarea interogării pentru o entitate (lista goală de condiții potrivește tot).
    pub fn matches(&self, attrs: &[Value]) -> bool {
        if self.conds.is_empty() {
            return true;
        }
        // SAU între grupuri de condiții legate prin ȘI
        let mut any = false;
        let mut group = true;
        for (i, c) in self.conds.iter().enumerate() {
            if i > 0 && c.join == Join::Or {
                any |= group;
                group = true;
            }
            if group {
                group = c.test(attrs);
            }
        }
        any || group
    }
}

/// Indicii entităților care îndeplinesc condițiile.
pub fn run(layer: &VectorLayer, conds: &[Cond]) -> Vec<usize> {
    let q = Query::new(layer, conds);
    layer
        .features
        .iter()
        .enumerate()
        .filter(|(_, f)| q.matches(&f.attrs))
        .map(|(i, _)| i)
        .collect()
}

/// Textul interogării, pentru afișare: `JUDET = "Argeș" ȘI POP > 1000`.
pub fn describe(conds: &[Cond]) -> String {
    let mut s = String::new();
    for (i, c) in conds.iter().enumerate() {
        if i > 0 {
            s.push_str(&format!(" {} ", c.join.label()));
        }
        let f = c.field.clone().unwrap_or_else(|| tr!("orice câmp").to_string());
        if c.op.needs_value() {
            let v = if parse_num(&c.value).is_some() {
                c.value.trim().to_string()
            } else {
                format!("\"{}\"", c.value.trim())
            };
            s.push_str(&format!("{f} {} {v}", c.op.label()));
        } else {
            s.push_str(&format!("{f} {}", c.op.label()));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Feature, Field, FieldType};

    fn layer() -> VectorLayer {
        let mut l = VectorLayer::new("t");
        l.fields = vec![
            Field::new("NUME", FieldType::Text),
            Field::new("JUDET", FieldType::Text),
            Field::new("POP", FieldType::Integer),
            Field::new("COD", FieldType::Text),
        ];
        let rows = [
            ("Ştefăneşti", "Argeș", 14000, "13"),
            ("Pitești", "Argeș", 155000, "13"),
            ("Brașov", "Brașov", 237000, "8"),
            ("Mioveni", "ARGES", 31000, ""),
        ];
        for (n, j, p, c) in rows {
            l.features.push(Feature {
                geom: None,
                attrs: vec![
                    Value::Text(n.into()),
                    Value::Text(j.into()),
                    Value::Int(p),
                    if c.is_empty() { Value::Null } else { Value::Text(c.into()) },
                ],
            });
        }
        l
    }

    #[test]
    fn diacritics_and_case() {
        let l = layer();
        assert_eq!(run(&l, &[Cond::new(Some("JUDET"), Op::Eq, "arges")]), vec![0, 1, 3]);
        assert_eq!(run(&l, &[Cond::new(Some("NUME"), Op::Eq, "stefanesti")]), vec![0]);
        assert_eq!(run(&l, &[Cond::new(Some("NUME"), Op::StartsWith, "PIT")]), vec![1]);
        assert_eq!(run(&l, &[Cond::new(Some("NUME"), Op::EndsWith, "ști")]), vec![0, 1]);
    }

    #[test]
    fn numbers_and_empty() {
        let l = layer();
        assert_eq!(run(&l, &[Cond::new(Some("POP"), Op::Gt, "100000")]), vec![1, 2]);
        assert_eq!(run(&l, &[Cond::new(Some("POP"), Op::Le, "31 000")]), vec![0, 3]);
        assert_eq!(run(&l, &[Cond::new(Some("COD"), Op::Eq, "13.0")]), vec![0, 1]);
        assert_eq!(run(&l, &[Cond::new(Some("COD"), Op::Empty, "")]), vec![3]);
        assert_eq!(run(&l, &[Cond::new(Some("COD"), Op::NotEmpty, "")]), vec![0, 1, 2]);
        // text comparat cu un număr: ordine alfabetică
        assert_eq!(run(&l, &[Cond::new(Some("NUME"), Op::Lt, "c")]), vec![2]);
    }

    #[test]
    fn and_or_precedence() {
        let l = layer();
        // (JUDET = arges ȘI POP > 100000) SAU NUME = brasov
        let q = [
            Cond::new(Some("JUDET"), Op::Eq, "argeș"),
            Cond::new(Some("POP"), Op::Gt, "100000"),
            Cond::new(Some("NUME"), Op::Eq, "brasov").or(),
        ];
        assert_eq!(run(&l, &q), vec![1, 2]);
        // JUDET = arges ȘI (nimic) – câmp inexistent nu potrivește nimic
        assert!(run(&l, &[Cond::new(Some("NU_EXISTA"), Op::NotEmpty, "")]).is_empty());
        assert_eq!(run(&l, &[]).len(), 4);
    }

    #[test]
    fn any_field() {
        let l = layer();
        assert_eq!(run(&l, &[Cond::new(None, Op::Contains, "bras")]), vec![2]);
        assert_eq!(run(&l, &[Cond::new(None, Op::NotContains, "arg")]), vec![2]);
        assert_eq!(run(&l, &[Cond::new(None, Op::Eq, "155000")]), vec![1]);
    }

    #[test]
    fn describe_text() {
        let q = [
            Cond::new(Some("JUDET"), Op::Eq, "Argeș"),
            Cond::new(Some("POP"), Op::Gt, "1000").or(),
        ];
        // eticheta legăturii depinde de limba curentă (alte teste o pot schimba în paralel)
        let d = describe(&q);
        assert!(d == "JUDET = \"Argeș\" SAU POP > 1000" || d == "JUDET = \"Argeș\" OR POP > 1000", "{d}");
    }
}
