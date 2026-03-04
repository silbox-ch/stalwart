/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use crate::{
    object::JmapObject,
    request::{
        IntoValid, MaybeInvalid,
        deserialize::{DeserializeArguments, deserialize_request},
        reference::{MaybeIdReference, MaybeResultReference, ResultReference},
    },
    types::state::State,
};
use jmap_tools::Value;
use serde::{Deserialize, Deserializer, ser::SerializeSeq};
use types::id::Id;

#[derive(Debug, Clone)]
pub struct GetRequest<T: JmapObject> {
    pub account_id: Id,
    pub ids: Option<MaybeResultReference<Vec<MaybeIdReference<T::Id>>>>,
    pub properties: Option<MaybeResultReference<Vec<MaybeInvalid<T::Property>>>>,
    pub arguments: T::GetArguments,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetResponse<T: JmapObject> {
    #[serde(rename = "accountId")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<Id>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<State>,

    pub list: Vec<Value<'static, T::Property, T::Element>>,

    #[serde(rename = "notFound")]
    pub not_found: NotFoundIds<T::Id>,
}

/// Holds both parsed IDs (that were valid but not found in the store)
/// and unparseable ID strings (that could not be decoded as server IDs).
/// Serializes as a single flat JSON array, per JMAP spec RFC 8620 §5.1.
#[derive(Debug, Clone)]
pub struct NotFoundIds<I> {
    ids: Vec<I>,
    invalid: Vec<String>,
}

impl<I> NotFoundIds<I> {
    pub fn new() -> Self {
        Self {
            ids: Vec::new(),
            invalid: Vec::new(),
        }
    }

    pub fn push(&mut self, id: I) {
        self.ids.push(id);
    }

    pub fn add_invalid(&mut self, strings: Vec<String>) {
        self.invalid.extend(strings);
    }
}

impl<I> Default for NotFoundIds<I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: serde::Serialize> serde::Serialize for NotFoundIds<I> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.ids.len() + self.invalid.len()))?;
        for id in &self.ids {
            seq.serialize_element(id)?;
        }
        for s in &self.invalid {
            seq.serialize_element(s)?;
        }
        seq.end()
    }
}

impl<'de, T: JmapObject> DeserializeArguments<'de> for GetRequest<T> {
    fn deserialize_argument<A>(&mut self, key: &str, map: &mut A) -> Result<(), A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        hashify::fnc_map!(key.as_bytes(),
            b"accountId" => {
                self.account_id = map.next_value()?;
            },
            b"ids" => {
                self.ids = map.next_value::<Option<Vec<MaybeIdReference<T::Id>>>>()?.map(MaybeResultReference::Value);
            },
            b"properties" => {
                self.properties = map.next_value::<Option<Vec<MaybeInvalid<T::Property>>>>()?.map(MaybeResultReference::Value);
            },
            b"#ids" => {
                self.ids = Some(MaybeResultReference::Reference(map.next_value::<ResultReference>()?));
            },
            b"#properties" => {
                self.properties = Some(MaybeResultReference::Reference(map.next_value::<ResultReference>()?));
            },
            _ => {
                self.arguments.deserialize_argument(key, map)?;
            }
        );

        Ok(())
    }
}

impl<'de, T: JmapObject> Deserialize<'de> for GetRequest<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_request(deserializer)
    }
}

impl<T: JmapObject> Default for GetRequest<T> {
    fn default() -> Self {
        Self {
            account_id: Id::default(),
            ids: None,
            properties: None,
            arguments: T::GetArguments::default(),
        }
    }
}

impl<T: JmapObject> GetRequest<T> {
    pub fn unwrap_properties(&mut self, default: &[T::Property]) -> Vec<T::Property> {
        if let Some(properties_) = self.properties.take().map(|p| p.unwrap()) {
            let mut properties = Vec::with_capacity(properties_.len());
            let id_prop = T::ID_PROPERTY;
            let mut has_id = false;

            for prop in properties_ {
                if let MaybeInvalid::Value(p) = prop {
                    if p == id_prop {
                        has_id = true;
                    }
                    properties.push(p);
                }
            }

            if !has_id {
                properties.push(id_prop);
            }

            properties
        } else {
            default.to_vec()
        }
    }

    pub fn unwrap_ids(
        &mut self,
        max_objects_in_get: usize,
    ) -> trc::Result<(Option<Vec<T::Id>>, Vec<String>)> {
        if let Some(ids) = self.ids.take() {
            let ids = ids.unwrap();
            if ids.len() <= max_objects_in_get {
                let mut valid = Vec::new();
                let mut invalid = Vec::new();
                for id in ids {
                    match id {
                        MaybeIdReference::Id(id) => valid.push(id),
                        MaybeIdReference::Reference(s) | MaybeIdReference::Invalid(s) => {
                            invalid.push(s);
                        }
                    }
                }
                Ok((Some(valid), invalid))
            } else {
                Err(trc::JmapEvent::RequestTooLarge.into_err())
            }
        } else {
            Ok((None, Vec::new()))
        }
    }
}
