use std::collections::HashMap;
use std::sync::Arc;

use super::*;
use crate::Error;
use crate::dimension::{AttrValue, DimensionKind};

/// regions (Total = Nord + Sued) × measures (Sales, Cost, Profit = Sales − Cost, Margin, Quantity,
/// Price, Revenue) with the base values of the plan's example rules.
fn model() -> Cube {
    let mut r = Dimension::new("regions", DimensionKind::Generic).edit();
    let total = r.add("Total").unwrap();
    let nord = r.add("Nord").unwrap();
    let sued = r.add("Sued").unwrap();
    r.add_edge(total, nord, 1.0).unwrap();
    r.add_edge(total, sued, 1.0).unwrap();
    r.set_attribute("currency", nord, AttrValue::Text("EUR".into()))
        .unwrap();
    let mut m = Dimension::new("measures", DimensionKind::Measures).edit();
    let profit = m.add("Profit").unwrap();
    let sales = m.add("Sales").unwrap();
    let cost = m.add("Cost").unwrap();
    m.add_edge(profit, sales, 1.0).unwrap();
    m.add_edge(profit, cost, -1.0).unwrap();
    for n in ["Margin", "Quantity", "Price", "Revenue"] {
        m.add(n).unwrap();
    }
    let mut c = Cube::new(
        "sales",
        vec![Arc::new(r.commit().unwrap()), Arc::new(m.commit().unwrap())],
    );
    for (a, b, v) in [
        ("Nord", "Sales", 200.0),
        ("Nord", "Cost", 150.0),
        ("Sued", "Sales", 0.0),
        ("Sued", "Cost", 40.0),
        ("Nord", "Quantity", 10.0),
        ("Nord", "Price", 3.0),
        ("Sued", "Quantity", 5.0),
        ("Sued", "Price", 4.0),
    ] {
        let addr = c.address(&[a, b]).unwrap();
        c.set_base(&addr, Some(v)).unwrap();
    }
    c
}

fn cell(c: &Cube, a: &[&str]) -> CellValue {
    c.cell(&c.address(a).unwrap(), None).unwrap()
}

fn num(c: &Cube, a: &[&str]) -> Option<f64> {
    cell(c, a).number()
}

#[test]
fn syntax_errors_carry_positions() {
    let mut c = model();
    let e = c.set_rules("['Margin'] = 1 +;").unwrap_err();
    assert!(
        matches!(&e, Error::Parse(m) if m.starts_with("line 1, column 17")),
        "{e}"
    );
    let e = c.set_rules("# comment\n['Margin'] 5;").unwrap_err();
    assert!(
        matches!(&e, Error::Parse(m) if m.starts_with("line 2")),
        "{e}"
    );
    assert!(c.set_rules("['Margin'] = 'unterminated").is_err());
    assert!(c.set_rules("X:['Margin'] = 1;").is_err());
}

#[test]
fn invalid_rules_are_marked_and_skipped() {
    let mut c = model();
    let report = c
        .set_rules("['Margin'] = 1;\n['Nothing'] = 2;\n['Price'] = FOO(1);\n['Revenue'] = ['Quantity'] * ['Price'];")
        .unwrap();
    let valid: Vec<bool> = report.iter().map(|r| r.valid).collect();
    assert_eq!(valid, [true, false, false, true]);
    assert!(report[1].message.as_ref().unwrap().contains("Nothing"));
    assert_eq!(num(&c, &["Nord", "Margin"]), Some(1.0));
}

#[test]
fn all_levels_rule_calculates_aggregates_itself() {
    let mut c = model();
    c.set_rules("['Margin'] = IF(['Sales'] = 0, NULL, ['Profit'] / ['Sales']);")
        .unwrap();
    assert_eq!(num(&c, &["Nord", "Margin"]), Some(0.25));
    assert_eq!(cell(&c, &["Sued", "Margin"]), CellValue::Empty); // Sales = 0 → NULL
    // Total: (200 − 190) / 200, calculated from the aggregates, not summed
    assert_eq!(num(&c, &["Total", "Margin"]), Some(0.05));
}

#[test]
fn base_rule_is_summed_in_aggregates() {
    let mut c = model();
    c.set_rules("N: ['Revenue'] = ['Quantity'] * ['Price'];")
        .unwrap();
    assert_eq!(num(&c, &["Nord", "Revenue"]), Some(30.0));
    assert_eq!(num(&c, &["Sued", "Revenue"]), Some(20.0));
    assert_eq!(num(&c, &["Total", "Revenue"]), Some(50.0));
}

#[test]
fn feeder_marks_cells_without_stored_value() {
    let mut c = model();
    // Revenue is never stored; the feeder tells that Quantity ≠ 0 means a Revenue cell exists
    c.set_rules("N: ['Revenue'] = ['Quantity'] * ['Price'] FEED ['Quantity'];")
        .unwrap();
    assert_eq!(num(&c, &["Total", "Revenue"]), Some(50.0));
    // no Quantity in any region → nothing fed
    let addr = c.address(&["Nord", "Quantity"]).unwrap();
    c.set_base(&addr, None).unwrap();
    let addr = c.address(&["Sued", "Quantity"]).unwrap();
    c.set_base(&addr, None).unwrap();
    assert_eq!(cell(&c, &["Total", "Revenue"]), CellValue::Empty);
}

#[test]
fn aggregate_rule_applies_to_aggregates_only() {
    let mut c = model();
    c.set_rules(
        "N: ['Revenue'] = ['Quantity'] * ['Price'];\nC: ['Price'] = ['Revenue'] / ['Quantity'];",
    )
    .unwrap();
    assert_eq!(num(&c, &["Nord", "Price"]), Some(3.0)); // base: stored
    assert_eq!(num(&c, &["Total", "Price"]), Some(50.0 / 15.0));
}

#[test]
fn first_matching_rule_wins_and_continue_passes_on() {
    let mut c = model();
    c.set_rules("['Margin'] = 1;\n['Margin'] = 2;").unwrap();
    assert_eq!(num(&c, &["Nord", "Margin"]), Some(1.0));
    c.set_rules("[regions:'Nord', 'Margin'] = IF(1, CONTINUE(), 7);\n['Margin'] = 2;")
        .unwrap();
    assert_eq!(num(&c, &["Nord", "Margin"]), Some(2.0));
    assert_eq!(num(&c, &["Sued", "Margin"]), Some(2.0));
}

#[test]
fn error_values() {
    let mut c = model();
    c.set_rules(
        "['Margin'] = 1 / 0;\n['Quantity'] = ['Missing'];\n['Price'] = SQRT(0 - 1);\n['Revenue'] = ['Revenue'];\n['Sales'] = ['Cost'] + [measures:!measures];",
    )
    .unwrap();
    assert_eq!(
        cell(&c, &["Nord", "Margin"]),
        CellValue::Error(CellError::Div0)
    );
    // rules with unknown members are invalid → skipped; Quantity stays a base value
    assert_eq!(num(&c, &["Nord", "Quantity"]), Some(10.0));
    assert_eq!(
        cell(&c, &["Nord", "Price"]),
        CellValue::Error(CellError::Err)
    );
    assert_eq!(
        cell(&c, &["Nord", "Revenue"]),
        CellValue::Error(CellError::Cycle)
    );
    // an error in a base cell reaches the aggregate
    assert_eq!(
        cell(&c, &["Total", "Margin"]),
        CellValue::Error(CellError::Div0)
    );
}

#[test]
fn computed_member_names_and_ref_errors() {
    let mut c = model();
    c.set_rules("['Margin'] = [measures:'Sa' + 'les'];")
        .unwrap_err(); // string concatenation is not supported
    c.set_rules(
        "['Margin'] = [measures:IF(!regions = 'Nord', 'Sales', 'Cost')];\n['Price'] = [measures:IF(!regions = 'Sued', 'Nope', 'Cost')];",
    )
    .unwrap();
    assert_eq!(num(&c, &["Nord", "Margin"]), Some(200.0));
    assert_eq!(num(&c, &["Sued", "Margin"]), Some(40.0));
    // a computed name that does not exist is a #REF at calculation time (a literal one marks the rule invalid)
    assert_eq!(
        cell(&c, &["Sued", "Price"]),
        CellValue::Error(CellError::Ref)
    );
    assert_eq!(num(&c, &["Nord", "Price"]), Some(150.0));
}

#[test]
fn desc_target_and_functions() {
    let mut c = model();
    c.set_rules(
        "[regions:DESC('Total'), 'Margin'] = ELEMLEVEL('regions') * 10 + ISLEAF('regions') + ROUND(2.567, 1) + MAX(1, 5, 3) + MIN(4, 2) + ABS(0 - 1);",
    )
    .unwrap();
    // Nord is a descendant of Total: 0*10 + 1 + 2.6 + 5 + 2 + 1
    assert!((num(&c, &["Nord", "Margin"]).unwrap() - 11.6).abs() < 1e-9);
    // Total itself is not a descendant, and Margin has no stored values to feed from → empty
    assert_eq!(cell(&c, &["Total", "Margin"]), CellValue::Empty);
}

#[test]
fn attr_and_cross_cube_lookup() {
    let mut c = model();
    let mut rates = Dimension::new("cur", DimensionKind::Generic).edit();
    rates.add("EUR").unwrap();
    rates.add("USD").unwrap();
    let rd = Arc::new(rates.commit().unwrap());
    let mut fx = Cube::new("fx", vec![rd.clone()]);
    fx.set_base(&[rd.member("EUR").unwrap()], Some(1.5))
        .unwrap();
    c.set_rules("[regions:'Nord', 'Margin'] = DB('fx', ATTR('regions', !regions, 'currency'));")
        .unwrap();
    let cubes: HashMap<String, Cube> = HashMap::from([("fx".to_string(), fx)]);
    let v = c
        .cell(&c.address(&["Nord", "Margin"]).unwrap(), Some(&cubes))
        .unwrap();
    assert_eq!(v, CellValue::Num(1.5));
    // without access to the other cube → #REF
    assert_eq!(
        cell(&c, &["Nord", "Margin"]),
        CellValue::Error(CellError::Ref)
    );
}

#[test]
fn rule_cells_are_locked_for_writing() {
    let mut c = model();
    c.set_rules("['Margin'] = 1;\nN: ['Revenue'] = ['Quantity'] * ['Price'];")
        .unwrap();
    let ctx = WriteContext::new("u");
    for cell in [
        ["Nord", "Margin"],
        ["Total", "Margin"],
        ["Nord", "Revenue"],
        ["Total", "Revenue"],
    ] {
        let addr = c.address(&cell).unwrap();
        let e = c.write(&ctx, &addr, Input::Set(5.0)).unwrap_err();
        assert!(
            matches!(e, Error::Denied(m) if m.contains("rule in line")),
            "{cell:?}"
        );
    }
    // other cells are still writable; an area spanning a rule cell is rejected
    let ok = c.address(&["Nord", "Sales"]).unwrap();
    c.write(&ctx, &ok, Input::Set(210.0)).unwrap();
    let all = c.address(&["Nord", "Profit"]).unwrap();
    assert!(c.write(&ctx, &all, Input::Set(1.0)).is_err()); // weighted anyway
}

#[test]
fn view_applies_rules_and_reports_errors() {
    let mut c = model();
    c.set_rules("['Margin'] = IF(['Sales'] = 0, 1 / 0, ['Profit'] / ['Sales']);")
        .unwrap();
    let rd = c.dimensions()[0].clone();
    let md = c.dimensions()[1].clone();
    let rows: Vec<_> = ["Nord", "Sued", "Total"]
        .iter()
        .map(|n| rd.member(n).unwrap())
        .collect();
    let cols: Vec<_> = ["Sales", "Margin"]
        .iter()
        .map(|n| md.member(n).unwrap())
        .collect();
    let g = c.view(0, &rows, 1, &cols, &[rows[0], cols[0]]).unwrap();
    assert_eq!(g.get(0, 0), Some(200.0));
    assert_eq!(g.get(0, 1), Some(0.25));
    assert_eq!(g.errors.get(&3), Some(&CellError::Div0)); // row 1 (Sued), column 1 (Margin) → index 1 * 2 + 1
    assert_eq!(g.get(1, 1), None);
    assert_eq!(g.get(2, 0), Some(200.0)); // unaffected cells keep the fast path
}

#[test]
fn rule_text_is_stored_and_replaced() {
    let mut c = model();
    let text = "# Marge\n['Margin'] = 1;";
    c.set_rules(text).unwrap();
    assert_eq!(c.rules_text(), text);
    c.set_rules("").unwrap();
    assert_eq!(cell(&c, &["Nord", "Margin"]), CellValue::Empty);
}
