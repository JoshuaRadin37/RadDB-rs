use crate::identifier::Identifier;
use crate::key::primary::PrimaryKeyDefinition;
use crate::relations::tuple_storage::TupleStorage;
use crate::relations::AsTypeList;
use crate::tuple::Tuple;
use rad_db_types::Type;
use std::iter::FromIterator;
use std::ops::{Deref, Index, Shr};

pub struct Relation {
    name: Identifier,
    attributes: Vec<(String, Type)>,
    primary_key: PrimaryKeyDefinition,
    backing_table: TupleStorage,
}

impl Relation {
    pub fn name(&self) -> &Identifier {
        &self.name
    }
    pub fn attributes(&self) -> &Vec<(String, Type)> {
        &self.attributes
    }
    pub fn primary_key(&self) -> &PrimaryKeyDefinition {
        &self.primary_key
    }

    pub fn len(&self) -> usize {
        unimplemented!()
    }

    pub fn get_at_index(&self, index: usize) -> &Tuple {
        unimplemented!()
    }

    pub fn get_relation_definition(&self) -> RelationDefinition {
        let mut ret = Vec::new();
        for (name, ty) in &self.attributes {
            let identifier = Identifier::with_parent(&self.name, name);
            ret.push((identifier, ty.clone()));
        }
        RelationDefinition::new(ret)
    }
}

impl AsTypeList for Relation {
    fn to_type_list(&self) -> Vec<Type> {
        self.attributes.iter().map(|(_, t)| t).cloned().collect()
    }
}

/// A structure representing the actual names and types of a relation
#[derive(Debug, Clone)]
pub struct RelationDefinition {
    attributes: Vec<(Identifier, Type)>,
}

impl RelationDefinition {
    pub fn new(attributes: Vec<(Identifier, Type)>) -> Self {
        RelationDefinition { attributes }
    }

    /// Gets the minimum number of id levels
    ///
    /// # Example
    /// A list of parent::child1, child2 would have a minimum of 1
    fn min_id_length(&self) -> usize {
        let mut min = None;
        for (id, _) in &self.attributes {
            match min {
                None => min = Some(id.len()),
                Some(old_len) => {
                    let len = id.len();
                    if old_len < len {
                        min = Some(len)
                    }
                }
            }
        }
        min.expect("Identifier can not have zero length, so this can't happen")
    }

    /// Checks if all the ids are of the same length
    fn all_id_len_same(&self) -> bool {
        let mut value = None;
        for (id, _) in &self.attributes {
            match value {
                None => value = Some(id.len()),
                Some(val) => {
                    if val != id.len() {
                        return false;
                    }
                }
            }
        }
        true
    }

    pub fn strip_highest_prefix(&self) -> Option<RelationDefinition> {
        if self.all_id_len_same() {
            let vec: Vec<_> = self
                .attributes
                .iter()
                .map(|(id, ty)| (id.strip_highest_parent(), ty.clone()))
                .filter_map(|(id, ty)| match id {
                    None => None,
                    Some(id) => Some((id, ty)),
                })
                .collect();
            if vec.is_empty() {
                None
            } else {
                Some(RelationDefinition::new(vec))
            }
        } else {
            let min = self.min_id_length();
            let vec: Vec<_> = self
                .attributes
                .iter()
                .map(|(id, ty)| {
                    if id.len() > min {
                        (id.strip_highest_parent(), ty.clone())
                    } else {
                        (Some(id.clone()), ty.clone())
                    }
                })
                .filter_map(|(id, ty)| match id {
                    None => None,
                    Some(id) => Some((id, ty)),
                })
                .collect();
            if vec.is_empty() {
                None
            } else {
                Some(RelationDefinition::new(vec))
            }
        }
    }

    pub fn identifier_iter(&self) -> impl IntoIterator<Item = &Identifier> {
        self.attributes.iter().map(|(id, _)| id)
    }

    pub fn len(&self) -> usize {
        self.attributes.len()
    }
}

impl FromIterator<(Identifier, Type)> for RelationDefinition {
    fn from_iter<T: IntoIterator<Item = (Identifier, Type)>>(iter: T) -> Self {
        Self::new(iter.into_iter().collect())
    }
}

impl FromIterator<(String, Type)> for RelationDefinition {
    fn from_iter<T: IntoIterator<Item = (String, Type)>>(iter: T) -> Self {
        Self::from_iter(iter.into_iter().map(|(id, v)| (Identifier::new(id), v)))
    }
}

impl Index<usize> for RelationDefinition {
    type Output = (Identifier, Type);

    fn index(&self, index: usize) -> &Self::Output {
        &self.attributes[index]
    }
}

impl Index<Identifier> for RelationDefinition {
    type Output = Type;

    fn index(&self, index: Identifier) -> &Self::Output {
        for (id, ty) in &self.attributes {
            if id == &index {
                return ty;
            }
        }
        panic!("Index \"{}\" out of bounds", index)
    }
}

impl Shr<usize> for RelationDefinition {
    type Output = RelationDefinition;

    fn shr(self, rhs: usize) -> Self::Output {
        let mut ret = self;
        for _ in 0..rhs {
            match ret.strip_highest_prefix() {
                None => {}
                Some(next) => ret = next,
            }
        }
        ret
    }
}

impl PartialEq for RelationDefinition {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }

        let mut zip = self
            .identifier_iter()
            .into_iter()
            .zip(other.identifier_iter());

        for (left, right) in zip {
            if left != right {
                return false;
            }
        }

        true
    }
}

impl IntoIterator for &RelationDefinition {
    type Item = Type;
    type IntoIter = <Vec<Type> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        let ret: Vec<_> = self.attributes.iter().map(|(_, ty)| ty.clone()).collect();
        ret.into_iter()
    }
}