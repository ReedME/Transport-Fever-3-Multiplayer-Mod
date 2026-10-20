//! Actions as the Lua mod reads and writes them: plain tables in the game's
//! own units, converted here to and from [`Action`], the one schema.
//!
//! The mod hands the hook a captured action as a table
//! (`tpf3mp_native.command`), and the hook hands the mod each action to
//! apply as a table (`handlers.apply`). The hook reads and writes those
//! tables in the game's Lua state as a [`LuaValue`] tree, and this module
//! converts the tree. So the schema, its bounds and the rounding of the
//! game's floats are defined once, in Rust; the mod has no encoder of its
//! own (docs/HOOKS.md, "The Lua side").
//!
//! The tables mirror the Rust types field for field, as serde names them:
//!
//! - a struct is a table of its fields by their Rust names; an `Option`
//!   that is `None` is a field left out;
//! - a list, or a fixed-size array, is a sequence `{ a, b, c }`;
//! - an enum value is its variant's name as a string when it carries
//!   nothing (`"Ground"`), and a one-entry table when it does
//!   (`{ Bridge = "bridge/cement.lua" }`, `{ Edges = { network = ..., edges = ... } }`);
//! - an id (`VehicleId`, ...) is its number.
//!
//! Numbers are in the game's units: positions, tangents, a terrain cell's
//! heights and a terrain grid's cell size in metres; unit directions and a
//! transform's basis as plain fractions; a `Fixed` construction parameter
//! as the number itself. They become the schema's integers (millimetres and
//! millionths) by rounding to the nearest, halves away from zero. Anything
//! else must be a whole number. What does not fit is refused, never
//! clamped: not a finite number, out of the field's range, a fraction where
//! a whole number goes, a field the type does not have, a missing field, a
//! string that is not UTF-8 or a table that is not a sequence where one is
//! expected. Errors name the path to the bad value (`polyline.vertices[2].pos.x`).
//!
//! Building the tree from a Lua state is the hook's part; [`MAX_DEPTH`] and
//! [`MAX_NODES`] are the bounds it must walk within.

use std::fmt;

use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeSeed, IntoDeserializer, Visitor},
    ser,
};
use thiserror::Error;

use crate::action::Action;

/// The deepest table nesting any action needs is under 10; the hook stops
/// reading a table deeper than this.
pub const MAX_DEPTH: usize = 16;
/// Most values in one action's table tree. The largest action, a terraform
/// of 8,192 cells, is about 25,000.
pub const MAX_NODES: usize = 65_536;

/// A Lua value, as the hook reads it from the game's Lua state or writes it
/// there. Tables keep their entries in any order; a Lua table cannot hold a
/// nil key or value, so `Nil` appears only at the top.
#[derive(Debug, Clone, PartialEq)]
pub enum LuaValue {
    Nil,
    Boolean(bool),
    /// A Lua number: a double in Lua 5.1 and 5.2.
    Number(f64),
    /// A Lua 5.3 integer, should the game's Lua have them.
    Integer(i64),
    /// A Lua string: bytes, which must be UTF-8 where text is expected.
    String(Vec<u8>),
    Table(Vec<(LuaValue, LuaValue)>),
}

impl LuaValue {
    pub fn string(text: &str) -> Self {
        Self::String(text.as_bytes().to_vec())
    }

    /// The value at string key `key`, if this is a table that has one.
    pub fn get(&self, key: &str) -> Option<&LuaValue> {
        match self {
            Self::Table(entries) => entries
                .iter()
                .find(|(k, _)| matches!(k, Self::String(s) if s == key.as_bytes()))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Nil => "nil",
            Self::Boolean(_) => "a boolean",
            Self::Number(_) | Self::Integer(_) => "a number",
            Self::String(_) => "a string",
            Self::Table(_) => "a table",
        }
    }
}

/// Why a table is not an action, or an action cannot be a table, and where.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub struct LuaError {
    /// Innermost first: segments are prepended as the error unwinds.
    path: Vec<String>,
    message: String,
}

impl LuaError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            path: Vec::new(),
            message: message.into(),
        }
    }

    fn within(mut self, segment: String) -> Self {
        self.path.push(segment);
        self
    }

    /// The path to the bad value, such as `polyline.vertices[2].pos.x`.
    pub fn path(&self) -> String {
        let mut out = String::new();
        for segment in self.path.iter().rev() {
            if !segment.starts_with('[') && !out.is_empty() {
                out.push('.');
            }
            out.push_str(segment);
        }
        out
    }
}

impl fmt::Display for LuaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path(), self.message)
        }
    }
}

impl de::Error for LuaError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::new(msg.to_string())
    }
}

impl ser::Error for LuaError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::new(msg.to_string())
    }
}

/// An action from the mod's table.
pub fn action_from_lua(value: &LuaValue) -> Result<Action, LuaError> {
    let action = Action::deserialize(De {
        value,
        scale: Scale::One,
    })?;
    action
        .validate()
        .map_err(|error| LuaError::new(error.to_string()))?;
    Ok(action)
}

/// An action as the table the mod applies. Fails only for a number Lua
/// cannot hold exactly (a construction parameter beyond 2^53).
pub fn action_to_lua(action: &Action) -> Result<LuaValue, LuaError> {
    action.serialize(Ser { scale: Scale::One })
}

// ---------- units ----------

/// How many schema units one game unit is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scale {
    /// Whole numbers, as they are.
    One,
    /// Metres to millimetres.
    Milli,
    /// Fractions to millionths.
    Micro,
}

impl Scale {
    fn factor(self) -> f64 {
        match self {
            Self::One => 1.0,
            Self::Milli => 1_000.0,
            Self::Micro => 1_000_000.0,
        }
    }
}

/// The unit of a field, by the struct it is in (docs in `action.rs`).
/// `TerraformFields` is the name `Terraform` deserializes through.
fn field_scale(owner: &str, field: &str) -> Scale {
    match (owner, field) {
        ("Pos" | "Pos2" | "Tangent" | "TerrainCell", _) => Scale::Milli,
        ("Lane", "speed" | "width" | "height" | "offset") => Scale::Milli,
        ("TrafficPhase", "duration" | "minimum") => Scale::Milli,
        ("Terraform" | "TerraformFields", "cell") => Scale::Milli,
        ("UnitDir" | "Tint", _)
        | ("Transform", "basis")
        | ("LoanTerms", "percentage")
        | ("LineStop", "min_wait" | "max_wait" | "max_extra_wait")
        | ("LineData", "reservation_priority") => Scale::Micro,
        _ => Scale::One,
    }
}

/// The unit of an enum variant's value.
fn variant_scale(owner: &str, variant: &str) -> Scale {
    match (owner, variant) {
        ("ParamValue", "Fixed") => Scale::Micro,
        _ => Scale::One,
    }
}

/// The largest integer a double holds exactly.
const EXACT: f64 = 9_007_199_254_740_992.0;

// ---------- table -> action ----------

struct De<'a> {
    value: &'a LuaValue,
    scale: Scale,
}

impl De<'_> {
    fn expected(&self, what: &str) -> LuaError {
        LuaError::new(format!("expected {what}, found {}", self.value.kind()))
    }

    /// The value as a schema integer: scaled and rounded, or whole.
    fn integer(&self) -> Result<i128, LuaError> {
        let number = match *self.value {
            LuaValue::Integer(n) if self.scale == Scale::One => return Ok(i128::from(n)),
            LuaValue::Integer(n) => n as f64,
            LuaValue::Number(n) => n,
            _ => return Err(self.expected("a number")),
        };
        if !number.is_finite() {
            return Err(LuaError::new(format!("not a finite number: {number}")));
        }
        let scaled = number * self.scale.factor();
        if self.scale == Scale::One && scaled.fract() != 0.0 {
            return Err(LuaError::new(format!("not a whole number: {number}")));
        }
        let rounded = scaled.round();
        if rounded.abs() > EXACT {
            return Err(LuaError::new(format!("out of range: {number}")));
        }
        Ok(rounded as i128)
    }

    fn ranged<T: TryFrom<i128>>(&self) -> Result<T, LuaError> {
        let n = self.integer()?;
        T::try_from(n).map_err(|_| {
            LuaError::new(format!(
                "out of range for {}: {n}",
                std::any::type_name::<T>()
            ))
        })
    }

    fn table(&self) -> Result<&[(LuaValue, LuaValue)], LuaError> {
        match self.value {
            LuaValue::Table(entries) => Ok(entries),
            _ => Err(self.expected("a table")),
        }
    }

    /// The table's values in order, if its keys are exactly 1..=n.
    fn sequence(&self) -> Result<Vec<&LuaValue>, LuaError> {
        let entries = self.table()?;
        let mut items: Vec<Option<&LuaValue>> = vec![None; entries.len()];
        for (key, value) in entries {
            let index = match *key {
                LuaValue::Integer(n) => usize::try_from(n).ok(),
                LuaValue::Number(n) if n.fract() == 0.0 && (1.0..=EXACT).contains(&n) => {
                    Some(n as usize)
                }
                _ => None,
            };
            let slot = index
                .filter(|&i| i >= 1 && i <= entries.len())
                .and_then(|i| items.get_mut(i - 1))
                .ok_or_else(|| LuaError::new("expected a sequence { a, b, ... }"))?;
            if slot.replace(value).is_some() {
                return Err(LuaError::new("a sequence index given twice"));
            }
        }
        items
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| LuaError::new("expected a sequence { a, b, ... }"))
    }

    fn string(&self) -> Result<String, LuaError> {
        match self.value {
            LuaValue::String(bytes) => String::from_utf8(bytes.clone())
                .map_err(|_| LuaError::new("a string that is not UTF-8")),
            _ => Err(self.expected("a string")),
        }
    }
}

macro_rules! integers {
    ($($method:ident $visit:ident $ty:ty;)*) => {
        $(fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, LuaError> {
            visitor.$visit(self.ranged::<$ty>()?)
        })*
    };
}

impl<'de> de::Deserializer<'de> for De<'_> {
    type Error = LuaError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, LuaError> {
        match self.value {
            LuaValue::Nil => visitor.visit_unit(),
            LuaValue::Boolean(b) => visitor.visit_bool(*b),
            LuaValue::Number(_) | LuaValue::Integer(_) => visitor.visit_i64(self.ranged()?),
            LuaValue::String(_) => visitor.visit_string(self.string()?),
            LuaValue::Table(_) => Err(LuaError::new("a table where no type says what it is")),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, LuaError> {
        match self.value {
            LuaValue::Boolean(b) => visitor.visit_bool(*b),
            _ => Err(self.expected("a boolean")),
        }
    }

    integers! {
        deserialize_i8 visit_i8 i8;
        deserialize_i16 visit_i16 i16;
        deserialize_i32 visit_i32 i32;
        deserialize_i64 visit_i64 i64;
        deserialize_u8 visit_u8 u8;
        deserialize_u16 visit_u16 u16;
        deserialize_u32 visit_u32 u32;
        deserialize_u64 visit_u64 u64;
    }

    fn deserialize_f32<V: Visitor<'de>>(self, _: V) -> Result<V::Value, LuaError> {
        Err(LuaError::new("the schema has no floats"))
    }

    fn deserialize_f64<V: Visitor<'de>>(self, _: V) -> Result<V::Value, LuaError> {
        Err(LuaError::new("the schema has no floats"))
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, LuaError> {
        self.deserialize_string(visitor)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, LuaError> {
        self.deserialize_string(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, LuaError> {
        visitor.visit_string(self.string()?)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, _: V) -> Result<V::Value, LuaError> {
        Err(LuaError::new("the schema has no raw bytes"))
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, _: V) -> Result<V::Value, LuaError> {
        Err(LuaError::new("the schema has no raw bytes"))
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, LuaError> {
        match self.value {
            LuaValue::Nil => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, LuaError> {
        match self.value {
            LuaValue::Nil => visitor.visit_unit(),
            _ => Err(self.expected("nothing")),
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, LuaError> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, LuaError> {
        // A Fraction is in millionths wherever it stands.
        let scale = if name == "Fraction" {
            Scale::Micro
        } else {
            self.scale
        };
        visitor.visit_newtype_struct(De { scale, ..self })
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, LuaError> {
        visitor.visit_seq(Seq::new(self.sequence()?, Scale::One))
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, LuaError> {
        let items = self.sequence()?;
        if items.len() != len {
            return Err(LuaError::new(format!(
                "expected {len} values, found {}",
                items.len()
            )));
        }
        // A fixed-size array takes its unit from the field that holds it.
        visitor.visit_seq(Seq::new(items, self.scale))
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, LuaError> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, _: V) -> Result<V::Value, LuaError> {
        Err(LuaError::new("the schema has no maps"))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, LuaError> {
        let mut found: Vec<(&'static str, &LuaValue)> = Vec::new();
        for (key, value) in self.table()? {
            let key = match key {
                LuaValue::String(bytes) => std::str::from_utf8(bytes).ok(),
                _ => None,
            };
            let Some(field) = key.and_then(|k| fields.iter().find(|f| **f == k)) else {
                let shown = key.map_or_else(|| "a non-string key".to_owned(), str::to_owned);
                return Err(LuaError::new(format!("{name} has no field {shown}")));
            };
            if found.iter().any(|(f, _)| f == field) {
                return Err(LuaError::new(format!("field {field} given twice")));
            }
            found.push((field, value));
        }
        // In declaration order, so errors come out the same every time.
        found.sort_by_key(|(field, _)| fields.iter().position(|f| f == field));
        visitor.visit_map(Fields {
            owner: name,
            entries: found.into_iter(),
            pending: None,
        })
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, LuaError> {
        match self.value {
            LuaValue::String(_) => visitor.visit_enum(Variant {
                owner: name,
                variant: self.string()?,
                value: None,
            }),
            LuaValue::Table(entries) => match entries.as_slice() {
                [(LuaValue::String(key), value)] => visitor.visit_enum(Variant {
                    owner: name,
                    variant: String::from_utf8(key.clone())
                        .map_err(|_| LuaError::new("a variant name that is not UTF-8"))?,
                    value: Some(value),
                }),
                _ => Err(LuaError::new(format!(
                    "a {name} is a variant name, or a table of one variant and its value"
                ))),
            },
            _ => Err(self.expected(&format!("a {name}"))),
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, LuaError> {
        self.deserialize_string(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, LuaError> {
        visitor.visit_unit()
    }
}

struct Seq<'a> {
    items: std::vec::IntoIter<&'a LuaValue>,
    len: usize,
    next: usize,
    scale: Scale,
}

impl<'a> Seq<'a> {
    fn new(items: Vec<&'a LuaValue>, scale: Scale) -> Self {
        Self {
            len: items.len(),
            items: items.into_iter(),
            next: 0,
            scale,
        }
    }
}

impl<'de> de::SeqAccess<'de> for Seq<'_> {
    type Error = LuaError;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, LuaError> {
        let Some(value) = self.items.next() else {
            return Ok(None);
        };
        self.next += 1;
        let index = self.next;
        seed.deserialize(De {
            value,
            scale: self.scale,
        })
        .map(Some)
        .map_err(|e| e.within(format!("[{index}]")))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.len)
    }
}

struct Fields<'a> {
    owner: &'static str,
    entries: std::vec::IntoIter<(&'static str, &'a LuaValue)>,
    pending: Option<(&'static str, &'a LuaValue)>,
}

impl<'de> de::MapAccess<'de> for Fields<'_> {
    type Error = LuaError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, LuaError> {
        let Some((field, value)) = self.entries.next() else {
            return Ok(None);
        };
        self.pending = Some((field, value));
        seed.deserialize(field.into_deserializer()).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, LuaError> {
        let (field, value) = self
            .pending
            .take()
            .ok_or_else(|| LuaError::new("a value without its field"))?;
        seed.deserialize(De {
            value,
            scale: field_scale(self.owner, field),
        })
        .map_err(|e| e.within(field.to_owned()))
    }
}

struct Variant<'a> {
    owner: &'static str,
    variant: String,
    value: Option<&'a LuaValue>,
}

impl<'de, 'a> de::EnumAccess<'de> for Variant<'a> {
    type Error = LuaError;
    type Variant = VariantValue<'a>;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, VariantValue<'a>), LuaError> {
        let scale = variant_scale(self.owner, &self.variant);
        let chosen = seed.deserialize(self.variant.clone().into_deserializer())?;
        Ok((
            chosen,
            VariantValue {
                name: self.variant,
                value: self.value,
                scale,
            },
        ))
    }
}

struct VariantValue<'a> {
    name: String,
    value: Option<&'a LuaValue>,
    scale: Scale,
}

impl<'a> VariantValue<'a> {
    fn value(&self) -> Result<De<'a>, LuaError> {
        let value = self.value.ok_or_else(|| {
            LuaError::new(format!(
                "{} needs a value: {{ {} = ... }}",
                self.name, self.name
            ))
        })?;
        Ok(De {
            value,
            scale: self.scale,
        })
    }
}

impl<'de> de::VariantAccess<'de> for VariantValue<'_> {
    type Error = LuaError;

    fn unit_variant(self) -> Result<(), LuaError> {
        match self.value {
            None => Ok(()),
            Some(_) => Err(LuaError::new(format!(
                "{} carries nothing: write it as the string \"{}\"",
                self.name, self.name
            ))),
        }
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, LuaError> {
        let de = self.value()?;
        seed.deserialize(de).map_err(|e| e.within(self.name))
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, LuaError> {
        let de = self.value()?;
        de::Deserializer::deserialize_tuple(de, len, visitor).map_err(|e| e.within(self.name))
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, LuaError> {
        let de = self.value()?;
        // No struct variant has a field in other units.
        de::Deserializer::deserialize_struct(de, "variant", fields, visitor)
            .map_err(|e| e.within(self.name))
    }
}

// ---------- action -> table ----------

struct Ser {
    scale: Scale,
}

impl Ser {
    fn integer(&self, n: i128) -> Result<LuaValue, LuaError> {
        let value = n as f64 / self.scale.factor();
        if (n as f64).abs() > EXACT {
            return Err(LuaError::new(format!("{n} is not exact as a Lua number")));
        }
        Ok(LuaValue::Number(value))
    }
}

fn sequence(items: Vec<LuaValue>) -> LuaValue {
    LuaValue::Table(
        items
            .into_iter()
            .enumerate()
            .map(|(i, v)| (LuaValue::Number((i + 1) as f64), v))
            .collect(),
    )
}

fn one_entry(key: &str, value: LuaValue) -> LuaValue {
    LuaValue::Table(vec![(LuaValue::string(key), value)])
}

macro_rules! serialize_integers {
    ($($method:ident $ty:ty;)*) => {
        $(fn $method(self, v: $ty) -> Result<LuaValue, LuaError> {
            self.integer(i128::from(v))
        })*
    };
}

impl ser::Serializer for Ser {
    type Ok = LuaValue;
    type Error = LuaError;
    type SerializeSeq = Items;
    type SerializeTuple = Items;
    type SerializeTupleStruct = Items;
    type SerializeTupleVariant = Items;
    type SerializeMap = ser::Impossible<LuaValue, LuaError>;
    type SerializeStruct = Record;
    type SerializeStructVariant = Record;

    fn serialize_bool(self, v: bool) -> Result<LuaValue, LuaError> {
        Ok(LuaValue::Boolean(v))
    }

    serialize_integers! {
        serialize_i8 i8;
        serialize_i16 i16;
        serialize_i32 i32;
        serialize_i64 i64;
        serialize_u8 u8;
        serialize_u16 u16;
        serialize_u32 u32;
        serialize_u64 u64;
    }

    fn serialize_f32(self, _: f32) -> Result<LuaValue, LuaError> {
        Err(LuaError::new("the schema has no floats"))
    }

    fn serialize_f64(self, _: f64) -> Result<LuaValue, LuaError> {
        Err(LuaError::new("the schema has no floats"))
    }

    fn serialize_char(self, v: char) -> Result<LuaValue, LuaError> {
        Ok(LuaValue::string(&v.to_string()))
    }

    fn serialize_str(self, v: &str) -> Result<LuaValue, LuaError> {
        Ok(LuaValue::string(v))
    }

    fn serialize_bytes(self, _: &[u8]) -> Result<LuaValue, LuaError> {
        Err(LuaError::new("the schema has no raw bytes"))
    }

    fn serialize_none(self) -> Result<LuaValue, LuaError> {
        Ok(LuaValue::Nil)
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<LuaValue, LuaError> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<LuaValue, LuaError> {
        Ok(LuaValue::Nil)
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<LuaValue, LuaError> {
        Ok(LuaValue::Nil)
    }

    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
    ) -> Result<LuaValue, LuaError> {
        Ok(LuaValue::string(variant))
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<LuaValue, LuaError> {
        if name == "Fraction" {
            return value.serialize(Ser {
                scale: Scale::Micro,
            });
        }
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<LuaValue, LuaError> {
        let inner = value
            .serialize(Ser {
                scale: variant_scale(name, variant),
            })
            .map_err(|e| e.within(variant.to_owned()))?;
        Ok(one_entry(variant, inner))
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Items, LuaError> {
        Ok(Items::new(len, Scale::One, None))
    }

    fn serialize_tuple(self, len: usize) -> Result<Items, LuaError> {
        Ok(Items::new(Some(len), self.scale, None))
    }

    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<Items, LuaError> {
        Ok(Items::new(Some(len), self.scale, None))
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Items, LuaError> {
        Ok(Items::new(Some(len), Scale::One, Some(variant)))
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, LuaError> {
        Err(LuaError::new("the schema has no maps"))
    }

    fn serialize_struct(self, name: &'static str, _: usize) -> Result<Record, LuaError> {
        Ok(Record {
            owner: name,
            entries: Vec::new(),
            variant: None,
        })
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Record, LuaError> {
        Ok(Record {
            owner: "variant",
            entries: Vec::new(),
            variant: Some(variant),
        })
    }
}

struct Items {
    items: Vec<LuaValue>,
    scale: Scale,
    variant: Option<&'static str>,
}

impl Items {
    fn new(len: Option<usize>, scale: Scale, variant: Option<&'static str>) -> Self {
        Self {
            items: Vec::with_capacity(len.unwrap_or(0)),
            scale,
            variant,
        }
    }

    fn push<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), LuaError> {
        let index = self.items.len() + 1;
        let item = value
            .serialize(Ser { scale: self.scale })
            .map_err(|e| e.within(format!("[{index}]")))?;
        self.items.push(item);
        Ok(())
    }

    fn done(self) -> LuaValue {
        let table = sequence(self.items);
        match self.variant {
            Some(variant) => one_entry(variant, table),
            None => table,
        }
    }
}

impl ser::SerializeSeq for Items {
    type Ok = LuaValue;
    type Error = LuaError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), LuaError> {
        self.push(value)
    }
    fn end(self) -> Result<LuaValue, LuaError> {
        Ok(self.done())
    }
}

impl ser::SerializeTuple for Items {
    type Ok = LuaValue;
    type Error = LuaError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), LuaError> {
        self.push(value)
    }
    fn end(self) -> Result<LuaValue, LuaError> {
        Ok(self.done())
    }
}

impl ser::SerializeTupleStruct for Items {
    type Ok = LuaValue;
    type Error = LuaError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), LuaError> {
        self.push(value)
    }
    fn end(self) -> Result<LuaValue, LuaError> {
        Ok(self.done())
    }
}

impl ser::SerializeTupleVariant for Items {
    type Ok = LuaValue;
    type Error = LuaError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), LuaError> {
        self.push(value)
    }
    fn end(self) -> Result<LuaValue, LuaError> {
        Ok(self.done())
    }
}

struct Record {
    owner: &'static str,
    entries: Vec<(LuaValue, LuaValue)>,
    variant: Option<&'static str>,
}

impl Record {
    fn field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), LuaError> {
        let value = value
            .serialize(Ser {
                scale: field_scale(self.owner, key),
            })
            .map_err(|e| e.within(key.to_owned()))?;
        // A None is a field left out: a Lua table cannot hold nil.
        if value != LuaValue::Nil {
            self.entries.push((LuaValue::string(key), value));
        }
        Ok(())
    }

    fn done(self) -> LuaValue {
        let table = LuaValue::Table(self.entries);
        match self.variant {
            Some(variant) => one_entry(variant, table),
            None => table,
        }
    }
}

impl ser::SerializeStruct for Record {
    type Ok = LuaValue;
    type Error = LuaError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), LuaError> {
        self.field(key, value)
    }
    fn end(self) -> Result<LuaValue, LuaError> {
        Ok(self.done())
    }
}

impl ser::SerializeStructVariant for Record {
    type Ok = LuaValue;
    type Error = LuaError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), LuaError> {
        self.field(key, value)
    }
    fn end(self) -> Result<LuaValue, LuaError> {
        Ok(self.done())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{
        EdgeRef, Link, ParamValue, Polyline, Pos, Resolve, RoadBuild, Structure, Tangent, Tram,
        Vertex,
    };
    use crate::{BoundedVec, Text};

    fn n(v: f64) -> LuaValue {
        LuaValue::Number(v)
    }
    fn s(v: &str) -> LuaValue {
        LuaValue::string(v)
    }
    fn t(entries: Vec<(&str, LuaValue)>) -> LuaValue {
        LuaValue::Table(entries.into_iter().map(|(k, v)| (s(k), v)).collect())
    }
    fn seq(items: Vec<LuaValue>) -> LuaValue {
        sequence(items)
    }
    fn pos(x: f64, y: f64, z: f64) -> LuaValue {
        t(vec![("x", n(x)), ("y", n(y)), ("z", n(z))])
    }

    fn road(vertex0: LuaValue) -> LuaValue {
        t(vec![(
            "BuildRoad",
            t(vec![
                ("street", s("street/standard/town_medium_new.lua")),
                ("bus_lane", LuaValue::Boolean(false)),
                ("tram", s("None")),
                (
                    "polyline",
                    t(vec![
                        (
                            "vertices",
                            seq(vec![
                                vertex0,
                                t(vec![("pos", pos(10.0, 0.0, 0.0)), ("resolve", s("New"))]),
                            ]),
                        ),
                        (
                            "links",
                            seq(vec![t(vec![
                                ("from", n(0.0)),
                                ("to", n(1.0)),
                                ("tangent0", pos(10.0, 0.0, 0.0)),
                                ("tangent1", pos(10.0, 0.0, 0.0)),
                                ("structure", t(vec![("Bridge", s("bridge/cement.lua"))])),
                            ])]),
                        ),
                        ("removals", seq(vec![])),
                        ("removed_nodes", seq(vec![])),
                    ]),
                ),
            ]),
        )])
    }

    fn vertex(x: f64) -> LuaValue {
        t(vec![
            ("pos", pos(x, -0.0625, -2.5)),
            ("resolve", t(vec![("Node", s("Street"))])),
        ])
    }

    #[test]
    fn a_road_in_metres_becomes_millimetres() {
        let action = action_from_lua(&road(vertex(1.0625))).unwrap();
        let tangent = Tangent {
            x: 10_000,
            y: 0,
            z: 0,
        };
        let expected = Action::BuildRoad(RoadBuild {
            street: Text::new("street/standard/town_medium_new.lua").unwrap(),
            style: None,
            bus_lane: false,
            tram: Tram::None,
            polyline: Polyline::new(
                BoundedVec::new(vec![
                    Vertex {
                        // 1.0625 m is 1062.5 mm and -0.0625 m is -62.5 mm:
                        // halves round away from zero.
                        pos: Pos {
                            x: 1_063,
                            y: -63,
                            z: -2_500,
                        },
                        resolve: Resolve::Node(crate::action::Network::Street),
                    },
                    Vertex {
                        pos: Pos {
                            x: 10_000,
                            y: 0,
                            z: 0,
                        },
                        resolve: Resolve::New,
                    },
                ])
                .unwrap(),
                BoundedVec::new(vec![Link {
                    precedence: None,
                    position0: None,
                    position1: None,
                    kept: BoundedVec::default(),

                    from: 0,
                    to: 1,
                    tangent0: tangent,
                    tangent1: tangent,
                    structure: Structure::Bridge(Text::new("bridge/cement.lua").unwrap()),
                    kind: None,
                    decorations: BoundedVec::default(),
                    locked: false,
                    owned: false,
                    lanes: BoundedVec::default(),
                }])
                .unwrap(),
                BoundedVec::<EdgeRef, 256>::empty(),
            )
            .unwrap(),
            crossing_type: None,
        });
        assert_eq!(action, expected);
        let back = action_to_lua(&action).unwrap();
        assert_eq!(action_from_lua(&back).unwrap(), expected);
    }

    #[test]
    fn a_loan_keeps_its_terms_and_its_rate_in_millionths() {
        // The finance window's "Obtain": the offer drawn to follow, and the
        // offer taken, as the loan script's tables have them.
        let next = t(vec![
            ("type", s("Small")),
            ("amount", n(7_000_000.0)),
            ("duration", n(2_190_000.0)),
            ("percentage", n(0.04)),
            ("birthDay", n(1_234_000.0)),
        ]);
        let offer = t(vec![
            ("type", s("Small")),
            ("amount", n(5_000_000.0)),
            ("duration", n(1_095_000.0)),
            ("percentage", n(0.03)),
            ("birthDay", n(400_000.0)),
        ]);
        let table = t(vec![(
            "Loan",
            t(vec![("Take", t(vec![("next", next), ("offer", offer)]))]),
        )]);
        let action = action_from_lua(&table).unwrap();
        let Action::Loan(op) = &action else {
            panic!("{action:?}");
        };
        let crate::action::LoanOp::Take { next, offer } = op.as_ref() else {
            panic!("{action:?}");
        };
        assert_eq!(offer.kind.as_str(), "Small");
        assert_eq!(offer.amount, 5_000_000);
        assert_eq!(offer.percentage, 30_000, "0.03 in millionths");
        assert_eq!(offer.birth_day, Some(400_000));
        assert_eq!(offer.id, None);
        assert_eq!(next.percentage, 40_000);
        let back = action_to_lua(&action).unwrap();
        assert_eq!(action_from_lua(&back).unwrap(), action);
        // A rate finer than a millionth is rounded, a fractional amount
        // refused.
        let odd = t(vec![(
            "Loan",
            t(vec![(
                "Repay",
                t(vec![(
                    "loan",
                    t(vec![
                        ("type", s("Custom")),
                        ("amount", n(1.5)),
                        ("duration", n(1.0)),
                        ("percentage", n(0.1)),
                    ]),
                )]),
            )]),
        )]);
        assert_eq!(
            refusal(odd),
            "Loan.Repay.loan.amount: not a whole number: 1.5"
        );
    }

    fn refusal(value: LuaValue) -> String {
        action_from_lua(&value).unwrap_err().to_string()
    }

    #[test]
    fn a_bad_number_is_refused_with_its_path() {
        assert_eq!(
            refusal(road(vertex(f64::NAN))),
            "BuildRoad.polyline.vertices[1].pos.x: not a finite number: NaN"
        );
        assert_eq!(
            refusal(road(vertex(3_000_000.0))),
            "BuildRoad.polyline.vertices[1].pos.x: out of range for i32: 3000000000"
        );
        assert_eq!(
            refusal(road(t(vec![
                ("pos", pos(0.0, 0.0, 0.0)),
                ("resolve", s("Node"))
            ]))),
            "BuildRoad.polyline.vertices[1].resolve: Node needs a value: { Node = ... }"
        );
    }

    #[test]
    fn fields_are_exactly_the_schema_s() {
        let mut extra = vertex(0.0);
        if let LuaValue::Table(entries) = &mut extra {
            entries.push((s("colour"), s("red")));
        }
        assert_eq!(
            refusal(road(extra)),
            "BuildRoad.polyline.vertices[1]: Vertex has no field colour"
        );
        let missing = t(vec![("pos", pos(0.0, 0.0, 0.0))]);
        assert_eq!(
            refusal(road(missing)),
            "BuildRoad.polyline.vertices[1]: missing field `resolve`"
        );
    }

    #[test]
    fn a_whole_number_is_whole() {
        let fraction = |from: f64| {
            let mut value = road(vertex(0.0));
            let LuaValue::Table(outer) = &mut value else {
                unreachable!()
            };
            let polyline = outer[0].1.get("polyline").unwrap().clone();
            let LuaValue::Table(mut fields) = polyline else {
                unreachable!()
            };
            fields[1] = (
                s("links"),
                seq(vec![t(vec![
                    ("from", n(from)),
                    ("to", n(1.0)),
                    ("tangent0", pos(1.0, 0.0, 0.0)),
                    ("tangent1", pos(1.0, 0.0, 0.0)),
                    ("structure", s("Ground")),
                ])]),
            );
            refusal(t(vec![(
                "BuildRoad",
                t(vec![
                    ("street", s("a.lua")),
                    ("bus_lane", LuaValue::Boolean(false)),
                    ("tram", s("None")),
                    ("polyline", LuaValue::Table(fields)),
                ]),
            )]))
        };
        assert_eq!(
            fraction(0.5),
            "BuildRoad.polyline.links[1].from: not a whole number: 0.5"
        );
        assert_eq!(
            fraction(-1.0),
            "BuildRoad.polyline.links[1].from: out of range for u16: -1"
        );
    }

    #[test]
    fn a_sequence_has_keys_one_to_n() {
        let gappy = LuaValue::Table(vec![(n(1.0), n(1.0)), (n(3.0), n(2.0))]);
        let action = t(vec![("SellVehicle", t(vec![("vehicles", gappy)]))]);
        assert_eq!(
            refusal(action),
            "SellVehicle.vehicles: expected a sequence { a, b, ... }"
        );
        let ok = t(vec![(
            "SellVehicle",
            t(vec![("vehicles", seq(vec![n(7.0), n(9.0)]))]),
        )]);
        assert!(action_from_lua(&ok).is_ok());
    }

    #[test]
    fn a_unit_variant_is_a_string_and_nothing_else() {
        let action = t(vec![(
            "EditLine",
            t(vec![
                ("line", n(3.0)),
                ("change", t(vec![("Delete", LuaValue::Boolean(true))])),
            ]),
        )]);
        assert_eq!(
            refusal(action),
            "EditLine.change: Delete carries nothing: write it as the string \"Delete\""
        );
        assert_eq!(
            refusal(s("Nonsense")),
            "unknown variant `Nonsense`, expected one of `BuildRoad`, `BuildTrack`, \
             `Bulldoze`, `BuildConstruction`, `BuyVehicle`, `SellVehicle`, `CreateLine`, \
             `EditLine`, `AssignLine`, `PlaceStop`, `Terraform`, `CompanyOp`, `Loan`, `VehicleOp`, \
             `ReplaceVehicle`, `Prospect`, `NotificationSeen`, `ApplyRank`, `EditJunctions`, \
             `Subsidy`, `Rename`, `Perk`, `Preserve`, `CalendarSpeed`, `PlaceSignals`, `Paint`, `PlantAssets`, `SetCrossingType`"
        );
    }

    #[test]
    fn text_must_be_utf8_and_within_bounds() {
        let action = |name: LuaValue| {
            t(vec![(
                "CompanyOp",
                t(vec![("Create", t(vec![("name", name)]))]),
            )])
        };
        assert_eq!(
            refusal(action(LuaValue::String(vec![0xff, 0xfe]))),
            "CompanyOp.Create.name: a string that is not UTF-8"
        );
        assert!(refusal(action(s(&"x".repeat(65)))).starts_with("CompanyOp.Create.name: "));
        assert!(action_from_lua(&action(s("Rail & Co"))).is_ok());
    }

    #[test]
    fn fixed_parameters_are_millionths() {
        let fixed = ParamValue::Fixed(-1_500_000);
        let value = fixed.serialize(Ser { scale: Scale::One }).unwrap();
        assert_eq!(value, t(vec![("Fixed", n(-1.5))]));
        let back = ParamValue::deserialize(De {
            value: &value,
            scale: Scale::One,
        })
        .unwrap();
        assert_eq!(back, fixed);
        let int = ParamValue::Int(1 << 54);
        assert!(
            int.serialize(Ser { scale: Scale::One }).is_err(),
            "not exact as a double"
        );
    }

    #[test]
    fn lua_integers_are_taken_too() {
        let action = t(vec![(
            "SellVehicle",
            t(vec![(
                "vehicles",
                LuaValue::Table(vec![(LuaValue::Integer(1), LuaValue::Integer(4))]),
            )]),
        )]);
        assert!(action_from_lua(&action).is_ok());
    }

    #[test]
    fn a_pos_round_trips_at_every_scale() {
        for value in [0, 1, -1, 999, 1_000_001, i32::MAX, i32::MIN, 123_456_789] {
            let pos = Pos {
                x: value,
                y: value / -2,
                z: value / 7,
            };
            let lua = pos.serialize(Ser { scale: Scale::One }).unwrap();
            let back = Pos::deserialize(De {
                value: &lua,
                scale: Scale::One,
            })
            .unwrap();
            assert_eq!(back, pos);
        }
    }
}
