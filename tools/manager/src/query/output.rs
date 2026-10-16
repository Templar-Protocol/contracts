//! Output selection is independent of loading; relationship IDs distinguish absent branches.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Result};
use serde::Serialize;

use super::model::{self, Field, FieldPath, NodeKind, Relation, Value};

pub(super) type Row = BTreeMap<&'static str, Cell>;

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(super) enum Cell {
    Scalar(Option<Value>),
    Object(Row),
}

pub(super) fn fields(
    kind: NodeKind,
    selected: &[String],
    included: &[String],
    usd: bool,
) -> Result<Vec<FieldPath>> {
    let mut fields = if selected.is_empty() {
        model::default_fields(kind)
    } else {
        selected
            .iter()
            .map(|text| FieldPath::parse(kind, text))
            .collect::<Result<Vec<_>>>()?
    };
    for text in included {
        let path = FieldPath::parse(kind, &format!("{text}.id"))?;
        for prefix in path.ancestor_ids().skip(1) {
            for field in model::default_fields(prefix.endpoint_kind()) {
                let mut nested = prefix.clone();
                nested.field = field.field;
                fields.push(nested);
            }
        }
    }
    if usd {
        ensure!(kind == NodeKind::Market, "--usd requires market output");
        fields.extend([
            FieldPath::parse(kind, "borrowed_usd")?,
            FieldPath::parse(kind, "supplied_usd")?,
        ]);
    }
    let mut unique = BTreeSet::from([FieldPath::parse(kind, "id")?]);
    for path in fields {
        unique.extend(path.ancestor_ids());
        unique.insert(path);
    }
    Ok(unique.into_iter().collect())
}

pub(super) fn row(values: BTreeMap<FieldPath, Option<Value>>) -> Row {
    let mut result = Row::new();
    // FieldPath orders ancestor IDs before their descendants.
    for (path, value) in values {
        insert(&mut result, &path.relations, path.field, value);
    }
    result
}

fn insert(row: &mut Row, relations: &[Relation], field: Field, value: Option<Value>) {
    let Some((relation, rest)) = relations.split_first() else {
        row.insert(field.as_str(), Cell::Scalar(value));
        return;
    };
    if rest.is_empty() && field == Field::Id && value.is_none() {
        row.insert(relation.as_str(), Cell::Scalar(None));
        return;
    }
    if let Cell::Object(child) = row
        .entry(relation.as_str())
        .or_insert_with(|| Cell::Object(Row::new()))
    {
        insert(child, rest, field, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(values: &[(&str, Option<Value>)]) -> serde_json::Value {
        let values = values
            .iter()
            .map(|(path, value)| {
                (
                    FieldPath::parse(NodeKind::Market, path).unwrap(),
                    value.clone(),
                )
            })
            .collect();
        serde_json::to_value(row(values)).unwrap()
    }

    #[test]
    fn shared_relationships_nest_without_dotted_keys() {
        assert_eq!(
            render(&[
                ("id", Some(Value::Text("market.near".into()))),
                ("borrow_apr", Some(Value::Number("0.125".parse().unwrap()))),
                ("oracle.id", Some(Value::Text("oracle.near".into()))),
                (
                    "oracle.version",
                    Some(Value::Version("0.4.1".parse().unwrap()))
                ),
                ("oracle.governance.id", Some(Value::Text("gov.near".into()))),
                (
                    "oracle.governance.version",
                    Some(Value::Version("0.3.1".parse().unwrap()))
                ),
            ]),
            json!({
                "id": "market.near", "borrow_apr": "0.125",
                "oracle": {
                    "id": "oracle.near", "version": "0.4.1",
                    "governance": {"id": "gov.near", "version": "0.3.1"}
                }
            })
        );
    }

    #[test]
    fn absent_relationships_are_null_but_unavailable_fields_keep_the_object() {
        assert_eq!(
            render(&[
                ("id", Some(Value::Text("market.near".into()))),
                ("oracle.id", Some(Value::Text("oracle.near".into()))),
                ("oracle.version", None),
                ("oracle.governance.id", None),
                ("oracle.governance.version", None),
            ]),
            json!({"id": "market.near", "oracle": {
                "id": "oracle.near", "version": null, "governance": null
            }})
        );
        assert_eq!(
            render(&[
                ("id", Some(Value::Text("market.near".into()))),
                ("oracle.id", None),
                ("oracle.version", None),
                ("oracle.governance.id", None),
                ("oracle.governance.version", None),
            ]),
            json!({"id": "market.near", "oracle": null})
        );
    }
}
