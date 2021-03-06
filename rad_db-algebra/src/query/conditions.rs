use crate::query::query_node::QueryNode;
use crate::wrapped_tuple::WrappedTuple;
use rad_db_structure::identifier::Identifier;
use rad_db_structure::tuple::Tuple;
use rad_db_types::Value;
use std::cmp::min;
use std::collections::HashSet;
use std::convert::TryFrom;
use std::iter::FromIterator;

#[derive(Debug, Clone)]
pub struct JoinCondition {
    left_id: Identifier,
    right_id: Identifier,
}

impl JoinCondition {
    pub fn new(left_id: Identifier, right_id: Identifier) -> Self {
        JoinCondition { left_id, right_id }
    }

    pub fn left_id(&self) -> &Identifier {
        &self.left_id
    }
    pub fn right_id(&self) -> &Identifier {
        &self.right_id
    }
}

#[derive(PartialEq, Debug, Clone)]
pub enum Operand {
    Id(Identifier),
    SignedNumber(i64),
    UnsignedNumber(u64),
    Float(f64),
    String(String),
    Char(char),
    Boolean(bool),
}

#[derive(PartialEq, Debug, Clone)]
pub enum ConditionOperation {
    Equals(Operand),
    Nequals(Operand),
    And(Box<ConditionOperation>, Box<Condition>),
    Or(Box<ConditionOperation>, Box<Condition>),
}

macro_rules! min_float {
    ($x:expr) => {
        $x
    };
    ($x:expr, $($rest:expr),+) => {
        {
            let temp = min_float!($($rest),*);
            if $x < temp {
                $x
            } else {
                temp
            }
        }
    };
    ($($x:expr),+,) => {
        min_float!($($x),*)
    };
}

/// This operation was invalid for some reason
#[derive(Debug)]
pub struct InvalidOperation;

impl ConditionOperation {
    fn selectivity(&self, max_tuples: usize) -> f64 {
        let ret = match self {
            ConditionOperation::Equals(_) => 1.0 / max_tuples as f64,
            ConditionOperation::Nequals(_) => 1.0 - 1.0 / max_tuples as f64,
            ConditionOperation::And(c, r) => c.selectivity(max_tuples) * r.selectivity(max_tuples),
            ConditionOperation::Or(c, r) => {
                min_float!(c.selectivity(max_tuples) + r.selectivity(max_tuples), 1.0)
            }
        };
        if ret.is_infinite() {
            if ret.is_sign_positive() {
                1.0
            } else {
                0.0
            }
        } else {
            ret
        }
    }

    fn relevant_fields(&self) -> HashSet<Identifier> {
        match &self {
            ConditionOperation::Equals(Operand::Id(id)) => HashSet::from_iter(vec![id.clone()]),
            ConditionOperation::Nequals(Operand::Id(id)) => HashSet::from_iter(vec![id.clone()]),
            ConditionOperation::And(left, more) => {
                let mut relevant = left.relevant_fields();
                relevant.extend(more.relevant_fields());
                relevant
            }
            ConditionOperation::Or(left, more) => {
                let mut relevant = left.relevant_fields();
                relevant.extend(more.relevant_fields());
                relevant
            }
            _ => HashSet::new(),
        }
    }

    fn evaluate_on(&self, compare: Value, tuple: &WrappedTuple) -> Result<bool, InvalidOperation> {
        match self {
            ConditionOperation::Equals(eq) => match eq {
                Operand::Id(id) => {
                    let right = &tuple[id];
                    Ok(&compare == right)
                }
                Operand::SignedNumber(signed) => {
                    let number = i64::try_from(compare).map_err(|_| InvalidOperation)?;
                    Ok(*signed == number)
                }
                Operand::UnsignedNumber(unsigned) => {
                    let number = u64::try_from(compare).map_err(|_| InvalidOperation)?;
                    Ok(*unsigned == number)
                }
                Operand::Float(f) => {
                    let number = f64::try_from(compare).map_err(|_| InvalidOperation)?;
                    Ok(*f == number)
                }
                Operand::String(_) => {}
                Operand::Boolean(_) => {}
            },
            ConditionOperation::Nequals(neq) => {}
            ConditionOperation::And(_, _) => {}
            ConditionOperation::Or(_, _) => {}
        }
    }
}

#[derive(PartialEq, Debug, Clone)]
pub struct Condition {
    base: Identifier,
    operation: ConditionOperation,
}

impl Condition {
    pub fn new<I: Into<Identifier>>(base: I, operation: ConditionOperation) -> Self {
        Condition {
            base: base.into(),
            operation,
        }
    }

    pub fn and(left: Self, right: Self) -> Self {
        let Condition { base, operation } = left;
        Condition::new(
            base,
            ConditionOperation::And(Box::new(operation), Box::new(right)),
        )
    }

    /// Splits a conditional from a list of and statements c<sub>1</sub> AND c_<sub>2</sub> AND ... AND c<sub>n</sub>
    /// into a list of Conditions c<sub>1</sub>, c<sub>2</sub>, ..., c<sub>n</sub>
    pub fn split_and(self) -> Vec<Self> {
        let mut ptr = self;
        let mut output = vec![];
        while let Self {
            base,
            operation: ConditionOperation::And(inner, next),
        } = ptr
        {
            let extracted = Condition::new(base, *inner);
            let flattened = extracted.split_and();
            output.extend(flattened);
            ptr = *next;
        }
        output.push(ptr);
        output
    }

    /// A heuristic that approximates how selective a condition is, where the lower the better
    pub fn selectivity(&self, max_tuples: usize) -> f64 {
        self.operation.selectivity(max_tuples)
    }

    /// Returns the relevant fields for the condition
    pub fn relevant_fields(&self) -> HashSet<Identifier> {
        let mut ret = HashSet::new();
        ret.insert(self.base.clone());
        ret.extend(self.operation.relevant_fields());
        ret
    }

    /// Tests whether this is a conjunction or not
    pub fn not_conjunction(&self) -> bool {
        match &self.operation {
            ConditionOperation::And(..) | ConditionOperation::Or(..) => false,
            _ => true,
        }
    }

    pub fn evaluate_on(&self, tuple: WrappedTuple) -> bool {
        let left_value = &tuple[&self.base];
        let right_value: Operand = {
            match &self.operation {
                ConditionOperation::Equals(eq) => eq.clone(),
                ConditionOperation::Nequals(neq) => neq.clone(),
                ConditionOperation::And(l, r) => {}
                ConditionOperation::Or(_, _) => {}
            }
        };
    }
}

impl<I: Into<Identifier>> From<I> for Operand {
    fn from(s: I) -> Self {
        Operand::Id(s.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and() {
        let base_case = Condition::new("id1", ConditionOperation::Equals(Operand::from("id2")));
        let copy = base_case.clone();
        let split = base_case.split_and();
        assert_eq!(split.len(), 1);
        assert_eq!(split[0], copy);
        let case2 = Condition::and(
            Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
            Condition::new("id2", ConditionOperation::Equals(Operand::from("id3"))),
        );
        let split = case2.split_and();
        assert_eq!(split.len(), 2);
        assert_eq!(
            split,
            vec![
                Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3")))
            ]
        );
        let case3 = Condition::and(
            Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
            Condition::and(
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3"))),
                Condition::new("id3", ConditionOperation::Equals(Operand::from("id4"))),
            ),
        );
        let split = case3.split_and();
        assert_eq!(split.len(), 3);
        assert_eq!(
            split,
            vec![
                Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3"))),
                Condition::new("id3", ConditionOperation::Equals(Operand::from("id4")))
            ]
        );
        /// First split
        let case4 = Condition::and(
            Condition::and(
                Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3"))),
            ),
            Condition::new("id3", ConditionOperation::Equals(Operand::from("id4"))),
        );
        let split = case4.split_and();
        assert_eq!(split.len(), 3);
        assert_eq!(
            split,
            vec![
                Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3"))),
                Condition::new("id3", ConditionOperation::Equals(Operand::from("id4")))
            ]
        );
        /// multi split
        let case5 = Condition::and(
            Condition::and(
                Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3"))),
            ),
            Condition::and(
                Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3"))),
            ),
        );
        let split = case5.split_and();
        assert_eq!(split.len(), 4);
        assert_eq!(
            split,
            vec![
                Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3"))),
                Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3")))
            ]
        );
        /// multi weird
        let case5 = Condition::and(
            Condition::and(
                Condition::and(
                    Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                    Condition::and(
                        Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                        Condition::new("id2", ConditionOperation::Equals(Operand::from("id3"))),
                    ),
                ),
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3"))),
            ),
            Condition::and(
                Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3"))),
            ),
        );
        let split = case5.split_and();
        assert_eq!(
            split,
            vec![
                Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3"))),
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3"))),
                Condition::new("id1", ConditionOperation::Equals(Operand::from("id2"))),
                Condition::new("id2", ConditionOperation::Equals(Operand::from("id3")))
            ]
        );
    }
}
