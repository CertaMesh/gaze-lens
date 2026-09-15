//! Query DTOs only. Authoritative schema resolution, normalization and compilation
//! stay server-owned. Restored strings remain strings, including numeric text.
use crate::{Error, Result, bounds};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

pub enum Scalar {
    String(String),
    Number(Box<RawValue>),
    Bool(bool),
    Null,
}
impl Serialize for Scalar {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::String(v) => v.serialize(s),
            Self::Number(v) => v.serialize(s),
            Self::Bool(v) => v.serialize(s),
            Self::Null => s.serialize_unit(),
        }
    }
}
impl<'de> Deserialize<'de> for Scalar {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(d)?;
        let s = raw.get();
        if s.starts_with('"') {
            serde_json::from_str(s)
                .map(Self::String)
                .map_err(|_| serde::de::Error::custom("invalid_request"))
        } else if s == "null" {
            Ok(Self::Null)
        } else if s == "true" || s == "false" {
            Ok(Self::Bool(s == "true"))
        } else if bounds::number(s.as_bytes()) {
            Ok(Self::Number(raw))
        } else {
            Err(serde::de::Error::custom("invalid_request"))
        }
    }
}
#[derive(Serialize)]
#[serde(untagged)]
pub enum Operand {
    Scalar(Scalar),
    List(Vec<Scalar>),
}
impl<'de> Deserialize<'de> for Operand {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(d)?;
        if raw.get().starts_with('[') {
            serde_json::from_str(raw.get()).map(Self::List)
        } else {
            serde_json::from_str(raw.get()).map(Self::Scalar)
        }
        .map_err(|_| serde::de::Error::custom("invalid_request"))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
    Like,
    IsNull,
    IsNotNull,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Combinator {
    And,
    Or,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Asc,
    Desc,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Predicate {
    pub col: String,
    pub op: Op,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_operand"
    )]
    pub val: Option<Operand>,
}
fn present_operand<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Operand>, D::Error> {
    Operand::deserialize(d).map(Some)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Order {
    pub col: String,
    pub dir: Direction,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub table: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub columns: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub r#where: Option<Vec<Predicate>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub where_combinator: Option<Combinator>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub order_by: Option<Vec<Order>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub limit: Option<usize>,
}
impl Query {
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(bounds::DEFAULT_ROWS)
    }
    pub fn validate(&self) -> Result<()> {
        bounds::identifier(&self.table)?;
        bounds::require(self.limit() > 0)?;
        bounds::cap(self.limit(), bounds::MAX_ROWS)?;
        if let Some(cols) = &self.columns {
            bounds::require(!cols.is_empty())?;
            bounds::cap(cols.len(), bounds::MAX_COLUMNS)?;
            let mut seen = std::collections::BTreeSet::new();
            for col in cols {
                bounds::identifier(col)?;
                bounds::require(seen.insert(col))?;
            }
        }
        let mut binds = 0;
        if let Some(predicates) = &self.r#where {
            bounds::cap(predicates.len(), bounds::MAX_PREDICATES)?;
            for p in predicates {
                bounds::identifier(&p.col)?;
                let check = |s: &Scalar| -> Result<()> {
                    match s {
                        Scalar::Null => Err(Error::InvalidRequest),
                        Scalar::String(s) => bounds::text(s),
                        Scalar::Number(n) => {
                            bounds::cap(n.get().len(), bounds::SCALAR_BYTES)?;
                            bounds::require(bounds::number(n.get().as_bytes()))
                        }
                        Scalar::Bool(_) => Ok(()),
                    }
                };
                match (p.op, &p.val) {
                    (Op::IsNull | Op::IsNotNull, None) => {}
                    (Op::In, Some(Operand::List(xs))) => {
                        bounds::require(!xs.is_empty())?;
                        for x in xs {
                            check(x)?;
                        }
                        binds += xs.len();
                    }
                    (Op::IsNull | Op::IsNotNull | Op::In, _) => return Err(Error::InvalidRequest),
                    (_, Some(Operand::Scalar(x))) => {
                        check(x)?;
                        binds += 1;
                    }
                    _ => return Err(Error::InvalidRequest),
                }
            }
        }
        bounds::cap(binds, bounds::MAX_BINDS)?;
        if let Some(order) = &self.order_by {
            bounds::cap(order.len(), bounds::MAX_ORDER)?;
            for o in order {
                bounds::identifier(&o.col)?;
            }
        }
        Ok(())
    }
}

fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}
