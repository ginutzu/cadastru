//! Selectarea entităților unui strat după atribute, cu o expresie în stilul SQL WHERE
//! (`admin_unit_id = 123`, `status = 1 AND is_active = 1`). Câmpurile și valorile se pot
//! insera din liste; expresia este verificată pe măsură ce se scrie.

use super::App;
use super::layers::LayerKind;
use super::select::SelMode;
use cadastru::expr::{Expr, quote_field, quote_value};
use cadastru::i18n::{self, tr};
use cadastru::model::{FieldType, Value};
use eframe::egui::{self, RichText};
use std::collections::HashMap;

/// Cum se combină rezultatul cu selecția existentă a stratului.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    New,
    Add,
    Remove,
    Within,
}

impl Mode {
    const ALL: [Mode; 4] = [Mode::New, Mode::Add, Mode::Remove, Mode::Within];
    fn label(self) -> &'static str {
        match self {
            Mode::New => tr!("Selecție nouă"),
            Mode::Add => tr!("Adaugă la selecție"),
            Mode::Remove => tr!("Scoate din selecție"),
            Mode::Within => tr!("Selectează din selecția curentă"),
        }
    }
}

/// Valorile distincte ale unui câmp: (valoare, de câte ori apare).
struct Values {
    key: (u64, usize, usize),
    list: Vec<(Value, usize)>,
    truncated: bool,
}

/// Cel mult atâtea valori distincte în listă.
const MAX_VALUES: usize = 5000;

#[derive(Default)]
pub struct AttrSelDlg {
    pub open: bool,
    pub focus: bool,
    pub layer: Option<u64>,
    pub expr: String,
    pub mode: Mode,
    /// textele se compară exact (majuscule, diacritice)
    pub exact: bool,
    /// zoom pe rezultat
    pub zoom: bool,
    /// câmpul ales în listă (pentru valori)
    field: Option<usize>,
    values: Option<Values>,
    value_filter: String,
    /// mesajul ultimei operații: (reușită, text)
    msg: Option<(bool, String)>,
    /// expresiile folosite (cele mai noi primele)
    pub history: Vec<String>,
    /// expresia scrisă pentru fiecare strat
    per_layer: HashMap<u64, String>,
}

enum Act {
    Check,
    Apply,
    ClearLayer,
    Table,
}

/// Ce se inserează în expresie: textul și cu câte caractere se mută cursorul înapoi.
struct Insert(String, usize);

fn distinct(values: impl Iterator<Item = Value>) -> (Vec<(Value, usize)>, bool) {
    let mut map: HashMap<String, (Value, usize)> = HashMap::new();
    let mut truncated = false;
    for v in values {
        let k = match &v {
            Value::Null => "\u{0}null".to_string(),
            v => v.to_text(),
        };
        if let Some(e) = map.get_mut(&k) {
            e.1 += 1;
        } else if map.len() < MAX_VALUES {
            map.insert(k, (v, 1));
        } else {
            truncated = true;
        }
    }
    let mut list: Vec<(Value, usize)> = map.into_values().collect();
    list.sort_by(|(a, _), (b, _)| match (a, b) {
        (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
        (Value::Null, _) => std::cmp::Ordering::Less,
        (_, Value::Null) => std::cmp::Ordering::Greater,
        _ => match (a.as_f64(), b.as_f64(), matches!(a, Value::Text(_)) || matches!(b, Value::Text(_))) {
            (Some(x), Some(y), false) => x.total_cmp(&y),
            _ => cadastru::query::fold(&a.to_text()).cmp(&cadastru::query::fold(&b.to_text())),
        },
    });
    (list, truncated)
}

impl App {
    /// Deschide fereastra pentru stratul dat (sau cel activ).
    pub fn open_attr_select(&mut self, layer: Option<u64>) {
        let d = &mut self.dlg.attrsel;
        d.open = true;
        d.focus = true;
        let is_vec = |id: u64| self.layers.iter().any(|l| l.id == id && l.is_vector());
        let pick = layer
            .filter(|&id| is_vec(id))
            .or(self.selected_layer.filter(|&id| is_vec(id)))
            .or(d.layer.filter(|&id| is_vec(id)))
            .or_else(|| self.layers.iter().rev().find(|l| l.is_vector()).map(|l| l.id));
        if pick != d.layer {
            if let Some(old) = d.layer {
                d.per_layer.insert(old, std::mem::take(&mut d.expr));
            }
            d.expr = pick.and_then(|id| d.per_layer.get(&id).cloned()).unwrap_or_default();
            d.layer = pick;
            d.field = None;
            d.msg = None;
        }
    }

    pub fn attrsel_window(&mut self, ctx: &egui::Context) {
        if !self.dlg.attrsel.open {
            return;
        }
        let mut d = std::mem::take(&mut self.dlg.attrsel);
        if d.layer.is_some_and(|id| self.layer(id).is_none_or(|l| !l.is_vector())) {
            d.layer = None;
        }
        if d.layer.is_none() {
            d.layer = self.layers.iter().rev().find(|l| l.is_vector()).map(|l| l.id);
        }
        let vec_layers: Vec<(u64, String)> = self
            .layers
            .iter()
            .rev()
            .filter(|l| l.is_vector())
            .map(|l| (l.id, l.name.clone()))
            .collect();
        let edit_id = egui::Id::unique("attrsel_expr");
        let mut open = true;
        let mut act: Option<Act> = None;
        let mut insert: Option<Insert> = None;
        let mut new_layer: Option<u64> = None;
        let layer = d.layer.and_then(|id| self.layer(id));
        egui::Window::new(tr!("Selectare după atribute"))
            .id(egui::Id::unique("win_attrsel"))
            .open(&mut open)
            .default_width(620.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr!("Strat:"));
                    let cur = layer.map(|l| l.name.clone()).unwrap_or_else(|| tr!("(niciun strat vectorial)").into());
                    egui::ComboBox::from_id_salt("attrsel_layer")
                        .selected_text(cur)
                        .width(320.0)
                        .show_ui(ui, |ui| {
                            for (id, n) in &vec_layers {
                                if ui.selectable_label(d.layer == Some(*id), n).clicked() {
                                    new_layer = Some(*id);
                                }
                            }
                        });
                    if let Some(LayerKind::Vector(v)) = layer.map(|l| &l.kind) {
                        let n = v.data.features.len();
                        let s = v.sel.len();
                        ui.label(
                            RichText::new(tr!("{} · selectate {}", i18n::count(n, "entitate", "entități", "feature", "features"), s))
                                .weak(),
                        );
                    }
                });
                let Some(LayerKind::Vector(v)) = layer.map(|l| &l.kind) else {
                    ui.label(tr!("Deschideți un strat vectorial."));
                    return;
                };
                let data = &v.data;
                // câmpuri și valori
                ui.columns(2, |cols| {
                    let ui = &mut cols[0];
                    ui.label(RichText::new(tr!("Câmpuri (dublu-click = inserează)")).strong());
                    egui::ScrollArea::vertical()
                        .id_salt("attrsel_fields")
                        .max_height(170.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (i, f) in data.fields.iter().enumerate() {
                                let r = ui
                                    .selectable_label(
                                        d.field == Some(i),
                                        format!("{}   ({})", f.name, f.ftype.label()),
                                    )
                                    .on_hover_text(tr!("Click = valorile câmpului, dublu-click = inserează numele în expresie"));
                                if r.clicked() {
                                    d.field = Some(i);
                                }
                                if r.double_clicked() {
                                    insert = Some(Insert(format!("{} ", quote_field(&f.name)), 0));
                                }
                            }
                        });
                    let ui = &mut cols[1];
                    let Some(fi) = d.field.filter(|&f| f < data.fields.len()) else {
                        ui.label(RichText::new(tr!("Valori")).strong());
                        ui.label(RichText::new(tr!("Alegeți un câmp din listă pentru a-i vedea valorile.")).weak());
                        return;
                    };
                    let key = (v.data.features.len(), fi, data.fields.len());
                    let key = (d.layer.unwrap_or(0), key.1, key.0 ^ (key.2 << 40));
                    if d.values.as_ref().is_none_or(|c| c.key != key) {
                        let (list, truncated) = distinct(
                            data.features.iter().map(|f| f.attrs.get(fi).cloned().unwrap_or(Value::Null)),
                        );
                        d.values = Some(Values { key, list, truncated });
                    }
                    let vals = d.values.as_ref().expect("valori");
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(tr!("Valori „{}”: {}", data.fields[fi].name, vals.list.len())).strong(),
                        );
                        if vals.truncated {
                            ui.label(RichText::new(tr!("(primele {})", MAX_VALUES)).weak());
                        }
                    });
                    ui.add(
                        egui::TextEdit::singleline(&mut d.value_filter)
                            .hint_text(tr!("filtrează valorile"))
                            .desired_width(f32::INFINITY),
                    );
                    let q = cadastru::query::fold(&d.value_filter);
                    egui::ScrollArea::vertical()
                        .id_salt("attrsel_values")
                        .max_height(146.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            let mut shown = 0;
                            for (val, n) in &vals.list {
                                let t = match val {
                                    Value::Null => "NULL".to_string(),
                                    v => v.to_text(),
                                };
                                if !q.is_empty() && !cadastru::query::fold(&t).contains(&q) {
                                    continue;
                                }
                                shown += 1;
                                if shown > 1000 {
                                    ui.label(RichText::new(tr!("… (restrângeți cu filtrul)")).weak());
                                    break;
                                }
                                let r = ui
                                    .selectable_label(false, format!("{}   ×{}", if t.is_empty() { "''".into() } else { t }, n))
                                    .on_hover_text(tr!("Click = inserează valoarea; dublu-click = inserează „câmp = valoare”"));
                                // valoare text într-un câmp numeric de tip text: scrisă ca text
                                let lit = match (val, data.fields[fi].ftype) {
                                    (Value::Text(_), _) | (_, FieldType::Text | FieldType::Date) if !val.is_null() => {
                                        format!("'{}'", val.to_text().replace('\'', "''"))
                                    }
                                    _ => quote_value(val),
                                };
                                if r.double_clicked() {
                                    let f = quote_field(&data.fields[fi].name);
                                    // primul click a inserat deja valoarea: se înlocuiește
                                    insert = Some(Insert(
                                        if val.is_null() { format!("{f} IS NULL ") } else { format!("{f} = {lit} ") },
                                        usize::MAX,
                                    ));
                                } else if r.clicked() {
                                    insert = Some(Insert(
                                        if val.is_null() { "NULL ".into() } else { format!("{lit} ") },
                                        0,
                                    ));
                                }
                            }
                        });
                });
                ui.add_space(2.0);
                // operatori
                ui.horizontal_wrapped(|ui| {
                    let ops: [(&str, &str, usize); 16] = [
                        ("=", " = ", 0),
                        ("<>", " <> ", 0),
                        ("<", " < ", 0),
                        ("<=", " <= ", 0),
                        (">", " > ", 0),
                        (">=", " >= ", 0),
                        ("LIKE", " LIKE '%'", 2),
                        ("IN (…)", " IN ()", 1),
                        ("BETWEEN", " BETWEEN  AND ", 5),
                        ("IS NULL", " IS NULL ", 0),
                        ("IS NOT NULL", " IS NOT NULL ", 0),
                        ("AND", " AND ", 0),
                        ("OR", " OR ", 0),
                        ("NOT", "NOT ", 0),
                        ("( )", "()", 1),
                        ("%", "%", 0),
                    ];
                    for (lbl, text, back) in ops {
                        if ui.button(RichText::new(lbl).monospace()).clicked() {
                            insert = Some(Insert(text.to_string(), back));
                        }
                    }
                });
                // expresia
                let mut te = egui::TextEdit::multiline(&mut d.expr)
                    .id(edit_id)
                    .code_editor()
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("status = 1 AND is_active = 1");
                if ui.visuals().dark_mode {
                    te = te.text_color(egui::Color32::from_rgb(230, 230, 230));
                }
                let r = ui.add(te);
                if d.focus {
                    r.request_focus();
                    d.focus = false;
                }
                if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command) {
                    act = Some(Act::Apply);
                }
                // verificarea pe loc
                let check = (!d.expr.trim().is_empty()).then(|| Expr::compile(&d.expr, data, d.exact));
                match &check {
                    Some(Err(e)) => {
                        ui.colored_label(ui.visuals().error_fg_color, e.to_string());
                    }
                    Some(Ok(_)) => {
                        ui.label(
                            RichText::new(tr!("Expresie corectă. Ctrl+Enter = selectează."))
                                .color(egui::Color32::from_rgb(80, 170, 90)),
                        );
                    }
                    None => {
                        ui.label(
                            RichText::new(tr!(
                                "Exemple: admin_unit_id = 123 · status = 1 AND is_active = 1 · nume LIKE 'Pit%' · cod IN (1, 2, 3) · supr BETWEEN 100 AND 500 · obs IS NULL"
                            ))
                            .weak(),
                        );
                    }
                }
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("attrsel_mode")
                        .selected_text(d.mode.label())
                        .show_ui(ui, |ui| {
                            for m in Mode::ALL {
                                ui.selectable_value(&mut d.mode, m, m.label());
                            }
                        });
                    ui.checkbox(&mut d.exact, tr!("Exact (majuscule și diacritice)"))
                        .on_hover_text(tr!("Implicit „Argeș” = „ARGES”; bifat, textele trebuie să fie identice"));
                    ui.checkbox(&mut d.zoom, tr!("Zoom pe rezultat"));
                });
                ui.horizontal(|ui| {
                    let ok = matches!(check, Some(Ok(_)));
                    if ui
                        .add_enabled(ok, egui::Button::new(RichText::new(tr!("Selectează")).strong()))
                        .clicked()
                    {
                        act = Some(Act::Apply);
                    }
                    if ui.add_enabled(ok, egui::Button::new(tr!("Verifică"))).on_hover_text(tr!("Numără entitățile potrivite, fără să schimbe selecția")).clicked() {
                        act = Some(Act::Check);
                    }
                    if ui.add_enabled(!d.expr.is_empty(), egui::Button::new(tr!("Golește expresia"))).clicked() {
                        d.expr.clear();
                        d.msg = None;
                    }
                    ui.separator();
                    if ui.add_enabled(!v.sel.is_empty(), egui::Button::new(tr!("Deselectează"))).clicked() {
                        act = Some(Act::ClearLayer);
                    }
                    if ui.button(tr!("Tabel")).on_hover_text(tr!("Tabelul de atribute al stratului, doar cu entitățile selectate")).clicked() {
                        act = Some(Act::Table);
                    }
                    ui.add_enabled_ui(!d.history.is_empty(), |ui| {
                        ui.menu_button(tr!("Recente…"), |ui| {
                            for h in &d.history {
                                let t: String = h.chars().take(90).collect();
                                if ui.button(RichText::new(t).monospace()).clicked() {
                                    insert = Some(Insert(h.clone(), usize::MAX - 1));
                                    ui.close();
                                }
                            }
                        });
                    });
                });
                if let Some((ok, m)) = &d.msg {
                    let c = if *ok { ui.visuals().text_color() } else { ui.visuals().error_fg_color };
                    ui.colored_label(c, m);
                }
            });
        if let Some(id) = new_layer.filter(|&id| Some(id) != d.layer) {
            if let Some(old) = d.layer {
                d.per_layer.insert(old, std::mem::take(&mut d.expr));
            }
            d.expr = d.per_layer.get(&id).cloned().unwrap_or_default();
            d.layer = Some(id);
            d.field = None;
            d.msg = None;
        }
        if let Some(Insert(text, back)) = insert {
            insert_text(ctx, edit_id, &mut d.expr, &text, back);
        }
        match act {
            Some(Act::Check) => self.attrsel_run(&mut d, true, ctx, edit_id),
            Some(Act::Apply) => self.attrsel_run(&mut d, false, ctx, edit_id),
            Some(Act::ClearLayer) => {
                if let Some(id) = d.layer {
                    self.selection_action(id, "clear");
                    d.msg = None;
                }
            }
            Some(Act::Table) => {
                if let Some(id) = d.layer {
                    self.dlg.table.open = true;
                    self.dlg.table.layer_id = Some(id);
                    self.dlg.table.only_sel = true;
                }
            }
            None => {}
        }
        d.open = open;
        self.dlg.attrsel = d;
    }

    /// Aplică expresia (sau doar numără entitățile potrivite).
    fn attrsel_run(&mut self, d: &mut AttrSelDlg, only_count: bool, ctx: &egui::Context, edit_id: egui::Id) {
        let Some(id) = d.layer else { return };
        let Some(l) = self.layer(id) else { return };
        let name = l.name.clone();
        let LayerKind::Vector(v) = &l.kind else { return };
        let e = match Expr::compile(&d.expr, &v.data, d.exact) {
            Ok(e) => e,
            Err(e) => {
                // cursorul la locul greșelii
                if let Some(mut st) = egui::TextEdit::load_state(ctx, edit_id) {
                    let c = egui::text::CCursor::new(e.pos);
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::one(c)));
                    st.store(ctx, edit_id);
                }
                ctx.memory_mut(|m| m.request_focus(edit_id));
                d.msg = Some((false, e.to_string()));
                return;
            }
        };
        let hits = e.run(&v.data);
        let total = v.data.features.len();
        let tot = i18n::count(total, "entitate", "entități", "feature", "features");
        if only_count {
            d.msg = Some((true, tr!("Expresia este îndeplinită de {} din {}.", hits.len(), tot)));
            return;
        }
        let expr = d.expr.trim().to_string();
        d.history.retain(|h| h != &expr);
        d.history.insert(0, expr);
        d.history.truncate(20);
        let nhits = hits.len();
        if let Some(LayerKind::Vector(v)) = self.layer_mut(id).map(|l| &mut l.kind) {
            match d.mode {
                Mode::New => v.apply_sel(hits, SelMode::Replace),
                Mode::Add => v.apply_sel(hits, SelMode::Add),
                Mode::Remove => v.apply_sel(hits, SelMode::Remove),
                Mode::Within => {
                    let keep: Vec<usize> = hits.into_iter().filter(|i| v.sel.contains(i)).collect();
                    v.apply_sel(keep, SelMode::Replace);
                }
            }
        }
        let nsel = match self.layer(id).map(|l| &l.kind) {
            Some(LayerKind::Vector(v)) => v.sel.len(),
            _ => 0,
        };
        let sel = i18n::count(nsel, "entitate selectată", "entități selectate", "feature selected", "features selected");
        let msg = tr!("{} potrivite din {}; în „{}”: {}.", nhits, tot, name, sel);
        self.status = msg.clone();
        d.msg = Some((true, msg));
        if d.zoom && nsel > 0 {
            self.zoom_to_selection(Some(id));
        }
    }
}

/// Inserează textul la cursor (înlocuind textul marcat). `back` = cu câte caractere se mută
/// cursorul înapoi după inserare; `usize::MAX` = înlocuiește ultima valoare inserată din listă
/// (dublu-click), `usize::MAX - 1` = înlocuiește toată expresia.
fn insert_text(ctx: &egui::Context, id: egui::Id, s: &mut String, text: &str, back: usize) {
    let mut st = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let n = s.chars().count();
    let (mut a, mut b) = st
        .cursor
        .char_range()
        .map(|r| {
            let x = r.as_sorted_char_range();
            (x.start.0.min(n), x.end.0.min(n))
        })
        .unwrap_or((n, n));
    let mut back = back;
    if back == usize::MAX - 1 {
        a = 0;
        b = n;
        back = 0;
    } else if back == usize::MAX {
        // dublu-click pe o valoare: primul click a inserat deja valoarea; se scoate
        back = 0;
        if let Some(prev) = LAST.with(|l| l.borrow().clone())
            && a >= prev.1
            && s.chars().skip(a - prev.1).take(prev.1).collect::<String>() == prev.0
        {
            a -= prev.1;
        }
    }
    // spațiu înaintea textului, dacă e lipit de cuvântul anterior
    let chars: Vec<char> = s.chars().collect();
    let mut text = text.to_string();
    if a > 0
        && !chars[a - 1].is_whitespace()
        && chars[a - 1] != '('
        && !text.starts_with(' ')
        && !text.starts_with(')')
        && text != "%"
    {
        text.insert(0, ' ');
    }
    // fără spațiu dublu la început
    if text.starts_with(' ') && (a == 0 || chars[a - 1].is_whitespace()) {
        text.remove(0);
    }
    let before: String = chars[..a].iter().collect();
    let after: String = chars[b..].iter().collect();
    *s = format!("{before}{text}{after}");
    let pos = a + text.chars().count() - back.min(text.chars().count());
    LAST.with(|l| *l.borrow_mut() = Some((text.clone(), text.chars().count())));
    st.cursor
        .set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(pos))));
    st.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
}

thread_local! {
    /// ultimul text inserat din listă (pentru înlocuirea la dublu-click)
    static LAST: std::cell::RefCell<Option<(String, usize)>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_sorted() {
        let (l, t) = distinct(
            [Value::Int(10), Value::Int(2), Value::Null, Value::Int(2), Value::Int(1)].into_iter(),
        );
        assert!(!t);
        let v: Vec<String> = l.iter().map(|(v, n)| format!("{}:{n}", v.to_text())).collect();
        assert_eq!(v, [":1", "1:1", "2:2", "10:1"]);
    }
}
